//! `desktop_actions` — a drop, a paste and a shortcut from outside the app.
//!
//! Files arriving from Explorer, the clipboard, the cut list, and the global keys
//! that act on them without the no-activate desktop ever taking focus.
//!
//! Moved out of `settings_sync.rs` verbatim.

use crate::*;
use tauri::{AppHandle, Emitter};

// ---------------------------------------------------------------------------------------
// Files arriving from outside: a drop from Explorer, or a paste.
// ---------------------------------------------------------------------------------------

/// The folder floatie under a point in record space, if any — the same 12px inflation
/// `floaty_dropped` uses, because a drop and the merge preview must agree.
pub(crate) fn folder_under(
    app: &AppHandle,
    x: f64,
    y: f64,
) -> Option<(String, std::path::PathBuf)> {
    store::with(app, |s| {
        let mut hit: Option<(String, std::path::PathBuf)> = None;
        for rec in s.iter() {
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
                hit = Some((rec.id.clone(), dir));
                break;
            }
        }
        hit.map(|(_, dir)| (String::new(), dir))
    })
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
pub(crate) fn floaty_drop_paths(
    paths: Vec<String>,
    x: i32,
    y: i32,
    app: AppHandle,
) -> Result<String, String> {
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
        let path_text = path.to_string_lossy();
        let inside = is_under_root(&root.to_string_lossy(), &path_text);
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
                let copying = how == exchange::Arrival::Copy;
                // one name in one place: a collision is numbered (2), (3) … whichever way it
                // arrives, and a folder across volumes is not copied here (a recursive copy is
                // not a drop's job) — the failure is reported instead of a half-copied directory
                let result: Result<std::path::PathBuf, String> = if copying {
                    let dest = placement::free_name(&dest_dir, &name);
                    if path.is_dir() {
                        Err("a folder on another drive is left where it is".into())
                    } else {
                        std::fs::copy(path, &dest)
                            .map(|_| dest)
                            .map_err(|e| e.to_string())
                    }
                } else {
                    placement::place_into(path, &dest_dir, None)
                };
                match result {
                    Ok(dest) => {
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
    let _ = sync_root(None, app.clone(), true, None);
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
    store::with(app, |s| {
        for (index, target) in landed.iter().enumerate() {
            // A file record keeps its path in `target` and a folder in `path`; either way
            // this is the record for the file that just arrived.
            let Some(id) = s
                .iter()
                .find(|r| {
                    ["path", "target"].iter().any(|key| {
                        r.data
                            .get(*key)
                            .and_then(|v| v.as_str())
                            .is_some_and(|p| p.eq_ignore_ascii_case(target))
                    })
                })
                .map(|r| r.id.clone())
            else {
                continue;
            };
            let step = index as i32 * 26;
            let (mut nx, mut ny) = (x + step, y + step);
            // a drop near an edge must not push the floatie off the screen
            let (w, h) = s
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
            if s.edit(&id, |rec| {
                rec.x = nx;
                rec.y = ny;
                moved.push(rec.clone());
            })
            .is_some()
            {
                placed += 1;
            }
        }
    });
    for rec in moved {
        let _ = app.emit("floaty-widget-updated", rec);
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
pub(crate) fn floaty_clipboard_copy(
    ids: Vec<String>,
    cut: bool,
    app: AppHandle,
) -> Result<usize, String> {
    let paths: Vec<std::path::PathBuf> = store::with(&app, |s| {
        ids.iter()
            .filter_map(|id| s.get(id))
            // `target` for a file, `path` for a folder — the same two keys the drop
            // placement reads, because a widget's file lives wherever its kind says it does
            .filter_map(|rec| {
                ["target", "path"]
                    .iter()
                    .find_map(|key| rec.data.get(*key).and_then(|v| v.as_str()))
            })
            .map(std::path::PathBuf::from)
            .collect()
    });
    if paths.is_empty() {
        return Err("nothing to copy".into());
    }
    exchange::set_clipboard_files(&paths)?;
    CUT.store(if cut { paths.clone() } else { Vec::new() });
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
pub(crate) static CUT: std::sync::Mutex<Vec<std::path::PathBuf>> =
    std::sync::Mutex::new(Vec::new());

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
