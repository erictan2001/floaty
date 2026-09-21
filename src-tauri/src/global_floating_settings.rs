//! `global_floating_settings` — moved out of lib.rs verbatim.
//!
//! Visibility is `pub(crate)` because the rest of the crate calls in across the module
//! boundary now; nothing here changed shape on the way out.

#![allow(unused_imports)]
use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

// ---------- global floating settings ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FloatSettings {
    #[serde(default = "default_pet_speed")]
    pub(crate) pet_speed: f64,
    #[serde(default = "default_gravity")]
    pub(crate) gravity: f64,
    #[serde(default = "default_bounce")]
    pub(crate) bounce: f64,
    #[serde(default = "default_single_click")]
    pub(crate) single_click: String,
    #[serde(default = "default_double_click")]
    pub(crate) double_click: String,
    /// live2d model library root picked by the user
    #[serde(default)]
    pub(crate) live2d_root: String,
    /// root directory where files and folders should be floaties
    #[serde(default)]
    pub(crate) files_root: String,
    /// plugin kinds whose windows stay closed
    #[serde(default)]
    pub(crate) disabled: Vec<String>,
    /// keep widgets visible on desktop when Show Desktop (Win+D) is triggered
    #[serde(default = "default_stay_on_desktop")]
    pub(crate) stay_on_desktop: bool,
    /// launch floaty when the user signs in (a Run entry in the registry)
    #[serde(default = "default_start_on_boot")]
    pub(crate) start_on_boot: bool,
    /// percentage of floaties that participate in animation (0 to 100)
    #[serde(default = "default_animated_ratio")]
    pub(crate) animated_ratio: f64,
    /// animation mode: "wave", "sync", "gentle", "static"
    #[serde(default = "default_animation_mode")]
    pub(crate) animation_mode: String,
    /// how far a resting icon rises, in px — the same travel in every mode.
    /// (Was multiplied by a second `floatiness` slider, which is folded in once
    /// by `migrate_float_settings`.)
    #[serde(default = "default_float_amplitude")]
    pub(crate) float_amplitude: f64,
    /// seconds per bob, in every mode
    #[serde(default = "default_float_period")]
    pub(crate) float_period: f64,
    /// wave only: how much of the cycle separates one icon from the next, as a
    /// percentage of one cycle
    #[serde(default = "default_float_spread")]
    pub(crate) float_spread: f64,
    /// ask before removing a floatie (file, folder or widget)
    #[serde(default = "default_confirm_remove")]
    pub(crate) confirm_remove: bool,
    /// visualizer sensitivity multiplier
    #[serde(default = "default_viz_gain")]
    pub(crate) viz_gain: f64,
    /// visualizer redraw rate (frames per second, while audio plays)
    #[serde(default = "default_viz_fps")]
    pub(crate) viz_fps: f64,
    /// The key that opens the launcher palette, e.g. "Ctrl+Alt+Space".
    #[serde(default = "default_palette_shortcut")]
    pub(crate) palette_shortcut: String,
    /// Get out of the way: hide the desktop while a fullscreen app is in front.
    #[serde(default = "default_true")]
    pub(crate) hide_in_fullscreen: bool,
    /// ...and stop animating when nobody has touched the machine for a while.
    #[serde(default = "default_true")]
    pub(crate) quiet_when_idle: bool,
    /// How long "a while" is, in minutes. 0 means never.
    #[serde(default = "default_idle_minutes")]
    pub(crate) idle_minutes: u32,
    /// Plugin id -> the fingerprint of the folder the user approved, for plugins
    /// they installed themselves. A plugin whose files have changed since is asked
    /// about again: approving code is not approving whatever replaces it later.
    #[serde(default)]
    pub(crate) plugin_trust: std::collections::HashMap<String, String>,
    /// system monitor sample period (milliseconds)
    #[serde(default = "default_sysmon_interval")]
    pub(crate) sysmon_interval: f64,
    /// version of the icon resolution pipeline the stored icons came from.
    /// Bumping `ICON_PIPELINE` re-resolves every stored icon once, which is how
    /// icons produced by an older (worse) pipeline get replaced.
    #[serde(default = "default_icon_pipeline")]
    pub(crate) icon_pipeline: u32,
}

/// Bump when the icon resolver changes what it produces, so already-stored
/// icons are refreshed once. v2: shell item image + alpha-preserving PNG.
/// v3: pick the route whose artwork actually fills the frame.
pub(crate) const ICON_PIPELINE: u32 = 4;

/// The launcher key. Ctrl+Alt+Space is free on a stock Windows, and unlike
/// Alt+Space (the window menu) or Ctrl+Space (the IME switcher on a CJK install)
/// it is not spoken for.
pub(crate) fn default_palette_shortcut() -> String {
    "Ctrl+Alt+Space".to_string()
}

pub(crate) fn default_true() -> bool {
    true
}

/// Ten minutes of no input at all — not ten minutes of the *app* being idle, which is a
/// different thing and would flatten the desktop while somebody reads a web page.
pub(crate) fn default_idle_minutes() -> u32 {
    10
}

pub(crate) fn default_icon_pipeline() -> u32 {
    // Old settings files predate the field; 0 means "resolve everything once".
    0
}

pub(crate) fn default_stay_on_desktop() -> bool {
    true
}

pub(crate) fn default_start_on_boot() -> bool {
    false
}

pub(crate) fn default_float_amplitude() -> f64 {
    6.0
}

pub(crate) fn default_float_period() -> f64 {
    10.0
}

