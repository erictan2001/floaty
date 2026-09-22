//! `commands` — moved out of lib.rs verbatim.
//!
//! Visibility is `pub(crate)` because the rest of the crate calls in across the module
//! boundary now; nothing here changed shape on the way out.

use crate::*;
use tauri::{AppHandle, Emitter, Manager};

// ---------- commands ----------

/// One widget record.
///
/// This exists because `loadRecord` used to call `floaty_list` and filter: every
/// widget's mount then pulled the whole store over IPC — 8.5MB with the icons
/// inlined — so a page with sixty widgets moved ~510MB and parsed it sixty times.
/// Measured: `floaty_list` at ~5s per call, reloads of 20-60s. One record is a
/// few KB.
#[tauri::command]
pub(crate) async fn floaty_get_record(id: String, app: AppHandle) -> Option<WidgetRecord> {
    let state = app.state::<AppState>();
    let guard = state.0.lock().ok()?;
    guard.widgets.get(&id).cloned()
}

#[tauri::command]
pub(crate) async fn floaty_list(app: AppHandle) -> Vec<WidgetRecord> {
    let state = app.state::<AppState>();
    state
        .0
        .lock()
        .map(|g| g.widgets.values().cloned().collect())
        .unwrap_or_default()
}

#[tauri::command]
pub(crate) fn floaty_create(kind: String, app: AppHandle) -> Result<WidgetRecord, String> {
    // A widget of an unapproved plugin would mount nothing and sit on the desktop
    // as a square, so refuse it with the reason instead.
    let settings = load_settings(&app);
    if plugins::find(&kind).is_none() && !plugin_approved(&settings, &plugins_dir(&app).join(&kind), &kind) {
        return Err(format!(
            "{kind} is not approved — approve it in the Plugins tab first"
        ));
    }
    create_record(&app, &kind)
}

#[tauri::command]
pub(crate) async fn floaty_save(mut record: WidgetRecord, app: AppHandle) {
    // A save carries the whole record, position included, and the page reads that position from
    // the widget's live slot — which is placed locally, from the pointer, without the clamp the
    // drag path applies to the record. So a save could put a widget back over an edge the drag
    // had been kept away from: measured, a plugin widget whose body ended 36px past the right of
    // the desktop, in a record no drag had touched. Clamped here as well — no write of any kind
    // may leave the desktop.
    //
    // Before the lock, deliberately: `screens` reads the same state this command is about to
    // take.
    let list = screens(&app);
    if !list.is_empty() {
        let (w, h) = plugins::size(&record.kind, &record.data);
        let (cx, cy) = screens::clamp_into(
            screens::union(&list),
            record.x as f64,
            record.y as f64,
            w,
            h,
            0.0,
        );
        record.x = cx as i32;
        record.y = cy as i32;
    }
    let state = app.state::<AppState>();
    if let Ok(mut guard) = state.0.lock() {
        if guard.dead.contains(&record.id) {
            return; // removed meanwhile (remove / folder-merge)
        }
        // Protect a stored icon from being overwritten by stale incoming data:
        // never lose an icon, and never trade a crisp one for a blurry one.
        if let Some(existing) = guard.widgets.get(&record.id) {
            if let Some(existing_icon) = existing.data.get("icon").and_then(|v| v.as_str()) {
                let incoming_icon = record.data.get("icon").and_then(|v| v.as_str()).unwrap_or("");
                let would_lose = icons::is_missing(incoming_icon) && !icons::is_missing(existing_icon);
                let would_downgrade =
                    icons::is_low_res(incoming_icon) && !icons::is_low_res(existing_icon);
                if would_lose || would_downgrade {
                    if let Some(obj) = record.data.as_object_mut() {
                        obj.insert(
                            "icon".to_string(),
                            serde_json::Value::String(existing_icon.to_string()),
                        );
                    }
                }
            }
            if record.kind == "folder" {
                let existing_items = folder_items(existing);
                let mut incoming_items = folder_items(&record);
                for in_it in incoming_items.iter_mut() {
                    if let Some(ex) = existing_items.iter().find(|e| e.target == in_it.target) {
                        let would_lose = icons::is_missing(&in_it.icon) && !icons::is_missing(&ex.icon);
                        let would_downgrade =
                            icons::is_low_res(&in_it.icon) && !icons::is_low_res(&ex.icon);
                        if would_lose || would_downgrade {
                            in_it.icon = ex.icon.clone();
                        }
                    }
                }
                set_folder_items(&mut record, &incoming_items);
            }
        }
        guard.widgets.insert(record.id.clone(), record);
    }
    persist(&app);
}

/// Re-read these widgets from the store and rebuild them where they now are.
///
/// A floatie mounts once and then holds its own copy of the record: a plugin that
/// moves *other* floaties — the trail plugin lines them up along a drawn path —
/// can write their positions, but the ones on screen will not notice until they are
/// remounted. This is that remount, over the same event a rename sends. It is how
/// an icon that was still in the air when it was placed ends up standing where it
/// was put, instead of falling to the floor the next time it is mounted.
#[tauri::command]
pub(crate) fn floaty_refresh(ids: Vec<String>, app: AppHandle) {
    let records: Vec<WidgetRecord> = {
        let state = app.state::<AppState>();
        let guard = match state.0.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        ids.iter()
            .filter_map(|id| guard.widgets.get(id).cloned())
            .collect()
    };
    if records.is_empty() {
        return;
    }
    log_line(
        &app,
        &format!("refresh: remounted {} floatie(s)", records.len()),
    );
    for rec in &records {
        app.emit("floaty-widget-updated", rec).ok();
    }
}
