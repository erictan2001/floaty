//! `live2d_model_library` — moved out of lib.rs verbatim.
//!
//! Visibility is `pub(crate)` because the rest of the crate calls in across the module
//! boundary now; nothing here changed shape on the way out.

use crate::*;
use serde::{ Serialize};
use std::collections::{HashMap};
use std::fs;
use tauri::{AppHandle, Emitter, Manager};

// ---------- live2d model library ----------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Live2dModelEntry {
    pub(crate) name: String,
    pub(crate) path: String,
}

pub(crate) struct ScoredModelEntry {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) score: i32,
    pub(crate) parent: String,
}

pub(crate) fn is_generic_model_stem(stem: &str) -> bool {
    let s = stem.to_ascii_lowercase();
    matches!(s.as_str(), "model" | "index" | "model3" | "character" | "main")
}

pub(crate) fn extract_model_stem(file_name: &str) -> Option<&str> {
    let fl = file_name.to_ascii_lowercase();
    if fl.ends_with(".model.json") {
        Some(&file_name[..file_name.len() - ".model.json".len()])
    } else if fl.ends_with(".model3.json") {
        Some(&file_name[..file_name.len() - ".model3.json".len()])
    } else {
        None
    }
}

/// Blocking walk for *.model.json / *.model3.json up to 3 levels deep.
/// Deduplicates duplicate model files across directories and extracts clean display names.
pub(crate) fn scan_models_blocking(root: String) -> Vec<Live2dModelEntry> {
    let base = std::path::PathBuf::from(&root);
    if !base.is_dir() {
        return vec![];
    }
    let mut model_map: HashMap<String, ScoredModelEntry> = HashMap::new();
    let mut stack: Vec<(std::path::PathBuf, u8)> = vec![(base, 0)];

    while let Some((dir, depth)) = stack.pop() {
        if depth > 6 {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if depth < 6 {
                    let hidden = p
                        .file_name()
                        .and_then(|s| s.to_str())
                        .map(|s| s.starts_with('.'))
                        .unwrap_or(true);
                    if !hidden {
                        stack.push((p, depth + 1));
                    }
                }
                continue;
            }
            let Some(file_name) = p.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(stem) = extract_model_stem(file_name) else {
                continue;
            };

            let parent = p
                .parent()
                .and_then(|d| d.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            let is_generic = is_generic_model_stem(stem);
            let dedup_key = if is_generic {
                format!("{}/{}", parent.to_lowercase(), stem.to_lowercase())
            } else {
                stem.to_lowercase()
            };

            let mut score = 0;
            let p_lower = parent.to_lowercase();
            let s_lower = stem.to_lowercase();
            if p_lower == s_lower {
                score += 100;
            } else if p_lower.contains(&s_lower) || s_lower.contains(&p_lower) {
                score += 50;
            }

            if let Some(existing) = model_map.get(&dedup_key) {
                if existing.score >= score {
                    continue;
                }
            }

            // Extract display name, checking JSON name field if available
            let mut display_name = if is_generic {
                if parent.is_empty() {
                    stem.to_string()
                } else {
                    parent.clone()
                }
            } else {
                stem.to_string()
            };

            if let Ok(content) = fs::read_to_string(&p) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(jn) = val.get("name").and_then(|v| v.as_str()).map(|s| s.trim()) {
                        if !jn.is_empty()
                            && !jn.eq_ignore_ascii_case(stem)
                            && !is_generic_model_stem(jn)
                        {
                            if is_generic && !parent.is_empty() {
                                display_name = format!("{parent} ({jn})");
                            } else {
                                display_name = format!("{stem} ({jn})");
                            }
                        }
                    }
                }
            }

            model_map.insert(
                dedup_key,
                ScoredModelEntry {
                    name: display_name,
                    path: p.to_string_lossy().to_string(),
                    score,
                    parent,
                },
            );
        }
    }

    // Check for display name collisions and disambiguate with parent folder if needed
    let mut name_counts: HashMap<String, usize> = HashMap::new();
    for entry in model_map.values() {
        *name_counts.entry(entry.name.clone()).or_insert(0) += 1;
    }

    let mut out: Vec<Live2dModelEntry> = model_map
        .into_values()
        .map(|entry| {
            let final_name = if name_counts.get(&entry.name).copied().unwrap_or(0) > 1
                && !entry.parent.is_empty()
                && !entry.name.contains(&entry.parent)
            {
                format!("{} ({})", entry.name, entry.parent)
            } else {
                entry.name
            };
            Live2dModelEntry {
                name: final_name,
                path: entry.path,
            }
        })
        .collect();

    out.sort_by_key(|a| a.name.to_lowercase());
    out.truncate(1000);
    out
}