pub(crate) fn default_float_spread() -> f64 {
    10.0
}

pub(crate) fn default_confirm_remove() -> bool {
    true
}

pub(crate) fn default_viz_gain() -> f64 {
    1.0
}

pub(crate) fn default_viz_fps() -> f64 {
    30.0
}

pub(crate) fn default_sysmon_interval() -> f64 {
    1000.0
}

pub(crate) fn default_animated_ratio() -> f64 {
    100.0
}

pub(crate) fn default_animation_mode() -> String {
    "wave".to_string()
}

pub(crate) fn default_pet_speed() -> f64 {
    1.0
}
pub(crate) fn default_gravity() -> f64 {
    2600.0
}
pub(crate) fn default_bounce() -> f64 {
    0.45
}
pub(crate) fn default_single_click() -> String {
    "drop".to_string()
}
pub(crate) fn default_double_click() -> String {
    "launch".to_string()
}

pub(crate) fn settings_file(app: &AppHandle) -> std::path::PathBuf {
    app_data_dir(app).join("floaty-settings.json")
}

/// Write the settings the way the settings pane does — pretty JSON, written
/// atomically — for the few commands that change them on their own.
pub(crate) fn write_settings(app: &AppHandle, settings: &FloatSettings) {
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        write_text_atomic(&settings_file(app), &json);
    }
}

/// Fold the settings a file written before the float sliders were made honest.
///
/// `floatiness` multiplied `float_amplitude`, so the two rows moved the same
/// thing and neither number was the travel; and `float_spread` was a percentage
/// of a stagger that never reached the tiles (the delay lost the cascade), on a
/// 0-200 scale whose 100 meant "15% of a cycle between neighbours". An existing
/// look therefore has to survive both changes: the multiplier is folded into
/// the height once, and the spread moves onto its new scale — percent of one
/// cycle between neighbouring icons.
///
/// Both results are rounded to the step of the slider that will show them (0.5px
/// and 1%), because a range input snaps to its own grid: a file holding 12.4
/// would otherwise draw its handle at 12 and write 12 the moment it was touched.
///
/// The presence of `floatiness` is what marks a file as old, and it is dropped
/// so this runs exactly once.
pub(crate) fn migrate_float_settings(value: &mut serde_json::Value) {
    let Some(map) = value.as_object_mut() else {
        return;
    };
    let Some(legacy) = map.remove("floatiness").and_then(|v| v.as_f64()) else {
        return;
    };
    if legacy.is_finite() && legacy > 0.0 {
        if let Some(amp) = map.get("float_amplitude").and_then(|v| v.as_f64()) {
            let folded = ((amp * legacy).clamp(0.0, 16.0) * 2.0).round() / 2.0;
            map.insert("float_amplitude".into(), serde_json::json!(folded));
        }
    }
    if let Some(spread) = map.get("float_spread").and_then(|v| v.as_f64()) {
        let rescaled = (spread * 0.15).clamp(0.0, 100.0).round();
        map.insert("float_spread".into(), serde_json::json!(rescaled));
    }
}

// ---------------------------------------------------------------------------------------
// Files arriving from outside: a drop from Explorer, or a paste.
// ---------------------------------------------------------------------------------------

/// The folder floatie under a point in record space, if any — the same 12px inflation
/// `floaty_dropped` uses, because a drop and the merge preview must agree.
pub(crate) fn folder_under(app: &AppHandle, x: f64, y: f64) -> Option<(String, std::path::PathBuf)> {
    let state = app.state::<AppState>();
    let guard = state.0.lock().ok()?;
    let mut hit: Option<(String, std::path::PathBuf)> = None;
    for (id, rec) in guard.widgets.iter() {
        if rec.kind != "folder" {
            continue;
        }
        let (w, h) = plugins::size(&rec.kind, &rec.data);
        let tile = exchange::Hit {
            x: rec.x as f64,
            y: rec.y as f64,
            w,
            h,
        };
        if !exchange::hits(&tile, x, y) {
            continue;
        }
        let Some(path) = rec.data.get("path").and_then(|v| v.as_str()) else {
            continue;
        };
        let dir = std::path::PathBuf::from(path);
        if dir.is_dir() {
            // serde keeps the map sorted, so two overlapping folders resolve the same way
            // here as they do in the drop rule
            hit = Some((id.clone(), dir));
            break;
        }
    }
    hit.map(|(_, dir)| (String::new(), dir))
}

