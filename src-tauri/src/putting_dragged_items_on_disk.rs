//! `putting_dragged_items_on_disk` — moved out of lib.rs verbatim.
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

// ---------- putting dragged items on disk ----------

/// The name Windows itself gives a folder made by hand: the shell takes the
/// first free one, so a second folder is "New folder (2)".
pub(crate) const NEW_FOLDER_BASE: &str = "New folder";

/// The next free "New folder (n)" in `dir`, without creating it.
pub(crate) fn new_folder_path(dir: &std::path::Path) -> std::path::PathBuf {
    let first = dir.join(NEW_FOLDER_BASE);
    if !first.exists() {
        return first;
    }
    for i in 2..1000 {
        let candidate = dir.join(format!("{NEW_FOLDER_BASE} ({i})"));
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

/// Create the next free "New folder (n)". The create is what reserves the name:
/// a name Explorer (or a second drag) takes in between is retried rather than
/// written over.
pub(crate) fn create_new_folder(dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    for _ in 0..64 {
        let candidate = new_folder_path(dir);
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("could not create a new folder".into())
}

/// The pointed root: the directory the floaties mirror. `None` when nothing is
/// pointed at (or the pointer went stale), which is the callers' cue to keep
/// their no-directory behaviour instead of inventing a location.
pub(crate) fn files_root_dir(app: &AppHandle) -> Option<std::path::PathBuf> {
    let root = load_settings(app).files_root;
    let p = std::path::PathBuf::from(root.trim());
    if p.is_dir() {
        Some(p)
    } else {
        None
    }
}

/// A dragged item may only be *moved* into a folder when the root owns it.
/// Anything from outside — an app icon pointing into Program Files or at a Start
/// Menu shortcut — is carried in as a shortcut file instead: a plain move there
/// would take the program out of its install directory.
pub(crate) fn can_move_into_folder(root: Option<&std::path::Path>, src: &std::path::Path) -> bool {
    match root {
        Some(r) => src.starts_with(r),
        None => false,
    }
}

/// A usable file name for the shortcut built from an item's label.
pub(crate) fn shortcut_file_name(name: &str) -> String {
    let stem: String = name
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        })
        .collect();
    let stem = stem.trim().trim_end_matches('.').trim();
    let stem = if stem.is_empty() { "Shortcut" } else { stem };
    if stem.to_ascii_lowercase().ends_with(".lnk") {
        stem.to_string()
    } else {
        format!("{stem}.lnk")
    }
}

/// Whether a path already *is* a shortcut, in which case the file can simply be
/// copied: that keeps its arguments, working directory and icon exactly.
pub(crate) fn is_lnk_path(p: &std::path::Path) -> bool {
    p.extension().map(|e| e.eq_ignore_ascii_case("lnk")).unwrap_or(false)
}

/// Write a Windows shortcut with the shell's own COM interface — the only
/// supported way to author a .lnk; hand-rolled byte layouts come back broken.
pub(crate) fn create_lnk(lnk: &std::path::Path, target: &std::path::Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const NO_WINDOW: u32 = 0x08000000;
        // single-quoted PowerShell literals; an inner quote doubles
        let q = |s: String| s.replace('\'', "''");
        let script = format!(
            "$sh = New-Object -ComObject WScript.Shell; $sc = $sh.CreateShortcut('{}'); $sc.TargetPath = '{}'; $sc.Save()",
            q(lnk.to_string_lossy().to_string()),
            q(target.to_string_lossy().to_string()),
        );
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(NO_WINDOW)
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        if !lnk.exists() {
            return Err("the shortcut was not created".into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (lnk, target);
        Err("shortcuts are windows-only".into())
    }
}

/// Put a dragged item inside `dest_dir` on disk: the file itself when the root
/// owns it, otherwise a shortcut file. Returns the item rewritten to its new
/// home plus the line to log, or the reason it stayed put.
///
/// No app handle: the disk effect is the whole contract, and this is what the
/// tests drive.
pub(crate) fn place_item_in_dir(
    dest_dir: &std::path::Path,
    item: &FolderItem,
    root: Option<&std::path::Path>,
) -> Result<(FolderItem, String), String> {
    let src = std::path::PathBuf::from(&item.target);
    if !src.exists() {
        return Err(format!("{} is gone; left alone", src.display()));
    }
    let mut out = item.clone();

    if can_move_into_folder(root, &src) {
        let file_name = src
            .file_name()
            .ok_or_else(|| format!("{} has no file name", src.display()))?
            .to_os_string();
        let dest = unique_dest_path(dest_dir, &file_name);
        if dest != src {
            fs::rename(&src, &dest).map_err(|e| format!("move {} FAILED: {e}", src.display()))?;
        }
        out.target = dest.to_string_lossy().to_string();
        out.name = dest
            .file_name()
            .unwrap_or(&file_name)
            .to_string_lossy()
            .to_string();
        out.is_dir = dest.is_dir();
        return Ok((
            out,
            format!("moved into folder: {} -> {}", src.display(), dest.display()),
        ));
    }

    // outside the root: the original stays where it is, a shortcut moves in
    let file_name = if is_lnk_path(&src) {
        src.file_name()
            .ok_or_else(|| format!("{} has no file name", src.display()))?
            .to_os_string()
    } else {
        std::ffi::OsString::from(shortcut_file_name(&item.name))
    };
    let dest = unique_dest_path(dest_dir, &file_name);
    let made: Result<(), String> = if is_lnk_path(&src) {
        fs::copy(&src, &dest).map(|_| ()).map_err(|e| e.to_string())
    } else {
        create_lnk(&dest, &src)
    };
    made.map_err(|e| format!("shortcut for {} FAILED: {e}", src.display()))?;
    out.target = dest.to_string_lossy().to_string();
    out.name = dest
        .file_name()
        .unwrap_or(&file_name)
        .to_string_lossy()
        .to_string();
    out.is_dir = false;
    Ok((
        out,
        format!("shortcut in folder: {} -> {}", src.display(), dest.display()),
    ))
}

/// Write the app's shortcut into the root: a copy when the app is already
/// floated through one (that keeps its arguments, working directory and icon),
/// otherwise a fresh .lnk pointing at the program. Returns the launcher's new
/// name and path plus the line to log.
pub(crate) fn shortcut_into_root(
    root: &std::path::Path,
    name: &str,
    path: &str,
) -> Result<(String, String, String), String> {
    let src = std::path::Path::new(path);
    let file_name = src
        .file_name()
        .ok_or_else(|| format!("{path} has no file name"))?;
    let file_name = if is_lnk_path(src) {
        file_name.to_os_string()
    } else {
        std::ffi::OsString::from(shortcut_file_name(name))
    };
    let dest = unique_dest_path(root, &file_name);
    let made: Result<(), String> = if is_lnk_path(src) {
        fs::copy(src, &dest).map(|_| ()).map_err(|e| e.to_string())
    } else {
        create_lnk(&dest, src)
    };
    made.map_err(|e| format!("app shortcut for {} FAILED: {e}", src.display()))?;
    let nm = dest
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string());
    let log = format!("app shortcut in root: {} -> {}", src.display(), dest.display());
    Ok((nm, dest.to_string_lossy().to_string(), log))
}

/// Materialise an app launcher as a shortcut file in the pointed root, so app
/// icons are files there like everything else (and grouping can move them).
/// Falls back to pointing straight at the app when there is no root.
pub(crate) fn app_into_root(app: &AppHandle, name: &str, path: &str) -> (String, String) {
    let unchanged = || (name.to_string(), path.to_string());
    let Some(root) = files_root_dir(app) else {
        return unchanged();
    };
    let src = std::path::Path::new(path);
    if src.starts_with(&root) || !src.exists() {
        return unchanged();
    }
    match shortcut_into_root(&root, name, path) {
        Ok((nm, dest, log)) => {
            log_line(app, &log);
            (nm, dest)
        }
        Err(msg) => {
            log_line(app, &msg);
            unchanged()
        }
    }
}

/// Keep the icons already resolved for the same paths. A rescan returns items
/// with empty icons, and dropping the stored ones (say, because they are only
/// 32px) is what made folder tiles flap between an icon and a blank tile on
/// every launch: most file types and every directory resolve to 32px.
pub(crate) fn carry_icons_by_target(old: &[FolderItem], items: &mut [FolderItem]) {
    for item in items.iter_mut() {
        if let Some(prev) = old.iter().find(|o| o.target == item.target) {
            if !prev.icon.is_empty() && prev.icon != "none" {
                item.icon = prev.icon.clone();
            }
        }
    }
}

/// Same, matched by name: for a rescan after the folder itself was renamed on
/// disk, where every target path moved but the items are the same files.
pub(crate) fn carry_icons_by_name(old: &[FolderItem], items: &mut [FolderItem]) {
    for item in items.iter_mut() {
        if let Some(prev) = old.iter().find(|o| o.name == item.name) {
            if !prev.icon.is_empty() && prev.icon != "none" {
                item.icon = prev.icon.clone();
            }
        }
    }
}

pub(crate) fn scan_folder_items(dir_path: &std::path::Path) -> Vec<FolderItem> {
    if !dir_path.is_dir() {
        return Vec::new();
    }
    let mut items = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();
            if file_name.starts_with('.')
                || file_name.eq_ignore_ascii_case("desktop.ini")
                || file_name.eq_ignore_ascii_case("thumbs.db")
            {
                continue;
            }
            let is_dir = path.is_dir();
            let target = path.to_string_lossy().to_string();
            let icon = String::new();
            items.push(FolderItem {
                name: file_name,
                target,
                icon,
                is_dir,
            });
        }
    }
    items.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            b.is_dir.cmp(&a.is_dir)
        } else {
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        }
    });
    items
}

