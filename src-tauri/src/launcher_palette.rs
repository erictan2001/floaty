//! `launcher_palette` — moved out of lib.rs verbatim.
//!
//! Visibility is `pub(crate)` because the rest of the crate calls in across the module
//! boundary now; nothing here changed shape on the way out.

use crate::*;
use std::collections::{ HashSet};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

// ---------- launcher palette ----------

/// The apps the launcher searches, cached.
///
/// `scan_apps_blocking` walks two Start Menu trees and the desktops: fine once,
/// when the Apps tab asks for it, and far too slow to repeat on every keystroke.
/// Five minutes is the compromise between "a program installed a moment ago is not
/// findable" and "the launcher hitches".
/// When the apps were last scanned, and what came back.
type AppScan = (std::time::Instant, Vec<DiscoveredApp>);
pub(crate) static APP_SCAN: std::sync::LazyLock<Mutex<Option<AppScan>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

pub(crate) fn cached_apps() -> Vec<DiscoveredApp> {
    const TTL: std::time::Duration = std::time::Duration::from_secs(300);
    if let Ok(guard) = APP_SCAN.lock() {
        if let Some((at, list)) = guard.as_ref() {
            if at.elapsed() < TTL {
                return list.clone();
            }
        }
    }
    let fresh = scan_apps_blocking();
    if let Ok(mut guard) = APP_SCAN.lock() {
        *guard = Some((std::time::Instant::now(), fresh.clone()));
    }
    fresh
}

/// The caption rule for a launcher row: a shortcut reads as the app it launches,
/// and a file keeps its extension, exactly as the desktop's captions do.
pub(crate) fn palette_caption(name: &str, is_dir: bool) -> String {
    if is_dir {
        return name.to_string();
    }
    let lower = name.to_lowercase();
    for extension in [".lnk", ".url"] {
        if lower.ends_with(extension) {
            return name[..name.len() - extension.len()].to_string();
        }
    }
    name.to_string()
}

/// Everything the launcher can find, ranked, with icons for the rows it returns.
///
/// Three sources: the applications a scan knows about (cached), the entries in the
/// pointed root, and every floatie's record — so a note or a clock is reachable by
/// name, not just a program. A floatie that stands for a file already listed from
/// the root is skipped: one desktop entry, one row.
pub(crate) fn palette_search(app: &AppHandle, query: &str) -> Vec<palette::PaletteHit> {
    let started = std::time::Instant::now();
    let settings = load_settings(app);
    let mut candidates: Vec<(String, palette::PaletteHit)> = Vec::new();
    // One desktop entry, one row: a floatie first claims its own path and name, and
    // the same thing found two other ways (the root listing, the Start Menu) is
    // skipped rather than drawn twice under two names.
    let mut seen_paths: HashSet<String> = HashSet::new();
    let mut seen_titles: HashSet<String> = HashSet::new();

    {
        let state = app.state::<AppState>();
        let guard = state.0.lock().unwrap_or_else(|e| e.into_inner());
        for rec in guard.widgets.values() {
            if settings.disabled.iter().any(|d| d == &rec.kind) {
                continue; // not on the desktop, so not in the launcher either
            }
            let path = plugins::path_key(&rec.kind)
                .and_then(|key| rec.data.get(key).and_then(|v| v.as_str()))
                .map(|s| s.to_string());
            if let Some(p) = &path {
                if seen_paths.contains(&p.to_lowercase()) {
                    continue;
                }
            }
            let label = plugins::find(&rec.kind)
                .map(|p| p.name.to_string())
                .unwrap_or_else(|| rec.kind.clone());
            let name = rec
                .data
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| label.clone());
            let icon = rec
                .data
                .get("icon")
                .and_then(|v| v.as_str())
                .filter(|s| icons::is_stored_url(s))
                .unwrap_or("")
                .to_string();
            // The same caption rule the desktop uses: a floatie standing for
            // `Joplin.lnk` reads as `Joplin` here too, because it is the same thing
            // under the same name — and it is what makes the dedupe below catch the
            // Start Menu's copy of it.
            let title = palette_caption(&name, rec.kind == "folder");
            seen_paths.insert(path.clone().unwrap_or_default().to_lowercase());
            seen_titles.insert(title.to_lowercase());
            candidates.push((
                title.clone(),
                palette::PaletteHit {
                    kind: "floatie".to_string(),
                    title,
                    subtitle: if label.is_empty() {
                        "on your desktop".to_string()
                    } else {
                        label
                    },
                    icon,
                    id: Some(rec.id.clone()),
                    path,
                },
            ));
        }
    }

    if let Some(root) = files_root_dir(app) {
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let path = entry.path().to_string_lossy().to_string();
                let title = palette_caption(&name, is_dir);
                if seen_paths.contains(&path.to_lowercase())
                    || seen_titles.contains(&title.to_lowercase())
                {
                    continue;
                }
                seen_paths.insert(path.to_lowercase());
                seen_titles.insert(title.to_lowercase());
                candidates.push((
                    name.clone(),
                    palette::PaletteHit {
                        kind: if is_dir { "folder" } else { "file" }.to_string(),
                        title,
                        subtitle: root.to_string_lossy().to_string(),
                        icon: String::new(),
                        id: None,
                        path: Some(path),
                    },
                ));
            }
        }
    }

    for entry in cached_apps() {
        let title = palette_caption(&entry.name, false);
        if seen_paths.contains(&entry.path.to_lowercase())
            || seen_titles.contains(&title.to_lowercase())
        {
            continue;
        }
        seen_titles.insert(title.to_lowercase());
        candidates.push((
            title.clone(),
            palette::PaletteHit {
                kind: "app".to_string(),
                title,
                subtitle: entry.path.clone(),
                icon: String::new(),
                id: None,
                path: Some(entry.path),
            },
        ));
    }

    let keys: Vec<String> = candidates.iter().map(|(key, _)| key.clone()).collect();
    let mut hits: Vec<palette::PaletteHit> = palette::rank(query, &keys, palette::MAX_HITS)
        .into_iter()
        .map(|index| candidates[index].1.clone())
        .collect();

    // Icons last, and only for the rows that will actually be drawn. Resolving is a
    // PowerShell round trip the first time a path is seen, and paying it for
    // thousands of candidates nobody asked about is how a launcher gets a reputation
    // for being slow.
    let wanted: Vec<String> = hits
        .iter()
        .filter(|hit| hit.icon.is_empty())
        .filter_map(|hit| hit.path.clone())
        .collect();
    if !wanted.is_empty() {
        let icons = resolve_icons_batch(&wanted, false);
        for hit in hits.iter_mut() {
            if hit.icon.is_empty() {
                if let Some(icon) = hit.path.as_ref().and_then(|path| icons.get(path)) {
                    if icon != "none" {
                        hit.icon = icon.clone();
                    }
                }
            }
        }
    }

    log_line(
        app,
        &format!(
            "palette: '{}' -> {} hit(s) of {} candidate(s) in {}ms",
            query,
            hits.len(),
            candidates.len(),
            started.elapsed().as_millis()
        ),
    );
    hits
}