/// Files dropped onto the desktop from outside floaty — Explorer, another app, or a paste.
///
/// Where they land is where they were dropped: on a folder floatie means *into* that
/// folder, anywhere else means into the folder the desktop is showing. The same volume
/// moves and another volume copies, and a program from Program Files becomes a shortcut
/// rather than leaving its install directory — `place_item_in_dir` already decides all of
/// that for the two-floatie merge, so it decides it here too.
///
/// A path that is *already* on the desktop is not copied anywhere: it moves to where it
/// was dropped, which is what a cut-and-paste of your own file should do.
#[tauri::command]
pub(crate) fn floaty_drop_paths(paths: Vec<String>, x: i32, y: i32, app: AppHandle) -> Result<String, String> {
    let root = files_root_dir(&app).ok_or("no folder is being watched")?;
    let incoming: Vec<std::path::PathBuf> = paths
        .iter()
        .map(std::path::PathBuf::from)
        .filter(|p| p.exists())
        .collect();
    if incoming.is_empty() {
        return Err("nothing that still exists was dropped".into());
    }
    let dest_dir = folder_under(&app, x as f64, y as f64)
        .map(|(_, dir)| dir)
        .unwrap_or_else(|| root.clone());
    let into_a_folder = dest_dir != root;
    let mut landed: Vec<String> = Vec::new();
    let mut refused: Vec<String> = Vec::new();
    for path in &incoming {
        let inside = path.starts_with(&root);
        if inside && !into_a_folder {
            // already here: this drop is a move, not an import
            landed.push(path.to_string_lossy().into_owned());
            continue;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Where a drop lands is the *desktop's* rule, not the merge's: the merge is about
        // two things the user already has, where a shortcut is the conservative answer —
        // dropping a file has to bring the file, the way Explorer would.
        let how = exchange::arrival(path, inside, &dest_dir);
        match how {
            exchange::Arrival::Move | exchange::Arrival::Copy => {
                let dest = exchange::unique_destination(&dest_dir, &name);
                // a folder across volumes is not copied here (a recursive copy is not a
                // drop's job) — the failure is reported instead of a half-copied directory
                let copying = how == exchange::Arrival::Copy;
                let result = if copying && !path.is_dir() {
                    std::fs::copy(path, &dest).map(|_| ())
                } else if copying {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Unsupported,
                        "a folder on another drive is left where it is",
                    ))
                } else {
                    std::fs::rename(path, &dest)
                };
                match result {
                    Ok(()) => {
                        log_line(
                            &app,
                            &format!(
                                "{}: {} -> {}",
                                if copying {
                                    "copied onto the desktop"
                                } else {
                                    "moved onto the desktop"
                                },
                                path.display(),
                                dest.display()
                            ),
                        );
                        landed.push(dest.to_string_lossy().into_owned());
                    }
                    Err(why) => refused.push(format!("{}: {why}", path.display())),
                }
            }
            exchange::Arrival::Shortcut => {
                let item = FolderItem {
                    name,
                    target: path.to_string_lossy().into_owned(),
                    icon: String::new(),
                    is_dir: path.is_dir(),
                };
                match place_item_in_dir(&dest_dir, &item, Some(&root)) {
                    Ok((placed, log)) => {
                        log_line(&app, &log);
                        landed.push(placed.target);
                    }
                    Err(why) => refused.push(why),
                }
            }
        }
    }
    if landed.is_empty() {
        return Err(refused.join("; "));
    }
    // The watcher's own pass: whatever arrived gets a record, the same way a file dropped
    // into the root from Explorer always has.
    let _ = sync_root(None, app.clone(), true);
    // ...and then the records are placed where the pointer was, cascading so that five
    // files dropped together are not five floaties under one another.
    let placed = place_arrived(&app, &landed, x, y);
    let what = if into_a_folder {
        format!("into {}", dest_dir.display())
    } else {
        "onto the desktop".to_string()
    };
    let mut report = format!("{} item(s) {what}", landed.len());
    if placed > 0 {
        report.push_str(&format!(", {placed} placed at the drop"));
    }
    if !refused.is_empty() {
        report.push_str(&format!(" — refused: {}", refused.join("; ")));
    }
    Ok(report)
}

/// Put the records for these paths where the drop happened, spread out, and tell the
/// pages. Returns how many were placed.
pub(crate) fn place_arrived(app: &AppHandle, landed: &[String], x: i32, y: i32) -> usize {
    let list = screens(app);
    let mut placed = 0usize;
    let mut moved: Vec<WidgetRecord> = Vec::new();
    {
        let state = app.state::<AppState>();
        let Ok(mut guard) = state.0.lock() else {
            return 0;
        };
        for (index, target) in landed.iter().enumerate() {
            // A file record keeps its path in `target` and a folder in `path`; either way
            // this is the record for the file that just arrived.
            let Some(id) = guard
                .widgets
                .iter()
                .find(|(_, r)| {
                    ["path", "target"].iter().any(|key| {
                        r.data
                            .get(*key)
                            .and_then(|v| v.as_str())
                            .is_some_and(|p| p.eq_ignore_ascii_case(target))
                    })
                })
                .map(|(id, _)| id.clone())
            else {
                continue;
            };
            let step = index as i32 * 26;
            let (mut nx, mut ny) = (x + step, y + step);
            // a drop near an edge must not push the floatie off the screen
            let (w, h) = guard
                .widgets
                .get(&id)
                .map(|r| plugins::size(&r.kind, &r.data))
                .unwrap_or((92.0, 112.0));
            if let Some(screen) = screens::screen_of(&list, nx as f64, ny as f64) {
                let area = &list[screen].logical;
                // clamp, with the upper bound never below the lower: a floatie wider than
                // the screen would otherwise panic here rather than sit at its corner
                let max_x = (area.x + area.w - w).max(area.x);
                let max_y = (area.y + area.h - h).max(area.y);
                nx = (nx as f64).clamp(area.x, max_x) as i32;
                ny = (ny as f64).clamp(area.y, max_y) as i32;
            }
            if let Some(rec) = guard.widgets.get_mut(&id) {
                rec.x = nx;
                rec.y = ny;
                moved.push(rec.clone());
                placed += 1;
            }
        }
    }
    for rec in moved {
        let _ = app.emit("floaty-widget-updated", rec);
    }
    if placed > 0 {
        persist(app);
    }
    placed
}