/// Close a widget window off the command thread (same reason as creation).
pub(crate) fn close_widget_async(app: &AppHandle, id: &str) {
    if app.get_webview_window("desktop-overlay").is_some() {
        // the overlay draws it: tell the overlay to drop it
        app.emit("floaty-widget-removed", id).ok();
    }
    // A widget can have a window of its own *in overlay mode too* — a pinned one
    // does — and that window has to come down for real: leaving it up left a
    // second copy of the widget on the desktop (and the overlay was asked to
    // drop a widget it was not drawing).
    if app.get_webview_window(&widget_label(id)).is_none() {
        return;
    }
    let handle = app.clone();
    let owned = id.to_string();
    std::thread::spawn(move || {
        let h2 = handle.clone();
        let id2 = owned.clone();
        if let Err(e) = handle.run_on_main_thread(move || {
            if let Some(w) = h2.get_webview_window(&widget_label(&id2)) {
                if let Err(e) = w.close() {
                    log_line(&h2, &format!("close window FAILED for {id2}: {e}"));
                }
            }
        }) {
            log_line(&handle, &format!("main-thread dispatch FAILED for {owned}: {e}"));
        }
    });
}

pub(crate) fn launch_target(app: &AppHandle, target: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW avoids a console flash; `start` resolves exe/lnk/urls
        const NO_WINDOW: u32 = 0x08000000;
        const DETACHED: u32 = 0x00000008;
        std::process::Command::new("cmd")
            .args(["/C", "start", "", target])
            .creation_flags(NO_WINDOW | DETACHED)
            .spawn()
            .map_err(|e| {
                log_line(app, &format!("launch FAILED: {e}"));
                e.to_string()
            })?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn floaty_launch(id: String, app: AppHandle) -> Result<(), String> {
    let target = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        guard
            .widgets
            .get(&id)
            .filter(|r| is_path_kind(&r.kind))
            .and_then(|r| {
                r.data
                    .get("target")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .ok_or("launcher not found")?
    };
    if target.trim().is_empty() {
        return Err("launcher has no target".into());
    }
    log_line(&app, &format!("launch {id} -> {target}"));
    launch_target(&app, &target)
}

#[tauri::command]
pub(crate) fn floaty_launch_target(target: String, app: AppHandle) -> Result<(), String> {
    if target.trim().is_empty() {
        return Err("empty target".into());
    }
    log_line(&app, "launch target from folder");
    launch_target(&app, &target)
}

/// Called (fire-and-forget) when an app icon is dropped after a manual drag.
/// If the drop point lands on another icon/folder window, merge them.
/// Returns the folder id when a merge happened.
/// The page's "I let go here". The merge itself lives in `floaty_dropped_inner`.
///
/// This wrapper is where a *move* becomes undoable: a drop that merges takes its own
/// checkpoint inside (with the same remembered origin), and a drop that does not is still
/// a change to the desktop — the icon is somewhere new, and undoing that means putting it
/// back on the spot it came from.
#[tauri::command]
pub(crate) fn floaty_dropped(id: String, x: Option<i32>, y: Option<i32>, app: AppHandle) -> Option<String> {
    let origin = drag_origin(&id);
    let merged = floaty_dropped_inner(id.clone(), x, y, app.clone(), origin);
    // A page that announced a gesture has already been snapshotted at pointerdown, and
    // commits it at pointerup; recording a second step here would spend two Ctrl+Alt+Z
    // presses on one drag. A move that arrives without a gesture — a plugin, a script, a
    // probe — still gets its own step.
    if merged.is_none() && !undo::gesture_pending() {
        record_move(&app, &id, origin);
    }
    forget_drag_origin(&id);
    merged
}

pub(crate) fn floaty_dropped_inner(
    id: String,
    x: Option<i32>,
    y: Option<i32>,
    app: AppHandle,
    origin: Option<(i32, i32)>,
) -> Option<String> {
    // Where the drop is, as a point in record space.
    //
    // The page's coordinates are used *while they agree with the record about which
    // screen the floatie is on*. After a drag that crossed to another screen they do not:
    // the page clamps a drag to the screen it began on, so a drop onto the second screen
    // would be vetted against the tiles at the first screen's edge — merging a real file
    // with nothing on screen to explain it. The record is where the floatie actually is,
    // because the drag handed it over on the way.
    let list = screens(&app);
    let (w, h, stored) = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().ok()?;
        let rec = guard.widgets.get(&id)?;
        let (w, h) = plugins::size(&rec.kind, &rec.data);
        (w as i32, h as i32, (rec.x, rec.y))
    };
    let on_screen = |px: i32, py: i32| screens::screen_of(&list, px as f64, py as f64);
    let (cx, cy) = if let (Some(px), Some(py)) = (x, y) {
        let (px, py) = if on_screen(px, py) == on_screen(stored.0, stored.1) {
            (px, py)
        } else {
            stored
        };
        // Nothing is dropped where no window draws it: the band between two screens at
        // different scales belongs to none of them, so the floatie comes back first and
        // the drop is judged where it actually is.
        if on_screen(px, py).is_none() {
            rehome_stranded(&app, "drop");
            let state = app.state::<AppState>();
            let guard = state.0.lock().ok()?;
            let rec = guard.widgets.get(&id)?;
            (rec.x, rec.y)
        } else {
            (px, py)
        };
        (px + w / 2, py + h / 2)
    } else if let Some(me) = app.get_webview_window(&widget_label(&id)) {
        let (mp, ms) = (me.outer_position().ok()?, me.inner_size().ok()?);
        (mp.x + ms.width as i32 / 2, mp.y + ms.height as i32 / 2)
    } else {
        return None;
    };

    // candidate targets (snapshot under the lock, probe windows after release)
    let cands: Vec<(String, String)> = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().ok()?;
        guard
            .widgets
            .iter()
            .filter(|(oid, r)| *oid != &id && plugins::is_desktop_item(&r.kind))
            .map(|(oid, r)| (oid.clone(), r.kind.clone()))
            .collect()
    };
    let is_overlay = app.get_webview_window("desktop-overlay").is_some();
    let mut hit: Option<(String, String)> = None;
    for (oid, kind) in cands {
        let (hx, hy, ww, hh) = if is_overlay {
            let state = app.state::<AppState>();
            let guard = state.0.lock().ok()?;
            if let Some(r) = guard.widgets.get(&oid) {
                let (w, h) = plugins::size(&r.kind, &r.data);
                (r.x - 12, r.y - 12, w as i32 + 24, h as i32 + 24)
            } else {
                continue;
            }
        } else if let Some(w) = app.get_webview_window(&widget_label(&oid)) {
            if let (Ok(p), Ok(s)) = (w.outer_position(), w.inner_size()) {
                (p.x - 12, p.y - 12, s.width as i32 + 24, s.height as i32 + 24)
            } else {
                continue;
            }
        } else {
            continue;
        };
        if cx >= hx && cx < hx + ww && cy >= hy && cy < hy + hh {
            hit = Some((oid, kind));
            break;
        }
    }
    let (target_id, target_kind) = hit?;
    // Everything above is a read; from here on real files move, so this is the
    // point where a merge becomes undoable. The folder the merge may create is
    // noted by id below (`undo::add_created`), since it does not exist yet.
    // The dragged icon is snapshotted at the place the drag *started* from, not the place
    // it was dropped — which, for a merge, is inside the folder it just went into. Undo
    // puts the file back out of the folder; this is what puts the icon back with it.
    log_line(
        &app,
        &format!(
            "merge: {id} into {target_id} — dragged from {}",
            origin
                .map(|(x, y)| format!("{x},{y}"))
                .unwrap_or_else(|| "an unknown place".to_string())
        ),
    );
    checkpoint_with_origin(
        &app,
        "merge",
        &[&id, &target_id],
        origin.map(|(x, y)| (id.as_str(), x, y)),
    );

    // snapshot the dragged item, then remove it (tombstone blocks its late
    // saves from resurrecting it)
    let mut dragged = {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().ok()?;
        let rec = guard.widgets.get(&id)?;
        let item = widget_as_folder_item(rec)?;
        if item.target.trim().is_empty() {
            return None;
        }
        guard.widgets.remove(&id);
        guard.dead.insert(id.clone());
        item
    };
    persist(&app);
    close_widget_async(&app, &id);

    // the pointed root, read once — outside the widget lock (it comes from disk)
    let root = files_root_dir(&app);

    if target_kind == "folder" {
        {
            let state = app.state::<AppState>();
            let mut guard = state.0.lock().ok()?;
            let rec = guard.widgets.get_mut(&target_id)?;
            let folder_path = rec.data.get("path").and_then(|v| v.as_str()).map(|s| s.to_string());

            // A folder backed by a directory on disk takes the item into that
            // directory: the file itself when the root owns it, a shortcut file
            // for anything from outside (an app in Program Files, say — moving
            // that one would take the program out of its install directory).
            if let Some(ref fpath) = folder_path {
                let dest_dir = std::path::Path::new(fpath);
                if dest_dir.is_dir() {
                    let moved_from = dragged.target.clone();
                    // bound to a local first: the borrow of `dragged` must end
                    // before it is rebound
                    let placed = match place_item_in_dir(dest_dir, &dragged, root.as_deref()) {
                        Ok((item, log)) => {
                            log_line(&app, &log);
                            Some(item)
                        }
                        Err(log) => {
                            log_line(&app, &log);
                            None
                        }
                    };
                    if let Some(placed) = placed {
                        dragged = placed;
                        // Either the file moved into the folder (undo moves it
                        // back) or a shortcut was written into it because the
                        // original lives outside the root (undo takes the new
                        // file away and leaves the original where it is). Which
                        // one happened is a question about the disk, not a guess.
                        if !std::path::Path::new(&moved_from).exists() {
                            undo::add_disk(undo::DiskOp::moved(&moved_from, &dragged.target));
                        } else if dragged.target != moved_from {
                            undo::add_disk(undo::DiskOp::discard(&dragged.target));
                        }
                    }
                }
            }

            let mut items = folder_items(rec);
            if !items.iter().any(|it| it.target == dragged.target) {
                if dragged.icon.is_empty() && !dragged.is_dir {
                    dragged.icon = resolve_icon_url(&dragged.target).unwrap_or_default();
                }
                items.push(dragged);
                set_folder_items(rec, &items);
            }
        }
        persist(&app);
        app.emit("floaty-folder-changed", &target_id).ok();
        log_line(&app, &format!("merged {id} into folder {target_id}"));
        return Some(target_id);
    }

    // target is an app icon: the two land in a *new folder in the pointed root*,
    // named the way Windows names one. The dragged files move into it; an app
    // icon, whose program lives outside the root, is carried in as a shortcut
    // file instead of being moved out of its install directory.
    let titem = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().ok()?;
        let trec = guard.widgets.get(&target_id)?;
        widget_as_folder_item(trec)?
    };

    if let Some(dir) = root.as_deref().and_then(|r| match create_new_folder(r) {
        Ok(d) => Some(d),
        Err(e) => {
            log_line(&app, &format!("new folder FAILED in {}: {e}", r.display()));
            None
        }
    }) {
        // The folder is new on disk, so undoing this grouping has to take it
        // away again — after the items inside it have been moved back out.
        undo::add_disk(undo::DiskOp::discard_dir(&dir.to_string_lossy()));
        let sources = [titem.target.clone(), dragged.target.clone()];
        let mut placed: Vec<FolderItem> = Vec::new();
        let mut moved: Vec<(String, String)> = Vec::new();
        for (it, source) in [&titem, &dragged].into_iter().zip(sources.iter()) {
            match place_item_in_dir(&dir, it, root.as_deref()) {
                Ok((item, log)) => {
                    log_line(&app, &log);
                    moved.push((source.clone(), item.target.clone()));
                    placed.push(item);
                }
                Err(log) => log_line(&app, &log),
            }
        }
        // One op per item that landed: the file moved into the folder (undo
        // moves it back) or a shortcut was written because the original lives
        // outside the root (undo takes the new file away and leaves the original
        // alone). Which one happened is a question about the disk.
        for (source, target) in &moved {
            if !std::path::Path::new(source).exists() {
                undo::add_disk(undo::DiskOp::moved(source, target));
            } else if target != source {
                undo::add_disk(undo::DiskOp::discard(target));
            }
        }
        if !placed.is_empty() {
            let (tx, ty) = {
                let state = app.state::<AppState>();
                let mut guard = state.0.lock().ok()?;
                let trec = guard.widgets.remove(&target_id)?;
                guard.dead.insert(target_id.clone());
                (trec.x, trec.y)
            };
            persist(&app);
            close_widget_async(&app, &target_id);

            // rescan the directory: items are files again (the shortcuts are new
            // on disk), so carry over the icons already resolved for them
            let mut items = scan_folder_items(&dir);
            carry_icons_by_name(&placed, &mut items);
            let name = dir
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "Folder".to_string());
            let mut data = serde_json::json!({
                "name": name,
                "path": dir.to_string_lossy(),
                "items": items,
            });
            data["pinned"] = serde_json::Value::Bool(true);
            return match create_record_with(&app, "folder", data, Some((tx, ty))) {
                Ok(rec) => {
                    undo::add_created(&rec.id);
                    log_line(
                        &app,
                        &format!(
                            "grouped {id} + {target_id} into {} ({})",
                            rec.id,
                            dir.display()
                        ),
                    );
                    Some(rec.id)
                }
                Err(e) => {
                    log_line(&app, &format!("folder create FAILED: {e}"));
                    None
                }
            };
        }
        // nothing landed: don't leave an empty folder behind
        fs::remove_dir(&dir).ok();
    }

    // no pointed root (or the folder could not be made): fold both into an
    // in-app folder at the target's spot, as before
    let mut items = vec![titem, dragged];
    for it in items.iter_mut() {
        if it.icon.is_empty() && !it.is_dir {
            it.icon = resolve_icon_url(&it.target).unwrap_or_default();
        }
    }
    let (tx, ty) = {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().ok()?;
        let trec = guard.widgets.remove(&target_id)?;
        guard.dead.insert(target_id.clone());
        (trec.x, trec.y)
    };
    persist(&app);
    close_widget_async(&app, &target_id);
    let data = serde_json::json!({ "name": "Folder", "items": items });
    match create_record_with(&app, "folder", data, Some((tx, ty))) {
        Ok(rec) => {
            undo::add_created(&rec.id);
            log_line(&app, &format!("grouped {id} + {target_id} into {}", rec.id));
            Some(rec.id)
        }
        Err(e) => {
            log_line(&app, &format!("folder create FAILED: {e}"));
            None
        }
    }
}