#[tauri::command]
pub(crate) async fn floaty_scan_models(root: String, app: AppHandle) -> Vec<Live2dModelEntry> {
    let out = tauri::async_runtime::spawn_blocking(move || scan_models_blocking(root))
        .await
        .unwrap_or_default();
    log_line(&app, &format!("model scan found {} models", out.len()));
    out
}

#[tauri::command]
pub(crate) fn floaty_set_widget_model(id: String, model: String, app: AppHandle) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        let rec = guard.widgets.get_mut(&id).ok_or("widget not found")?;
        if rec.kind != "live2d" {
            return Err("not a live2d widget".into());
        }
        if let Some(obj) = rec.data.as_object_mut() {
            obj.insert("model".to_string(), serde_json::Value::String(model));
        }
    }
    persist(&app);
    app.emit("floaty-live2d-changed", &id).ok();
    log_line(&app, &format!("live2d model changed for {id}"));
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct WindowCursorPos {
    pub(crate) rel_x: f64,
    pub(crate) rel_y: f64,
}

/// Returns the cursor position relative to the specified window's top-left corner
/// in logical pixels, tracking the cursor across the entire desktop.
#[tauri::command]
pub(crate) fn floaty_window_cursor_pos(label: String, app: AppHandle) -> Option<WindowCursorPos> {
    let w = app.get_webview_window(&label)?;
    let win_pos = w.outer_position().ok()?;
    let scale = w.scale_factor().unwrap_or(1.0);
    if scale <= 0.0 {
        return None;
    }

    #[cfg(windows)]
    {
        use std::mem::MaybeUninit;
        // As above: the Win32 struct's own name.
        #[allow(clippy::upper_case_acronyms)]
        #[repr(C)]
        struct POINT {
            pub(crate) x: i32,
            pub(crate) y: i32,
        }
        extern "system" {
            fn GetCursorPos(lpPoint: *mut POINT) -> i32;
        }
        let mut pt = MaybeUninit::<POINT>::uninit();
        if unsafe { GetCursorPos(pt.as_mut_ptr()) } != 0 {
            let pt = unsafe { pt.assume_init() };
            return Some(WindowCursorPos {
                rel_x: (pt.x as f64 - win_pos.x as f64) / scale,
                rel_y: (pt.y as f64 - win_pos.y as f64) / scale,
            });
        }
    }

    let cur = app.cursor_position().ok()?;
    Some(WindowCursorPos {
        rel_x: (cur.x - win_pos.x as f64) / scale,
        rel_y: (cur.y - win_pos.y as f64) / scale,
    })
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LayoutItem {
    pub(crate) id: String,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) w: i32,
    pub(crate) h: i32,
}

/// Logical-px rects of all gravity widgets, for inter-icon separation.
#[tauri::command]
pub(crate) fn floaty_layout(app: AppHandle) -> Vec<LayoutItem> {
    let state = app.state::<AppState>();
    let guard = match state.0.lock() {
        Ok(g) => g,
        Err(_) => return vec![],
    };
    guard
        .widgets
        .values()
        // What the desktop is made of is what an icon can land on: every
        // desktop item, folders included. This is the one-window-per-widget
        // half of the same list the overlay builds from its own slots, and
        // `is_path_kind` here (which excludes folders, and is about a record
        // carrying its own path) left both folders and files out of the
        // physics, so icons dropped from above fell straight through them.
        .filter(|r| plugins::is_desktop_item(&r.kind))
        .map(|r| {
            let (w, h) = plugins::size(&r.kind, &r.data);
            LayoutItem {
                id: r.id.clone(),
                x: r.x,
                y: r.y,
                w: w as i32,
                h: h as i32,
            }
        })
        .collect()
}