/// Paste onto the desktop: the clipboard's files, or a path copied as text.
#[tauri::command]
pub(crate) fn floaty_clipboard_paste(x: i32, y: i32, app: AppHandle) -> Result<String, String> {
    let mut paths: Vec<std::path::PathBuf> = exchange::clipboard_files();
    let from = if paths.is_empty() {
        match exchange::clipboard_text() {
            Some(text) => {
                paths = exchange::paths_from_text(&text);
                "text"
            }
            None => "",
        }
    } else {
        "files"
    };
    if paths.is_empty() {
        return Err("the clipboard has no files (or a path) on it".into());
    }
    let report = floaty_drop_paths(
        paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        x,
        y,
        app.clone(),
    )?;
    log_line(&app, &format!("paste: {report} (clipboard held {from})"));
    Ok(report)
}

/// Put these floaties' files on the clipboard, so Explorer and every other app can paste
/// them. `cut` stages the same list for floaty's own next paste, which *moves* them.
#[tauri::command]
pub(crate) fn floaty_clipboard_copy(ids: Vec<String>, cut: bool, app: AppHandle) -> Result<usize, String> {
    let paths: Vec<std::path::PathBuf> = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().map_err(|_| "busy".to_string())?;
        ids.iter()
            .filter_map(|id| guard.widgets.get(id))
            // `target` for a file, `path` for a folder — the same two keys the drop
            // placement reads, because a widget's file lives wherever its kind says it does
            .filter_map(|rec| {
                ["target", "path"]
                    .iter()
                    .find_map(|key| rec.data.get(*key).and_then(|v| v.as_str()))
            })
            .map(std::path::PathBuf::from)
            .collect()
    };
    if paths.is_empty() {
        return Err("nothing to copy".into());
    }
    exchange::set_clipboard_files(&paths)?;
    CUT.store(if cut {
        paths.clone()
    } else {
        Vec::new()
    });
    log_line(
        &app,
        &format!(
            "clipboard: {} item(s) {}",
            paths.len(),
            if cut { "cut" } else { "copied" }
        ),
    );
    Ok(paths.len())
}

/// The key that pastes the clipboard onto the desktop, under the pointer.
///
/// Global rather than a page shortcut for the same reason the launcher has one: the
/// desktop window is deliberately no-activate, so it can never receive a keystroke.
pub(crate) const PASTE_ACCELERATOR: &str = "Ctrl+Alt+V";

/// What an update check found, in the terms the settings row can show.
#[derive(serde::Serialize)]
pub(crate) struct UpdateReport {
    pub(crate) available: bool,
    pub(crate) current: String,
    pub(crate) version: Option<String>,
    pub(crate) notes: Option<String>,
    /// Why the check could not be made — a published app that cannot reach GitHub should
    /// say so rather than looking up to date.
    pub(crate) error: Option<String>,
}

/// Ask the release feed whether there is a newer floaty.
///
/// Rust rather than the updater's JS API on purpose: the check and the install are not
/// things the *page* should be trusted with, and keeping them here means no new webview
/// permission and no new global key.
#[tauri::command]
pub(crate) async fn floaty_check_update(app: AppHandle) -> UpdateReport {
    use tauri_plugin_updater::UpdaterExt;
    let current = app.package_info().version.to_string();
    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(e) => {
            return UpdateReport {
                available: false,
                current,
                version: None,
                notes: None,
                error: Some(format!("no update feed is configured ({e})")),
            }
        }
    };
    match updater.check().await {
        Ok(Some(update)) => UpdateReport {
            available: true,
            current,
            version: Some(update.version.clone()),
            notes: update.body.clone(),
            error: None,
        },
        Ok(None) => UpdateReport {
            available: false,
            current,
            version: None,
            notes: None,
            error: None,
        },
        Err(e) => UpdateReport {
            available: false,
            current,
            version: None,
            notes: None,
            // Offline, a corporate proxy, a manifest that does not parse: all of it is
            // "could not check", none of it is "you are up to date".
            error: Some(e.to_string()),
        },
    }
}

/// Download and install the update the check found, then restart into it.
#[tauri::command]
pub(crate) async fn floaty_install_update(app: AppHandle) -> Result<String, String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "there is no newer version".to_string())?;
    let version = update.version.clone();
    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    let _ = app.emit("floaty-update-installed", serde_json::json!({ "version": version }));
    log_line(&app, &format!("update: {version} installed, restarting"));
    app.restart();
}

/// The key that undoes the last change from anywhere.
///
/// Not a bare Ctrl+Z, and that is a trade worth naming: a *global* Ctrl+Z would take undo
/// away from every other application on the machine, which is not a price a desktop
/// decoration gets to charge. The desktop window is no-activate by design — it must never
/// take focus from what you are doing — so it cannot see a keystroke of its own; the
/// page's own Ctrl+Z handler only ever fires if focus lands there anyway. Hence a modified
/// global key, beside the launcher's and the paste key's.
pub(crate) const UNDO_ACCELERATOR: &str = "Ctrl+Alt+Z";

/// The key that replays an undone change. Same reasoning as the undo key, same modifier
/// family: a global shortcut has to be one no other application expects to keep.
pub(crate) const REDO_ACCELERATOR: &str = "Ctrl+Shift+Z";

/// Undo the last change, driven by the shortcut rather than a page.
pub(crate) fn undo_from_shortcut(app: &AppHandle) {
    match tauri::async_runtime::block_on(floaty_undo(app.clone())) {
        Ok(report) => log_line(
            app,
            &format!(
                "undo: '{}' ({} left to undo)",
                report.label, report.remaining
            ),
        ),
        Err(why) => log_line(app, &format!("undo: {why}")),
    }
}