/// Where an item goes when it is pulled out of a folder.
///
/// Two shapes. A folder backed by a directory on disk holds its items *inside*
/// that directory, so an item comes out beside it (`<folder>/..`). A folder made
/// in-app by grouping has no directory at all — its record carries no `path` and
/// its items were never moved anywhere — so they stay exactly where they are,
/// which is what returning the item's own directory means to the caller (it skips
/// the move when the destination is not different).
///
/// That second case used to step two levels up from the item; for a desktop item
/// that is the parent of the *Desktop*, so ungrouping a folder of desktop icons
/// moved them out of the Desktop (two of the user's landed in the OneDrive root).
pub(crate) fn ungroup_dest_dir(
    folder_path: Option<&str>,
    src: &std::path::Path,
) -> Option<std::path::PathBuf> {
    match folder_path {
        Some(fp) => std::path::Path::new(fp).parent().map(|p| p.to_path_buf()),
        None => src.parent().map(|p| p.to_path_buf()),
    }
}

/// Pull one file/folder out of a folder: remove the item, move it out on disk
/// to the parent directory (relative path change), and float it as its own widget.
#[tauri::command]
pub(crate) fn floaty_ungroup(folder_id: String, index: usize, x: i32, y: i32, app: AppHandle) -> Result<WidgetRecord, String> {
    checkpoint(&app, "ungroup", &[&folder_id]);
    let (mut item, folder_path) = {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        let rec = guard.widgets.get_mut(&folder_id).ok_or("folder not found")?;
        if rec.kind != "folder" {
            return Err("not a folder".into());
        }
        let folder_path = rec.data.get("path").and_then(|v| v.as_str()).map(|s| s.to_string());
        let mut items = folder_items(rec);
        if index >= items.len() {
            return Err("bad index".into());
        }
        let item = items.remove(index);
        if item.target.trim().is_empty() {
            return Err("empty target".into());
        }
        set_folder_items(rec, &items);
        (item, folder_path)
    };
    persist(&app);
    app.emit("floaty-folder-changed", &folder_id).ok();

    // Move file/directory out on disk to the parent directory
    let src_path = std::path::PathBuf::from(&item.target);
    if src_path.exists() {
        let dest_dir = ungroup_dest_dir(folder_path.as_deref(), &src_path);

        if let Some(dest_dir_path) = dest_dir {
            if dest_dir_path.is_dir() && dest_dir_path != src_path.parent().unwrap_or(&src_path) {
                if let Some(file_name) = src_path.file_name() {
                    let dest_path = unique_dest_path(&dest_dir_path, file_name);
                    if dest_path != src_path {
                        if let Ok(_) = std::fs::rename(&src_path, &dest_path) {
                            log_line(&app, &format!("moved out of folder on disk: {} -> {}", src_path.display(), dest_path.display()));
                            undo::add_disk(undo::DiskOp::moved(
                                &src_path.to_string_lossy(),
                                &dest_path.to_string_lossy(),
                            ));
                            item.target = dest_path.to_string_lossy().to_string();
                            item.name = dest_path.file_name().unwrap_or(file_name).to_string_lossy().to_string();
                            item.is_dir = dest_path.is_dir();
                        }
                    }
                }
            }
        }
    }

    // Treat as new floatie: directory -> folder floatie, launchable -> app
    // launcher floatie, anything else -> file floatie
    let target_path = std::path::Path::new(&item.target).to_path_buf();
    let is_dir = item.is_dir || target_path.is_dir();
    let rec = if is_dir {
        let sub_items = scan_folder_items(&target_path);
        let mut data = serde_json::json!({
            "name": item.name,
            "path": item.target,
            "items": sub_items,
        });
        data["pinned"] = serde_json::Value::Bool(true);
        create_record_with(&app, "folder", data, Some((x, y)))?
    } else {
        let mut data = serde_json::json!({
            "name": item.name,
            "target": item.target,
            "icon": item.icon,
        });
        data["pinned"] = serde_json::Value::Bool(true);
        create_record_with(&app, kind_for_path(&target_path, false), data, Some((x, y)))?
    };

    // the record this made is the other half of the undo
    undo::add_created(&rec.id);
    log_line(&app, &format!("ungrouped {} from {folder_id} as {} (kind: {})", item.name, rec.id, rec.kind));
    Ok(rec)
}