/// The launcher's rows: the same search the palette shows, for whatever asks.
#[tauri::command]
pub(crate) async fn floaty_palette_search(query: String, app: AppHandle) -> Vec<palette::PaletteHit> {
    tauri::async_runtime::spawn_blocking(move || palette_search(&app, &query))
        .await
        .unwrap_or_default()
}

/// What running a row reports: the line the palette shows. A run that could not
/// happen at all is an `Err`, which the page shows the same way.
#[derive(serde::Serialize)]
pub(crate) struct PaletteRun {
    pub(crate) note: String,
}

/// Run one row: launch an app, open a file or folder, or do what a double-click on
/// that floatie would do.
///
/// The palette closes first: a launcher that stays open after Enter is not a
/// launcher, and it would be the window in the way of what it just opened.
#[tauri::command]
pub(crate) async fn floaty_palette_run(
    hit: palette::PaletteHit,
    app: AppHandle,
) -> Result<PaletteRun, String> {
    palette::hide(&app);
    let path = hit.path.clone().unwrap_or_default();
    match hit.kind.as_str() {
        "app" | "file" | "folder" => {
            if path.trim().is_empty() {
                return Err("nothing to open".to_string());
            }
            launch_target(&app, &path)?;
            Ok(PaletteRun {
                note: format!("opened {path}"),
            })
        }
        "floatie" => {
            let id = hit.id.clone().unwrap_or_default();
            let target = {
                let state = app.state::<AppState>();
                let guard = state.0.lock().map_err(|e| e.to_string())?;
                guard.widgets.get(&id).and_then(|rec| {
                    plugins::path_key(&rec.kind).and_then(|key| {
                        rec.data
                            .get(key)
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.trim().is_empty())
                            .map(|s| s.to_string())
                    })
                })
            };
            match target {
                Some(target) => {
                    launch_target(&app, &target)?;
                    Ok(PaletteRun {
                        note: format!("launched {target}"),
                    })
                }
                // A note, a clock, a pet: it is already where it belongs. Saying so
                // is better than a run that looks like it failed.
                None => Ok(PaletteRun {
                    note: format!("{} lives on your desktop", hit.title),
                }),
            }
        }
        other => Err(format!("nothing here knows how to run a '{other}'")),
    }
}

#[tauri::command]
pub(crate) fn floaty_palette_hide(app: AppHandle) {
    palette::hide(&app);
}

/// Open the palette without the keyboard — the tray item and the settings button.
#[tauri::command]
pub(crate) fn floaty_show_palette(app: AppHandle) {
    show_palette(&app);
}

/// What the settings window shows about the hotkey: the key, whether it is really
/// registered (`Ctrl+Alt+Space` can be taken by another app), and whether the
/// palette is up.
#[derive(serde::Serialize)]
pub(crate) struct PaletteState {
    pub(crate) accelerator: String,
    pub(crate) live: bool,
    pub(crate) visible: bool,
}

#[tauri::command]
pub(crate) fn floaty_palette_state(app: AppHandle) -> PaletteState {
    let accelerator = load_settings(&app).palette_shortcut;
    PaletteState {
        live: palette::shortcut_is_live(&app, &accelerator),
        visible: palette::is_visible(&app),
        accelerator,
    }
}

/// Open the palette from anywhere, without blocking the caller.
///
/// **Building the window on the main thread deadlocks**, because the main thread is
/// the one that has to create it: measured, `floaty_show_palette` — a sync command,
/// so it runs on the main thread — never returned and no window ever appeared. The
/// tray handler and the hotkey handler are delivered on that same thread, so every
/// path that opens the palette comes through here instead. Showing the *existing*
/// window is safe from any thread; it is the first `build()` that cannot wait.
pub(crate) fn show_palette(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(err) = palette::show(&app) {
            log_line(&app, &format!("palette: could not open ({err})"));
        }
    });
}

/// Point the launcher hotkey at whatever the settings say, and say so in the log.
pub(crate) fn apply_palette_shortcut(app: &AppHandle) {
    let accelerator = load_settings(app).palette_shortcut;
    match palette::register_shortcut(app, &accelerator) {
        Ok(()) => log_line(app, &format!("palette: {accelerator} opens the launcher")),
        Err(err) => log_line(
            app,
            &format!("palette: {err} — the tray item still opens it"),
        ),
    }
}