/// Redo the last undone change, driven by the shortcut rather than a page.
pub(crate) fn redo_from_shortcut(app: &AppHandle) {
    match tauri::async_runtime::block_on(floaty_redo(app.clone())) {
        Ok(report) => log_line(
            app,
            &format!(
                "redo: '{}' ({} left to redo)",
                report.label, report.remaining
            ),
        ),
        Err(why) => log_line(app, &format!("redo: {why}")),
    }
}

/// Paste the clipboard onto the desktop where the pointer is.
pub(crate) fn paste_at_cursor(app: &AppHandle) -> Result<String, String> {
    let (px, py) = cursor_physical(app).ok_or("the pointer's place could not be read")?;
    // The numbers go in the message: this is the one place a scaled cursor reading turns
    // into a place on a screen, and when it goes wrong that is the only thing worth seeing.
    let (x, y) = virtual_at_physical(app, px, py)
        .ok_or_else(|| format!("no screen to paste onto (the pointer reads as {px},{py})"))?;
    floaty_clipboard_paste(x.round() as i32, y.round() as i32, app.clone())
}

/// The floaties that were cut, held here rather than in the clipboard: a cut is a promise
/// about *our* next paste, and pretending otherwise by rewriting the clipboard's drop
/// effect is how a paste into another app moves a file the user only meant to copy.
pub(crate) static CUT: std::sync::Mutex<Vec<std::path::PathBuf>> = std::sync::Mutex::new(Vec::new());

pub(crate) trait CutStore {
    fn store(&self, paths: Vec<std::path::PathBuf>);
}

impl CutStore for std::sync::Mutex<Vec<std::path::PathBuf>> {
    fn store(&self, paths: Vec<std::path::PathBuf>) {
        if let Ok(mut guard) = self.lock() {
            *guard = paths;
        }
    }
}

/// Where the pointer is, in physical px.
///
/// `cursor_position()` answers in the *primary monitor's* logical units — measured on a
/// 2x/1.5x pair, a pointer physically at 4000,667 came back as 2000,333 — and everything
/// else here (window rects, monitor rects, record positions) is physical. Scaling it back
/// up is what makes a hotkey paste land under the pointer on either screen.
pub(crate) fn cursor_physical(app: &AppHandle) -> Option<(f64, f64)> {
    let cursor = app.cursor_position().ok()?;
    let list = screens(app);
    let on_a_screen = |x: f64, y: f64| {
        list.iter().any(|s| {
            x >= s.physical.x
                && x < s.physical.x + s.physical.w
                && y >= s.physical.y
                && y < s.physical.y + s.physical.h
        })
    };
    // `cursor_position()` has been measured in two spaces on this machine: physical, and
    // the *primary monitor's* logical units (a pointer physically at 4000,667 read as
    // 2000,333 when the reading came through a virtualised path). Rather than assume
    // which one this is, take the reading that actually lands on a screen — with two
    // monitors at different scales the arrangement decides, and either answer is right.
    if on_a_screen(cursor.x, cursor.y) {
        return Some((cursor.x, cursor.y));
    }
    let primary = list.iter().find(|s| s.primary)?;
    Some((cursor.x * primary.scale, cursor.y * primary.scale))
}

/// The record-space place a physical point is, for a hotkey with no page behind it.
pub(crate) fn virtual_at_physical(app: &AppHandle, px: f64, py: f64) -> Option<(f64, f64)> {
    let list = screens(app);
    let index = list
        .iter()
        .position(|s| {
            px >= s.physical.x
                && px < s.physical.x + s.physical.w
                && py >= s.physical.y
                && py < s.physical.y + s.physical.h
        })
        // A pointer the platform will not place (a scaled reading, a cursor parked off the
        // arrangement) still has to paste *somewhere*: the screen it is nearest to in y,
        // which is where a person would look for it.
        .or_else(|| {
            list.iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    let da = (py - (a.physical.y + a.physical.h / 2.0)).abs();
                    let db = (py - (b.physical.y + b.physical.h / 2.0)).abs();
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| i)
        })?;
    let screen = &list[index];
    Some((
        screen.logical.x + (px - screen.physical.x) / screen.scale,
        screen.logical.y + (py - screen.physical.y) / screen.scale,
    ))
}

// ---------------------------------------------------------------------------------------
// Presence: getting out of the way of a fullscreen app, and going quiet when idle.
// ---------------------------------------------------------------------------------------

/// The state as last applied, one entry per desktop-layer window. The watcher decides every
/// couple of seconds; only a *change* does anything, so an already-quiet desktop is not
/// re-flattened every poll — and a change on one screen does not touch the others.
pub(crate) static PRESENCE: std::sync::Mutex<Option<Vec<(String, presence::State)>>> =
    std::sync::Mutex::new(None);

/// The rules in force, kept here so the watcher does not read the settings file 30 times
/// a minute. Written at startup and whenever the settings are saved.
pub(crate) static PRESENCE_RULES: std::sync::Mutex<Option<presence::Rules>> = std::sync::Mutex::new(None);

pub(crate) fn set_presence_rules(rules: presence::Rules) {
    if let Ok(mut guard) = PRESENCE_RULES.lock() {
        *guard = Some(rules);
    }
}

pub(crate) fn presence_rules() -> presence::Rules {
    PRESENCE_RULES
        .lock()
        .ok()
        .and_then(|guard| *guard)
        .unwrap_or_default()
}