#[tauri::command]
pub(crate) fn floaty_rename_folder_dir(folder_id: String, new_name: String, app: AppHandle) -> Result<(), String> {
    let (old_path, new_path) = {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        let rec = guard.widgets.get_mut(&folder_id).ok_or("folder not found")?;
        let path = rec.data.get("path").and_then(|v| v.as_str()).map(|s| s.to_string());
        if let Some(old_p) = path {
            let p = std::path::Path::new(&old_p);
            if let Some(parent) = p.parent() {
                let dest = parent.join(&new_name);
                if p.exists() && dest != p {
                    std::fs::rename(p, &dest).map_err(|e| e.to_string())?;
                    let new_p_str = dest.to_string_lossy().to_string();
                    if let Some(obj) = rec.data.as_object_mut() {
                        obj.insert("path".to_string(), serde_json::Value::String(new_p_str.clone()));
                        obj.insert("name".to_string(), serde_json::Value::String(new_name));
                    }
                    // A rescan starts every item with no icon; the items are the
                    // same files under a new parent, so keep them by name (a
                    // rename used to blank every icon in the folder).
                    let old_items = folder_items(rec);
                    let mut items = scan_folder_items(&dest);
                    carry_icons_by_name(&old_items, &mut items);
                    set_folder_items(rec, &items);
                    (old_p, new_p_str)
                } else {
                    return Ok(());
                }
            } else {
                return Ok(());
            }
        } else {
            return Ok(());
        }
    };
    persist(&app);
    app.emit("floaty-folder-changed", &folder_id).ok();
    log_line(&app, &format!("renamed folder dir on disk: {old_path} -> {new_path}"));
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesSyncResult {
    pub files: usize,
    pub dirs: usize,
    pub total: usize,
    /// Whether this reconcile actually moved anything. The watcher fires it on
    /// any name change, and most of those turn out to be nothing.
    #[serde(default)]
    pub changed: bool,
}

#[tauri::command]
pub(crate) fn floaty_sync_files(root: Option<String>, app: AppHandle) -> Result<FilesSyncResult, String> {
    sync_root(root, app, false)
}

/// Mirror the pointed root: bring the desktop in step with what the folder
/// actually holds — add what is new, refresh what moved, and (the half that
/// makes this a mirror rather than an importer) take off the desktop what is no
/// longer there. `quiet` keeps the watcher's no-op passes out of the log.
pub(crate) fn sync_root(root: Option<String>, app: AppHandle, quiet: bool) -> Result<FilesSyncResult, String> {
    let started = std::time::Instant::now();
    let target_root = match root {
        Some(r) if !r.trim().is_empty() => {
            let mut s = load_settings(&app);
            s.files_root = r.trim().to_string();
            if let Ok(json) = serde_json::to_string_pretty(&s) {
                write_text_atomic(&settings_file(&app), &json);
            }
            s.files_root
        }
        _ => load_settings(&app).files_root,
    };

    if target_root.trim().is_empty() {
        // Nothing is pointed at: nothing to follow, mirror or watch.
        fs_watch::retarget(None, app.clone());
        return Ok(FilesSyncResult {
            files: 0,
            dirs: 0,
            total: 0,
            changed: false,
        });
    }

    let base_path = std::path::PathBuf::from(&target_root);
    if !base_path.is_dir() {
        return Err(format!("'{}' is not a valid directory", target_root));
    }

    // Auto-enable plugins if disabled
    let s = load_settings(&app);
    if s.disabled.iter().any(|d| d == "app") {
        set_plugin_enabled(&app, "app", true);
    }
    if s.disabled.iter().any(|d| d == "folder") {
        set_plugin_enabled(&app, "folder", true);
    }
    if s.disabled.iter().any(|d| d == "file") {
        set_plugin_enabled(&app, "file", true);
    }

    let mut files_count = 0;
    let mut dirs_count = 0;

    let (mut occupied_positions, existing_map, mut next_id) = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        let occupied: Vec<(i32, i32)> = guard.widgets.values().map(|w| (w.x, w.y)).collect();
        let mut map: HashMap<String, (String, String)> = HashMap::new();
        for (id, w) in &guard.widgets {
            if w.kind == "folder" {
                if let Some(p) = w.data.get("path").and_then(|v| v.as_str()) {
                    map.insert(p.to_string(), (id.clone(), "folder".to_string()));
                }
            } else if is_path_kind(&w.kind) {
                if let Some(p) = w.data.get("target").and_then(|v| v.as_str()) {
                    map.insert(p.to_string(), (id.clone(), w.kind.clone()));
                }
            }
        }
        (occupied, map, guard.next)
    };

    let next_pos = |occupied: &mut Vec<(i32, i32)>| -> (i32, i32) {
        let mut cur_x = 80;
        let mut cur_y = 80;
        loop {
            let collision = occupied.iter().any(|(x, y)| (*x - cur_x).abs() < 50 && (*y - cur_y).abs() < 50);
            if !collision {
                occupied.push((cur_x, cur_y));
                return (cur_x, cur_y);
            }
            cur_y += 115;
            if cur_y > 800 {
                cur_y = 80;
                cur_x += 105;
            }
        }
    };

    let mut updated_folders: Vec<(String, Vec<FolderItem>)> = Vec::new();
    let mut new_records: Vec<WidgetRecord> = Vec::new();
    // What the folder holds right now, lowercased: Windows compares paths
    // without case, so a record's stored path has to be matched the same way to
    // decide whether the thing it mirrors is still there.
    let mut on_disk: HashSet<String> = HashSet::new();
    // Only a listing that ran to the end can be read as "this is the folder":
    // a directory we could not open must never look like a directory that is not
    // there, or one permission problem would take every floatie off the desktop.
    let mut listing_complete = false;

    if let Ok(entries) = std::fs::read_dir(&base_path) {
        let mut unreadable = false;
        for entry in entries {
            let Ok(entry) = entry else {
                unreadable = true;
                continue;
            };
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();
            if file_name.starts_with('.')
                || file_name.eq_ignore_ascii_case("desktop.ini")
                || file_name.eq_ignore_ascii_case("thumbs.db")
            {
                continue;
            }
            let path_str = path.to_string_lossy().to_string();
            on_disk.insert(path_str.to_ascii_lowercase());
            if path.is_dir() {
                let mut items = scan_folder_items(&path);
                match existing_map.get(&path_str) {
                    Some((fid, kind)) if kind == "folder" => {
                        let mut refreshed = true;
                        if let Ok(guard) = app.state::<AppState>().0.lock() {
                            if let Some(w) = guard.widgets.get(fid) {
                                let old_items = folder_items(w);
                                carry_icons_by_target(&old_items, &mut items);
                                // an external rename moved every target at once:
                                // the items are the same files under new paths
                                carry_icons_by_name(&old_items, &mut items);
                                // An idle folder must not look changed: the write
                                // below is what the store persist hangs on.
                                refreshed = old_items != items;
                            }
                        }
                        if refreshed {
                            updated_folders.push((fid.clone(), items));
                        }
                    }
                    // already represented by a record of another kind: leave it
                    Some(_) => {}
                    None => {
                        let (px, py) = next_pos(&mut occupied_positions);
                        next_id += 1;
                        let id = format!("folder-{}", next_id);
                        let data = serde_json::json!({
                            "name": file_name,
                            "path": path_str,
                            "items": items,
                            "pinned": true,
                        });
                        new_records.push(WidgetRecord {
                            id,
                            kind: "folder".to_string(),
                            x: px,
                            y: py,
                            data,
                        });
                    }
                }
                dirs_count += 1;
            } else {
                if !existing_map.contains_key(&path_str) {
                    let (px, py) = next_pos(&mut occupied_positions);
                    next_id += 1;
                    // launchable entries are apps, everything else is a file
                    let kind = kind_for_path(&path, false);
                    let id = format!("{kind}-{}", next_id);
                    let mut data = serde_json::json!({
                        "name": file_name,
                        "target": path_str,
                        "icon": "",
                    });
                    data["pinned"] = serde_json::Value::Bool(true);
                    new_records.push(WidgetRecord {
                        id,
                        kind: kind.to_string(),
                        x: px,
                        y: py,
                        data,
                    });
                }
                files_count += 1;
            }
        }
        listing_complete = !unreadable;
    }

    // The other half of the mirror: a record for something that is no longer in
    // the folder comes off the desktop. Only records the folder owns are
    // eligible — a floatie pointing outside the root (an app floated from
    // Program Files, a file carried in from elsewhere) is not something this
    // folder's listing can answer for — and only when the listing itself is
    // trustworthy, because a half-read folder must not read as "gone".
    let mut vanished: Vec<String> = Vec::new();
    if !listing_complete {
        log_line(
            &app,
            &format!("synced files root '{}': listing incomplete — keeping every floatie it could not account for", target_root),
        );
    } else {
        let state = app.state::<AppState>();
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        for (id, w) in &guard.widgets {
            let Some(path) = record_path(w) else { continue };
            if !mirrors_root_entry(&target_root, &path) {
                continue;
            }
            if on_disk.contains(&path.to_ascii_lowercase()) {
                continue;
            }
            vanished.push(id.clone());
        }
    }

    let mut changed = false;
    {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        guard.next = next_id;
        for (fid, items) in &updated_folders {
            if let Some(w) = guard.widgets.get_mut(fid) {
                set_folder_items(w, items);
                changed = true;
            }
        }
        for rec in &new_records {
            guard.widgets.insert(rec.id.clone(), rec.clone());
            changed = true;
        }
        for id in &vanished {
            if guard.widgets.remove(id).is_some() {
                changed = true;
            }
        }
    }

    // A reconcile that found nothing must not rewrite the store: it is megabytes
    // with every icon inlined, and the watcher calls this on any name change —
    // including ones that turn out to be nothing, or a folder that is simply
    // idle while OneDrive touches its metadata.
    if changed {
        persist(&app);
    }

    for (fid, _) in &updated_folders {
        app.emit("floaty-folder-changed", fid).ok();
    }
    for id in &vanished {
        hide_widget(&app, id);
    }

    if !new_records.is_empty() {
        if app.get_webview_window("desktop-overlay").is_some() {
            for rec in &new_records {
                app.emit("floaty-widget-added", rec).ok();
            }
        } else {
            let app_handle = app.clone();
            let to_spawn = new_records.clone();
            std::thread::spawn(move || {
                for rec in to_spawn {
                    let h = app_handle.clone();
                    let r = rec.clone();
                    let _ = app_handle.run_on_main_thread(move || {
                        let _ = spawn_widget(&h, &r);
                    });
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
            });
        }
    }

    if changed {
        let bg_app = app.clone();
        tauri::async_runtime::spawn(async move {
            upgrade_low_res_icons(&bg_app).await;
        });
        // The settings window lists the same floaties; tell it they moved.
        app.emit("floaty-widgets-changed", ()).ok();
    }

    // From here on the folder is followed rather than read once: the watcher
    // re-runs this reconcile whenever it changes. `retarget` no-ops when this is
    // already the watched root, so every sync can call it.
    fs_watch::retarget(Some(target_root.clone()), app.clone());

    // A watcher-driven pass that found nothing is not worth a line; a manual
    // sync always reports, since someone is waiting to read it.
    if !quiet || changed {
        log_line(
            &app,
            &format!(
                "synced files root '{}': {} files, {} dirs ({} new, {} gone, {} folders refreshed) in {}ms",
                target_root,
                files_count,
                dirs_count,
                new_records.len(),
                vanished.len(),
                updated_folders.len(),
                started.elapsed().as_millis()
            ),
        );
    }
    Ok(FilesSyncResult {
        files: files_count,
        dirs: dirs_count,
        total: files_count + dirs_count,
        changed,
    })
}

/// The path a record mirrors, when its kind stands for something on disk.
pub(crate) fn record_path(rec: &WidgetRecord) -> Option<String> {
    let key = plugins::path_key(&rec.kind)?;
    rec.data
        .get(key.as_str())
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Whether a record mirrors one of the *root's own entries* — a file or folder
/// the scan would list.
///
/// Deliberately not "anywhere under the root": the reconcile only lists the
/// root's top level, so a floatie pointing at something deeper (a file ungrouped
/// out of a folder that lives inside the root) is not something that listing can
/// account for, and reading its absence as "gone" would take it off the desktop.
/// Windows compares paths without case, and a stored path can disagree with the
/// root about it (a shortcut written by Explorer, a root typed by hand), so the
/// comparison folds case.
pub(crate) fn mirrors_root_entry(root: &str, path: &str) -> bool {
    let root = root.trim_end_matches(['\\', '/']).to_ascii_lowercase();
    if root.is_empty() {
        return false;
    }
    let path = path.to_ascii_lowercase();
    match path.rfind(['\\', '/']) {
        Some(cut) => path[..cut] == root,
        None => false,
    }
}

/// Move floaties with their files.
///
/// An external rename arrives as a pair, and applying it here — rather than
/// letting the reconcile drop the old record and add a new one — is what keeps
/// the tile where the user put it, with its icon: the desktop follows the file
/// instead of shuffling around it.
pub(crate) fn apply_root_renames(app: &AppHandle, renames: &[(String, String)]) -> usize {
    if renames.is_empty() {
        return 0;
    }
    let mut moved: Vec<WidgetRecord> = Vec::new();
    {
        let state = app.state::<AppState>();
        let Ok(mut guard) = state.0.lock() else {
            return 0;
        };
        for (old, new) in renames {
            let from = old.to_ascii_lowercase();
            let hit = guard
                .widgets
                .iter()
                .find(|(_, w)| record_path(w).is_some_and(|p| p.to_ascii_lowercase() == from))
                .map(|(id, w)| (id.clone(), plugins::path_key(&w.kind).unwrap_or_default()));
            let Some((id, key)) = hit else { continue };
            let label = std::path::Path::new(new)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let Some(w) = guard.widgets.get_mut(&id) else {
                continue;
            };
            if let Some(obj) = w.data.as_object_mut() {
                obj.insert(key, serde_json::Value::String(new.clone()));
                // the label follows the file it points at
                if !label.is_empty() && obj.contains_key("name") {
                    obj.insert("name".to_string(), serde_json::Value::String(label));
                }
            }
            moved.push(w.clone());
        }
    }
    if moved.is_empty() {
        return 0;
    }
    persist(app);
    for rec in &moved {
        // the event `floaty_rename` sends: the overlay remounts that slot
        app.emit("floaty-widget-updated", rec).ok();
    }
    log_line(
        app,
        &format!("watch: {} floatie(s) followed a rename", moved.len()),
    );
    moved.len()
}

/// The pointed root changed on disk: bring the desktop back in step.
///
/// Runs on the watcher thread, so it does the cheap thing and lets the reconcile
/// decide what actually changed: move the floaties whose files were renamed,
/// then re-run the mirror — which is idempotent, so a redundant call costs one
/// `read_dir` and nothing else.
pub(crate) fn root_changed(app: &AppHandle, renames: &[(String, String)]) {
    apply_root_renames(app, renames);
    // The reconcile reports for itself when it moved something, and
    // `apply_root_renames` reports the renames; what needs saying here is only
    // when the mirror could not run at all (a root gone read-only, a drive out).
    if let Err(err) = sync_root(None, app.clone(), true) {
        log_line(app, &format!("watch: mirror failed: {err}"));
    }
}
