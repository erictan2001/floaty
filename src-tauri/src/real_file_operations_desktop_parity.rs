//! `real_file_operations_desktop_parity` — moved out of lib.rs verbatim.
//!
//! Visibility is `pub(crate)` because the rest of the crate calls in across the module
//! boundary now; nothing here changed shape on the way out.

use crate::*;
use tauri::{AppHandle, Emitter, Manager, State};

// ---------- real file operations (desktop parity) ----------

/// The on-disk path a widget points at, with its kind. Folders keep theirs in
/// `path`, files and launchable items in `target`.
pub(crate) fn widget_path(app: &AppHandle, id: &str) -> Result<(String, String), String> {
    let state = app.state::<AppState>();
    let guard = state.0.lock().map_err(|e| e.to_string())?;
    let rec = guard.widgets.get(id).ok_or("widget not found")?;
    let key = plugins::path_key(&rec.kind);
    let path = key
        .and_then(|k| rec.data.get(k))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if path.trim().is_empty() {
        return Err(format!("this {} has no path", rec.kind));
    }
    Ok((rec.kind.clone(), path))
}

/// True when the path is one this app manages (a desktop item), i.e. deleting
/// the floatie may delete the file as well. Anything else is left alone —
/// removing the floatie for an installed program must not uninstall it.
pub(crate) fn is_desktop_item(app: &AppHandle, path: &str) -> bool {
    let p = std::path::Path::new(path);
    let root = load_settings(app).files_root;
    if !root.trim().is_empty() && p.starts_with(&root) {
        return true;
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        if p.starts_with(std::path::Path::new(&home).join("Desktop")) {
            return true;
        }
    }
    false
}

/// Delete a floatie the way the desktop would: a file or folder that lives on
/// the desktop goes to the Recycle Bin, so deleting the widget is a real
/// delete. Paths outside the desktop only lose their floatie.
#[tauri::command]
pub(crate) fn floaty_delete(id: String, app: AppHandle) -> Result<String, String> {
    let (kind, path) = widget_path(&app, &id)?;
    let managed = is_desktop_item(&app, &path)
        && (kind == "file" || kind == "folder" || kind == "app");
    checkpoint(&app, "delete", &[&id]);
    let note = if managed {
        // Where the file went is *reported*, not assumed: `FOF_ALLOWUNDO` asks for the
        // bin, and a volume whose bin is disabled (or too small for the file) deletes
        // outright. The undo entry is only honest when it really is in the bin.
        match shell_ops::recycle_checked(std::path::Path::new(&path))? {
            shell_ops::Recycled::ToBin => {
                // one press puts the file back out of the bin and the floatie with it
                undo::add_disk(undo::DiskOp::recycled(&path));
                format!("moved '{}' to the recycle bin", path)
            }
            shell_ops::Recycled::Permanently => {
                // No disk entry: an "unrecycle" that cannot work is worse than no undo at
                // all, and the message is the only honest thing left to give.
                format!(
                    "deleted '{}' for good — this drive's Recycle Bin would not take it",
                    path
                )
            }
        }
    } else {
        format!("removed the floatie; '{}' stays on disk", path)
    };
    let state: State<'_, AppState> = app.state::<AppState>();
    if let Ok(mut guard) = state.0.lock() {
        guard.widgets.remove(&id);
        guard.dead.insert(id.clone());
    }
    persist(&app);
    close_widget_async(&app, &id);
    app.emit("floaty-widget-removed", &id).ok();
    log_line(&app, &format!("delete {id}: {note}"));
    Ok(note)
}

/// The shell's own "Open with" picker.
#[tauri::command]
pub(crate) fn floaty_open_with(id: String, app: AppHandle) -> Result<(), String> {
    let (_, path) = widget_path(&app, &id)?;
    shell_ops::open_with(&path)
}

/// Show the item selected in File Explorer.
#[tauri::command]
pub(crate) fn floaty_reveal(id: String, app: AppHandle) -> Result<(), String> {
    let (_, path) = widget_path(&app, &id)?;
    shell_ops::reveal(&path)
}

/// The real Windows property sheet.
#[tauri::command]
pub(crate) fn floaty_properties(id: String, app: AppHandle) -> Result<(), String> {
    let (_, path) = widget_path(&app, &id)?;
    shell_ops::properties(&path)
}

/// Rename on disk, keeping the widget's label, target and folder path in step.
#[tauri::command]
pub(crate) fn floaty_rename(id: String, name: String, app: AppHandle) -> Result<WidgetRecord, String> {
    let (kind, path) = widget_path(&app, &id)?;
    let old = std::path::PathBuf::from(&path);
    let new_path = shell_ops::rename(&old, &name)?;
    let new_str = new_path.to_string_lossy().to_string();
    let new_name = new_path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| name.clone());
    let rec = {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        let r = guard.widgets.get_mut(&id).ok_or("widget not found")?;
        if let Some(obj) = r.data.as_object_mut() {
            obj.insert("name".to_string(), serde_json::Value::String(new_name));
            obj.insert("target".to_string(), serde_json::Value::String(new_str.clone()));
            if kind == "folder" {
                obj.insert("path".to_string(), serde_json::Value::String(new_str.clone()));
            }
        }
        r.clone()
    };
    persist(&app);
    app.emit("floaty-widget-updated", &rec).ok();
    log_line(&app, &format!("renamed {id}: {} -> {new_str}", old.display()));

    // A rename can change the icon (x.txt -> x.png): re-resolve in the background
    // and push it, so the tile never keeps the icon of its old name.
    let old_ext = old.extension().map(|e| e.to_string_lossy().to_lowercase());
    let new_ext = new_path.extension().map(|e| e.to_string_lossy().to_lowercase());
    if old_ext != new_ext {
        let handle = app.clone();
        let target = new_str.clone();
        let widget_id = id.clone();
        tauri::async_runtime::spawn(async move {
            let resolved = {
                let t = target.clone();
                tauri::async_runtime::spawn_blocking(move || resolve_icon_url(&t))
                    .await
                    .ok()
                    .flatten()
            };
            let Some(icon) = resolved else { return };
            {
                let state = handle.state::<AppState>();
                let Ok(mut guard) = state.0.lock() else { return };
                if let Some(r) = guard.widgets.get_mut(&widget_id) {
                    if let Some(obj) = r.data.as_object_mut() {
                        obj.insert("icon".to_string(), serde_json::Value::String(icon));
                    }
                }
            }
            persist(&handle);
            handle.emit("floaty-icon-refreshed", &widget_id).ok();
            log_line(&handle, &format!("icon re-resolved after renaming {widget_id}"));
        });
    }
    Ok(rec)
}

#[tauri::command]
pub(crate) fn floaty_show_settings(app: AppHandle) -> Result<(), String> {
    show_settings(&app).map_err(|e| {
        log_line(&app, &format!("show_settings FAILED: {e}"));
        e.to_string()
    })
}

#[tauri::command]
pub(crate) fn floaty_quit(app: AppHandle) {
    #[cfg(windows)]
    {
        desktop_pin::cleanup_hook();
    }
    app.exit(0);
}

#[tauri::command]
pub(crate) async fn floaty_log(msg: String, app: AppHandle) {
    log_line(&app, &format!("webview: {msg}"));
}

#[tauri::command]
pub(crate) fn floaty_desktop_rect(app: AppHandle) -> screens::Rect {
    let (x, y, w, h) = overlay_rect(&app);
    screens::Rect::new(x, y, w, h)
}

#[tauri::command]
pub(crate) fn floaty_monitors(app: AppHandle) -> Vec<screens::Rect> {
    screens(&app).iter().map(|s| s.logical).collect()
}