pub(crate) fn rules_from(settings: &FloatSettings) -> presence::Rules {
    presence::Rules {
        hide_for_fullscreen: settings.hide_in_fullscreen,
        quiet_when_idle: settings.quiet_when_idle,
        idle_after: std::time::Duration::from_secs(settings.idle_minutes.min(600) as u64 * 60),
    }
}

/// Every window that draws the desktop — one per screen, plus the per-screen top layers.
pub(crate) fn desktop_layer_windows(app: &AppHandle) -> Vec<tauri::WebviewWindow> {
    app.webview_windows()
        .into_iter()
        .filter(|(label, _)| is_desktop_layer_label(label))
        .map(|(_, window)| window)
        .collect()
}

/// One desktop layer's answer, so a reader can see *which* screen went away. The state is
/// per screen because the windows are; a single `hidden` cannot describe this desktop.
#[derive(serde::Serialize)]
pub(crate) struct PresenceScreenReport {
    /// The desktop-layer window's label, which is also the screen it draws.
    pub(crate) label: String,
    /// The monitor's own name, when the label maps to one.
    pub(crate) screen: Option<String>,
    pub(crate) hidden: bool,
    pub(crate) quiet: bool,
    pub(crate) reason: Option<String>,
}

#[derive(serde::Serialize)]
pub(crate) struct PresenceReport {
    /// Hidden on *any* screen: what a one-line readout means by "the desktop is out of the
    /// way". `screens` says which.
    pub(crate) hidden: bool,
    /// Quiet on *every* screen — the shared audio capture stops only then.
    pub(crate) quiet: bool,
    pub(crate) reason: Option<String>,
    pub(crate) idle_ms: u64,
    /// The window in front, as the decision saw it — the field that explains a state
    /// nobody expected, so the diagnostics tab and a probe can both read it.
    pub(crate) foreground: Option<String>,
    pub(crate) foreground_shell: bool,
    pub(crate) foreground_ours: bool,
    /// One entry per desktop layer — the field that makes a per-screen rule legible.
    pub(crate) screens: Vec<PresenceScreenReport>,
    pub(crate) hide_in_fullscreen: bool,
    pub(crate) quiet_when_idle: bool,
    pub(crate) idle_minutes: u32,
}

pub(crate) fn presence_observations(app: &AppHandle) -> presence::Observations {
    let own: Vec<isize> = desktop_layer_windows(app)
        .iter()
        .filter_map(|w| w.hwnd().ok().map(|h| h.0 as isize))
        .collect();
    presence::observe(&own)
}

/// The screen a desktop-layer window draws, as the presence rules see it.
///
/// A per-screen overlay says so in its label (`desktop-overlay-2` is the second screen). A
/// `widget-*` window in per-widget mode does not, so it is placed by where it is. A window
/// whose screen cannot be worked out gets `None`, and the rule leaves it alone: hiding the
/// desktop on a guess is worse than leaving it up.
pub(crate) fn presence_screen_of(
    list: &[screens::Screen],
    window: &tauri::WebviewWindow,
) -> Option<presence::Rect> {
    let rect = |s: &screens::Screen| {
        presence::Rect::new(s.physical.x, s.physical.y, s.physical.w, s.physical.h)
    };
    if let Some(index) = overlay_index(window.label()) {
        return list.get(index).map(rect);
    }
    let at = window.outer_position().ok()?;
    let (x, y) = (at.x as f64, at.y as f64);
    list.iter()
        .find(|s| {
            x >= s.physical.x
                && y >= s.physical.y
                && x < s.physical.x + s.physical.w
                && y < s.physical.y + s.physical.h
        })
        .map(rect)
}

/// The decision for every desktop-layer window — one per screen — and the machine's answer.
///
/// Per screen, because the windows are: a fullscreen app in front of one screen takes that
/// screen's desktop away and leaves the others alone.
pub(crate) fn presence_states(
    app: &AppHandle,
    observed: &presence::Observations,
) -> (Vec<(String, presence::State)>, presence::State) {
    let rules = presence_rules();
    let list = screens(app);
    let states: Vec<(String, presence::State)> = desktop_layer_windows(app)
        .into_iter()
        .map(|window| {
            let screen = presence_screen_of(&list, &window);
            let state = presence::decide_for(
                observed.foreground.as_ref(),
                screen.as_ref(),
                &rules,
                observed.idle_ms,
            );
            (window.label().to_string(), state)
        })
        .collect();
    let summary = presence::summarise(&states.iter().map(|(_, s)| *s).collect::<Vec<_>>());
    (states, summary)
}

/// Put the desktop where the decision says it should be, and say so once.
pub(crate) fn apply_presence(
    app: &AppHandle,
    states: &[(String, presence::State)],
    summary: presence::State,
) {
    let previous = PRESENCE.lock().ok().and_then(|mut guard| guard.replace(states.to_vec()));
    if previous.as_deref() == Some(states) {
        return;
    }
    // Only the screens that changed are touched: a fullscreen app on one screen hides that
    // screen's overlay and leaves the other screens' windows exactly as they were.
    let changed: Vec<&(String, presence::State)> = states
        .iter()
        .filter(|(label, state)| {
            let before = previous
                .as_ref()
                .and_then(|prev| prev.iter().find(|(l, _)| l == label).map(|(_, s)| s));
            before != Some(state)
        })
        .collect();
    for (label, state) in changed {
        if let Some(window) = app.get_webview_window(label) {
            if state.hidden {
                let _ = window.hide();
            } else {
                let _ = window.show();
            }
            // To this window only, and with its own state: the desktop layers are separate
            // windows, and a page must not act on another screen's verdict. A broadcast
            // would let a hidden screen's visualizer start the audio again.
            let _ = app.emit_to(
                tauri::EventTarget::webview_window(label.as_str()),
                "floaty-presence",
                serde_json::json!({
                    "hidden": state.hidden,
                    "quiet": state.quiet,
                    "reason": state.reason,
                    "machine": { "hidden": summary.hidden, "quiet": summary.quiet },
                }),
            );
            match state.reason {
                Some(why) => log_line(
                    app,
                    &format!(
                        "presence: {label} {} ({why}) — desktop {}",
                        if state.hidden { "hidden" } else { "quiet" },
                        if state.hidden { "off screen" } else { "on screen, still" }
                    ),
                ),
                None => log_line(
                    app,
                    &format!("presence: {label} back to normal input and no fullscreen app"),
                ),
            }
        }
    }
    if summary.quiet {
        // The loopback capture is the one thing that costs while nothing is happening;
        // the visualizer asks for it again when the machine wakes up (it listens for
        // the same event), so this is a pause rather than a switch that stays off. It
        // stops only when *every* screen is quiet — one screen left is enough to want it.
        audio::stop();
    }
}

/// Watch the machine, from a thread of its own: the poll is two Win32 calls and a
/// comparison, so it is cheap enough to keep running, and cheap enough to leave alone
/// when it decides nothing has changed.
pub(crate) fn start_presence_watch(app: AppHandle) {
    std::thread::spawn(move || loop {
        let observed = presence_observations(&app);
        let (states, summary) = presence_states(&app, &observed);
        apply_presence(&app, &states, summary);
        std::thread::sleep(std::time::Duration::from_millis(2000));
    });
}

/// The presence state, for the diagnostics tab and for probes.
#[tauri::command]
pub(crate) fn floaty_presence(app: AppHandle) -> PresenceReport {
    let settings = load_settings(&app);
    let observed = presence_observations(&app);
    let (states, summary) = presence_states(&app, &observed);
    let list = screens(&app);
    let fg = observed.foreground.as_ref();
    PresenceReport {
        hidden: states.iter().any(|(_, s)| s.hidden),
        quiet: summary.quiet,
        reason: states.iter().find_map(|(_, s)| s.reason).map(|r| r.to_string()),
        idle_ms: observed.idle_ms,
        foreground: fg.map(|f| {
            format!(
                "{}x{} at {},{}",
                f.rect.w.round(),
                f.rect.h.round(),
                f.rect.x.round(),
                f.rect.y.round()
            )
        }),
        foreground_shell: fg.is_some_and(|f| f.shell),
        foreground_ours: fg.is_some_and(|f| f.ours),
        screens: states
            .iter()
            .map(|(label, state)| PresenceScreenReport {
                label: label.clone(),
                screen: overlay_index(label)
                    .and_then(|index| list.get(index))
                    .map(|s| s.key.clone()),
                hidden: state.hidden,
                quiet: state.quiet,
                reason: state.reason.map(|r| r.to_string()),
            })
            .collect(),
        hide_in_fullscreen: settings.hide_in_fullscreen,
        quiet_when_idle: settings.quiet_when_idle,
        idle_minutes: settings.idle_minutes,
    }
}

pub(crate) fn load_settings(app: &AppHandle) -> FloatSettings {
    let path = settings_file(app);
    // Read through `Value` first so an old file can be migrated; anything that
    // is not an object falls back to the plain typed read below.
    let parsed: Option<FloatSettings> = read_json_with_backup::<serde_json::Value>(&path)
        .map(|(mut value, _)| {
            migrate_float_settings(&mut value);
            value
        })
        .and_then(|value| serde_json::from_value::<FloatSettings>(value).ok())
        .or_else(|| read_json_with_backup::<FloatSettings>(&path).map(|(s, _)| s));
    match parsed {
        Some(s) => FloatSettings {
            single_click: if s.single_click.is_empty() {
                default_single_click()
            } else {
                s.single_click
            },
            double_click: if s.double_click.is_empty() {
                default_double_click()
            } else {
                s.double_click
            },
            animation_mode: if s.animation_mode.is_empty() {
                default_animation_mode()
            } else {
                s.animation_mode
            },
            palette_shortcut: if s.palette_shortcut.trim().is_empty() {
                default_palette_shortcut()
            } else {
                s.palette_shortcut
            },
            ..s
        },
        None => FloatSettings {
            pet_speed: default_pet_speed(),
            gravity: default_gravity(),
            bounce: default_bounce(),
            single_click: default_single_click(),
            double_click: default_double_click(),
            live2d_root: String::new(),
            files_root: String::new(),
            disabled: Vec::new(),
            stay_on_desktop: default_stay_on_desktop(),
            start_on_boot: default_start_on_boot(),
            animated_ratio: default_animated_ratio(),
            animation_mode: default_animation_mode(),
            float_amplitude: default_float_amplitude(),
            float_period: default_float_period(),
            float_spread: default_float_spread(),
            confirm_remove: default_confirm_remove(),
            viz_gain: default_viz_gain(),
            viz_fps: default_viz_fps(),
            sysmon_interval: default_sysmon_interval(),
            palette_shortcut: default_palette_shortcut(),
            hide_in_fullscreen: default_true(),
            quiet_when_idle: default_true(),
            idle_minutes: default_idle_minutes(),
            plugin_trust: std::collections::HashMap::new(),
            icon_pipeline: default_icon_pipeline(),
        },
    }
}

#[tauri::command]
pub(crate) async fn floaty_get_settings(app: AppHandle) -> FloatSettings {
    load_settings(&app)
}

#[tauri::command]
pub(crate) fn floaty_set_settings(settings: FloatSettings, app: AppHandle) -> FloatSettings {
    // icon_pipeline is the backend's own bookkeeping (it decides whether stored
    // icons need re-resolving); the settings UI does not know about it, so keep
    // whatever is already on disk instead of letting a save reset it to 0.
    let stored = load_settings(&app);
    let stored_pipeline = stored.icon_pipeline;
    let mut s = FloatSettings {
        pet_speed: settings.pet_speed.clamp(0.0, 3.0),
        gravity: settings.gravity.clamp(0.0, 8000.0),
        bounce: settings.bounce.clamp(0.0, 0.95),
        single_click: match settings.single_click.as_str() {
            "hop" | "nothing" => settings.single_click,
            _ => "drop".to_string(),
        },
        double_click: match settings.double_click.as_str() {
            "drop" | "nothing" => settings.double_click,
            _ => "launch".to_string(),
        },
        live2d_root: settings.live2d_root,
        files_root: settings.files_root,
        disabled: settings.disabled,
        stay_on_desktop: settings.stay_on_desktop,
        start_on_boot: settings.start_on_boot,
        animated_ratio: settings.animated_ratio.clamp(0.0, 100.0),
        animation_mode: match settings.animation_mode.as_str() {
            "sync" | "gentle" | "static" => settings.animation_mode,
            _ => "wave".to_string(),
        },
        float_amplitude: settings.float_amplitude.clamp(0.0, 16.0),
        float_period: settings.float_period.clamp(2.0, 60.0),
        float_spread: settings.float_spread.clamp(0.0, 100.0),
        confirm_remove: settings.confirm_remove,
        viz_gain: settings.viz_gain.clamp(0.1, 4.0),
        viz_fps: settings.viz_fps.clamp(5.0, 60.0),
        sysmon_interval: settings.sysmon_interval.clamp(250.0, 10_000.0),
        // A shortcut that does not parse is not saved: the alternative is a
        // launcher that can never be opened again, and the typo would be invisible.
        palette_shortcut: match palette::parse_shortcut(settings.palette_shortcut.trim()) {
            Some(accelerator) => accelerator,
            None => stored.palette_shortcut.clone(),
        },
        hide_in_fullscreen: settings.hide_in_fullscreen,
        quiet_when_idle: settings.quiet_when_idle,
        // Ten hours is not a threshold, it is a typo; zero means never.
        idle_minutes: settings.idle_minutes.min(600),
        // The approvals are the backend's own bookkeeping, like icon_pipeline: a
        // settings form that does not show them must not be able to clear them.
        plugin_trust: stored.plugin_trust.clone(),
        icon_pipeline: stored_pipeline.max(settings.icon_pipeline),
    };
    // Start on boot is a registry entry, not a preference: write it now and, if
    // that fails, put the old value back rather than save a checkbox that claims
    // something Windows does not agree with.
    if s.start_on_boot != stored.start_on_boot && !set_start_on_boot(&app, s.start_on_boot) {
        s.start_on_boot = stored.start_on_boot;
    }
    write_settings(&app, &s);
    #[cfg(windows)]
    {
        if let Some(win) = app.get_webview_window("desktop-overlay") {
            if let Ok(hwnd) = win.hwnd() {
                desktop_pin::apply_overlay_desktop_pin("desktop-overlay", hwnd.0 as isize, s.stay_on_desktop);
            }
        }
        if let Some(win) = app.get_webview_window("top-overlay") {
            // the always-on-top layer is never parented into the desktop
            if let Ok(hwnd) = win.hwnd() {
                desktop_pin::apply_overlay_desktop_pin("top-overlay", hwnd.0 as isize, false);
            }
        }
        for (label, win) in app.webview_windows() {
            // the overlays were handled above, and they are pinned through
            // `apply_overlay_desktop_pin` (which is also what registers their
            // click-through region)
            if label == "desktop-overlay" || label == "top-overlay" {
                continue;
            }
            if let Ok(hwnd) = win.hwnd() {
                if is_desktop_layer_label(&label) {
                    desktop_pin::apply_desktop_pin(hwnd.0 as isize, s.stay_on_desktop);
                } else if desktop_pin::restore_ordinary_window(hwnd.0 as isize) {
                    // settings, the manager: ordinary windows. A settings save is
                    // frequent enough to repair one that an earlier pass pinned,
                    // and saying so is what makes the repair visible in the log.
                    log_line(
                        &app,
                        &format!("settings: {label} handed back to the shell as an ordinary window"),
                    );
                }
            }
        }
    }
    app.emit("floaty-settings-changed", &s).ok();
    log_line(&app, "settings updated");

    // A new root is a new folder to follow: mirror it once and point the watcher
    // at it (the sync does that itself, including stopping the watcher when the
    // root was cleared), so the desktop shows the folder straight away rather
    // than at the next launch.
    if stored.files_root.trim() != s.files_root.trim() {
        let _ = floaty_sync_files(None, app.clone());
    }
    // A new launcher key is registered now, so the settings row can say whether
    // the key it just took is really held.
    if stored.palette_shortcut.trim() != s.palette_shortcut.trim() {
        apply_palette_shortcut(&app);
    }
    // The presence watcher reads its rules from memory, not from this file every two
    // seconds, so a change here is what it notices.
    set_presence_rules(rules_from(&s));
    s
}
