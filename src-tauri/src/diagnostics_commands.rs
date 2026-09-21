//! `diagnostics` — moved out of lib.rs verbatim.
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

// ---------- diagnostics ----------

/// Everything the app knows about itself: the log tail, the monitor inventory,
/// where each window actually is, the heartbeat table, and the sizes on disk.
#[tauri::command]
pub(crate) async fn floaty_diagnostics(app: AppHandle) -> diagnostics::Report {
    let dir = app_data_dir(&app);
    let log_path = dir.join("floaty.log");
    let (lines, log_bytes) = diagnostics::read_log(&log_path);

    // The store plus whatever `.bak`/`.tmp` siblings the atomic writes have left.
    let mut store_bytes = fs::metadata(store_file(&app)).map(|m| m.len()).unwrap_or(0);
    let mut backups = 0;
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            if entry.file_name().to_string_lossy().starts_with("floaty-store.json.") {
                backups += 1;
                store_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }

    let icon_dir = icons_dir(&app);
    let (icon_files, icon_bytes) = diagnostics::dir_size(&icon_dir);

    let (installed, rejected) = plugins::install_from(&plugins_dir(&app));

    let records = app
        .state::<AppState>()
        .0
        .lock()
        .map(|guard| guard.widgets.len() as u64)
        .unwrap_or(0);

    let now = elapsed_ms();
    let mut heartbeats: Vec<diagnostics::BeatInfo> = heartbeats()
        .lock()
        .map(|map| {
            map.iter()
                .map(|(label, (at, visibility))| diagnostics::BeatInfo {
                    label: label.clone(),
                    visibility: visibility.clone(),
                    ms_ago: now.saturating_sub(*at),
                })
                .collect()
        })
        .unwrap_or_default();
    heartbeats.sort_by(|a, b| a.label.cmp(&b.label));

    let mut repairs: Vec<diagnostics::RepairInfo> = heartbeat_repairs()
        .lock()
        .map(|map| {
            map.iter()
                .map(|(label, (_, attempts))| diagnostics::RepairInfo {
                    label: label.clone(),
                    attempts: *attempts,
                })
                .collect()
        })
        .unwrap_or_default();
    repairs.sort_by(|a, b| a.label.cmp(&b.label));

    let primary = app
        .primary_monitor()
        .ok()
        .flatten()
        .and_then(|m| m.name().map(|n| n.to_string()));

    let monitors = app
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let name = m.name().map(|n| n.to_string()).unwrap_or_default();
            diagnostics::MonitorInfo {
                primary: Some(name.clone()) == primary,
                key: name.clone(),
                name,
                x: m.position().x,
                y: m.position().y,
                width: m.size().width,
                height: m.size().height,
                scale: m.scale_factor(),
            }
        })
        .collect();

    // Physical px, which is what the platform reports and what the monitor list
    // above is in: "the window is where I think it is" is the whole point.
    let mut windows: Vec<diagnostics::WindowInfo> = app
        .webview_windows()
        .into_iter()
        .map(|(label, window)| {
            let pos = window.outer_position().unwrap_or(tauri::PhysicalPosition::new(0, 0));
            let size = window.outer_size().unwrap_or(tauri::PhysicalSize::new(0, 0));
            diagnostics::WindowInfo {
                layer: if is_desktop_layer_label(&label) {
                    "desktop".to_string()
                } else {
                    "window".to_string()
                },
                visible: window.is_visible().unwrap_or(false),
                label,
                x: pos.x,
                y: pos.y,
                width: size.width,
                height: size.height,
            }
        })
        .collect();
    windows.sort_by(|a, b| a.label.cmp(&b.label));

    let display = match DISPLAY_STATE.load(std::sync::atomic::Ordering::SeqCst) {
        1 => "on",
        0 => "off",
        _ => "unknown",
    };

    diagnostics::Report {
        version: app.package_info().version.to_string(),
        mode: if load_settings(&app).stay_on_desktop {
            "desktop".to_string()
        } else {
            "floating".to_string()
        },
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        started_ms_ago: now,
        log: diagnostics::LogInfo {
            rotated: diagnostics::rolled_count(&log_path),
            path: log_path.to_string_lossy().to_string(),
            bytes: log_bytes,
            lines,
        },
        store: diagnostics::FileInfo {
            path: store_file(&app).to_string_lossy().to_string(),
            bytes: store_bytes,
            backups,
        },
        icons: diagnostics::DirInfo {
            path: icon_dir.to_string_lossy().to_string(),
            files: icon_files,
            bytes: icon_bytes,
        },
        records,
        installed_plugins: installed,
        rejected_plugins: rejected,
        display: display.to_string(),
        heartbeats,
        repairs,
        monitors,
        windows,
    }
}

/// Say something in a Windows notification. Raised from Rust on purpose: a
/// third-party plugin reaches it through the widget api, so it needs no permission
/// of its own and no plugin command is exposed to a page.
#[tauri::command]
pub(crate) fn floaty_notify(title: String, body: String, app: AppHandle) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("a notification needs a title".to_string());
    }
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| format!("the notification was refused: {e}"))
}

/// The arrangement changed: a display was plugged in, unplugged or re-scaled.
///
/// Three things go stale at once, and all three are the user's problem: the overlay
/// is the size it was built with, the pages hold the layout they read at mount (drag
/// bounds, and the floor a falling icon rests on), and a floatie that was on a screen
/// that just went away is sitting at coordinates that are on no screen at all.
///
/// Runs on a thread it is handed, never on the pumping thread: it touches the
/// webview, the window manager and the disk.
pub fn display_arrangement_changed() {
    static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = elapsed_ms();
    let previous = LAST.swap(now, std::sync::atomic::Ordering::SeqCst);
    // Every top-level window gets the message — five of ours means five threads — and
    // one change is one job. Short enough that a second, real change still counts.
    if now.saturating_sub(previous) < 2_000 {
        return;
    }
    let Some(app) = shared_app() else {
        return;
    };
    log_line(&app, "monitors: the arrangement changed");
    reconcile_desktop(&app);
}

/// Fit everything to the arrangement that exists now: the overlay windows, the pages
/// that hold a cached layout, and any floatie sitting off every screen.
///
/// Four callers and one implementation: `WM_DISPLAYCHANGE`, startup, the button in the
/// Diagnostics tab — and the palette when it moves itself to another screen, which is
/// also how this gets exercised without unplugging anything.
pub(crate) fn reconcile_desktop(app: &AppHandle) -> Vec<String> {
    if let Err(e) = spawn_overlay_windows(app) {
        log_line(app, &format!("monitors: could not build the overlays: {e}"));
    }
    fit_overlay_to_monitor(app);
    close_extra_overlays(app);
    let union = screens::union(&screens(app));
    log_line(
        app,
        &format!(
            "monitors: the desktop is now {},{} {}x{}",
            union.x, union.y, union.w, union.h
        ),
    );
    // Pages re-read the layout from this. `display` is "arrangement" rather than
    // "on"/"off" on purpose: a plugin watching the screen state ignores a value it
    // does not know rather than guessing, and this is not a screen state.
    let _ = app.emit(
        "floaty-display-changed",
        serde_json::json!({
            "display": "arrangement",
            "desktop": { "x": union.x, "y": union.y, "w": union.w, "h": union.h },
        }),
    );
    rehome_stranded(app, "monitors")
}

/// Bring back every floatie whose screen is gone, and say how many.
///
/// Two ways to end up off-screen, and both are silent: a display is unplugged while the
/// desktop covers it (the arrangement shrinks and the record keeps its old
/// coordinates), or the app starts after that happened. Either way the record is
/// `pinned`, so the physics will never move it, and nothing else in the app has an
/// opinion about position — so without this it is simply not on the screen any more.
/// Measured: two screens at 2.00/1.50, second one unplugged, and the floaties that had
/// been dragged there never came back.
pub(crate) fn rehome_stranded(app: &AppHandle, why: &str) -> Vec<String> {
    let list = screens(app);
    if list.is_empty() {
        return Vec::new();
    }
    let mut moved: Vec<String> = Vec::new();
    {
        let state = app.state::<AppState>();
        let Ok(mut guard) = state.0.lock() else {
            return Vec::new();
        };
        // What is already on each screen, so a floatie coming back lands *beside* its
        // neighbours rather than under them.
        let mut taken: Vec<Vec<screens::Rect>> = vec![Vec::new(); list.len()];
        for rec in guard.widgets.values() {
            let (x, y) = (rec.x as f64, rec.y as f64);
            if let Some(index) = screens::screen_of(&list, x, y) {
                let (w, h) = plugins::size(&rec.kind, &rec.data);
                taken[index].push(screens::Rect::new(x, y, w, h));
            }
        }

        // Top-down, left-to-right: with several coming back at once, which one ends up
        // where should not depend on a hash map's iteration order.
        let mut stranded: Vec<(i32, i32, String, String, serde_json::Value)> = guard
            .widgets
            .values()
            .filter(|rec| screens::screen_of(&list, rec.x as f64, rec.y as f64).is_none())
            .map(|rec| {
                (rec.y, rec.x, rec.id.clone(), rec.kind.clone(), rec.data.clone())
            })
            .collect();
        stranded.sort_by_key(|(y, x, _, _, _)| (*y, *x));

        for (_, _, id, kind, data) in stranded {
            let Some(rec) = guard.widgets.get(&id) else {
                continue;
            };
            let (x, y) = (rec.x as f64, rec.y as f64);
            let Some(index) = screens::nearest(&list, x, y) else {
                continue;
            };
            // The widget's own size, from the manifest that owns it — the same
            // `size(kind, data)` a fresh widget is built from, so a panel and an icon
            // each come back clear of the edge by their own width, not by a guess.
            let (w, h) = plugins::size(&kind, &data);
            let start = screens::clamp_into(list[index].logical, x, y, w, h, screens::MARGIN);
            let (nx, ny) = screens::place_without_overlap(
                list[index].logical,
                w,
                h,
                start,
                &taken[index],
                screens::MARGIN,
                screens::GAP,
            );
            taken[index].push(screens::Rect::new(nx, ny, w, h));
            if let Some(rec) = guard.widgets.get_mut(&id) {
                rec.x = nx as i32;
                rec.y = ny as i32;
            }
            moved.push(id);
        }

        // And whatever is left sticking out of the screen it is on comes back too. Its
        // top-left was on the screen and its body was not, and an overlay window *is* one
        // screen, so the part past the edge was drawn by nobody — invisible, and unclickable
        // wherever it overlapped nothing. Flush is allowed: this is a clamp, not a re-home,
        // so a widget a person pushed to the edge stays where they put it, just wholly on the
        // screen.
        for (id, rec) in guard.widgets.iter_mut() {
            let (w, h) = plugins::size(&rec.kind, &rec.data);
            if screens::within_one_screen(&list, rec.x as f64, rec.y as f64, w, h) {
                continue;
            }
            let Some((nx, ny)) = screens::confine(&list, rec.x as f64, rec.y as f64, w, h, 0.0)
            else {
                continue;
            };
            rec.x = nx as i32;
            rec.y = ny as i32;
            moved.push(id.clone());
        }
    }
    if moved.is_empty() {
        return moved;
    }
    persist(app);
    let records: Vec<WidgetRecord> = {
        let state = app.state::<AppState>();
        let Ok(guard) = state.0.lock() else {
            return moved;
        };
        moved
            .iter()
            .filter_map(|id| guard.widgets.get(id).cloned())
            .collect()
    };
    for rec in &records {
        app.emit("floaty-widget-updated", rec).ok();
    }
    log_line(
        app,
        &format!(
            "{why}: brought {} floatie(s) back onto a screen that still exists [{}]",
            moved.len(),
            moved.join(", ")
        ),
    );
    moved
}


/// Fit the desktop to the screens that exist now, and bring back anything that was
/// left off-screen. The Diagnostics tab's button: the same work a display change
/// does, for a user who would rather press something than restart.
#[tauri::command]
pub(crate) fn floaty_rehome_floaties(app: AppHandle) -> Result<usize, String> {
    Ok(reconcile_desktop(&app).len())
}

/// Where a dragged floatie is now, in the space records live in.
///
/// The page works out the place and sends it: it knows the pointer's physical position
/// (its own screen's origin plus the event's css px times its own scale factor) and the
/// screens it can map that through. Deliberately *not* built on `cursor_position()` —
/// measured on the mixed-DPI pair, that answers in a different space from window
/// geometry (a pointer physically at 4000,667 came back as 2000,333), so a mapping
/// built on it puts floaties in the wrong place while looking plausible.
///
/// Sent by the window that holds the pointer — which, thanks to pointer capture, is
/// still the window the drag started in even while the pointer is over another screen.
/// The record moves; when that puts it on a different screen the overlays are told, so
/// the widget is handed from the window it left to the window it entered *mid-drag*
/// rather than vanishing at the boundary and reappearing on release.
/// A dragged floatie's place, sent to every overlay window on each move.
///
/// The window a drag *started* in keeps the pointer (the capture is there), while the
/// window that *draws* the floatie may be the other one — so the place has to be pushed
/// to both. `floaty-widget-updated` is no good for this: it re-mounts the floatie, which
/// on every move would be a rebuild per frame.
#[derive(Clone, serde::Serialize)]
pub(crate) struct DragMoved {
    pub(crate) id: String,
    pub(crate) x: i32,
    pub(crate) y: i32,
}

#[derive(serde::Serialize)]
pub(crate) struct DragOwner {
    pub(crate) index: Option<usize>,
    pub(crate) screen: Option<String>,
    pub(crate) x: f64,
    pub(crate) y: f64,
}

#[tauri::command]
pub(crate) fn floaty_drag_to(id: String, x: f64, y: f64, app: AppHandle) -> DragOwner {
    let owner = drag_record_to(&app, &id, x, y);
    let list = screens(&app);
    DragOwner {
        index: owner,
        x: x.round(),
        y: y.round(),
        screen: owner.map(|i| list[i].name.clone()),
    }
}

/// Move a record during a drag and hand it over when that changes which screen it is on.
///
/// The handover is the part that matters: the window the floatie left unmounts it and
/// the one it entered mounts it, which only works because a record's owner is *derived*
/// from its position rather than stored.
/// The place a dragged floatie started from, until its drop is dealt with.
///
/// The record's own position cannot answer this: a drag writes it on every move — that is
/// how the floatie follows the pointer — so by the time a drop happens the "before" has
/// been overwritten many times over. Undo has to put the icon back on the spot the user
/// took it from, which is what undoing a move means, so the first move of a drag is where
/// it is remembered.
pub(crate) static DRAG_ORIGIN: std::sync::Mutex<Option<(String, i32, i32)>> = std::sync::Mutex::new(None);

/// Remember where this floatie was, once per drag.
pub(crate) fn remember_drag_origin(app: &AppHandle, id: &str) {
    let already = {
        let slot = match DRAG_ORIGIN.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        slot.as_ref().is_some_and(|(oid, _, _)| oid == id)
    };
    if already {
        return;
    }
    // read the position *before* taking the other lock: two locks held at once in two
    // orders is how a deadlock gets written
    let origin = {
        let state = app.state::<AppState>();
        let guard = match state.0.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.widgets.get(id).map(|rec| (rec.x, rec.y))
    };
    if let Some((x, y)) = origin {
        let mut slot = match DRAG_ORIGIN.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *slot = Some((id.to_string(), x, y));
    }
}

/// Where the drag that is in flight started, for this id.
pub(crate) fn drag_origin(id: &str) -> Option<(i32, i32)> {
    let slot = match DRAG_ORIGIN.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    slot.as_ref()
        .filter(|(oid, _, _)| oid == id)
        .map(|(_, x, y)| (*x, *y))
}

/// A drag is over (dropped, merged, or abandoned): the remembered place is spent.
pub(crate) fn forget_drag_origin(id: &str) {
    let mut slot = match DRAG_ORIGIN.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if slot.as_ref().is_some_and(|(oid, _, _)| oid == id) {
        *slot = None;
    }
}

/// Does the remembered origin belong to this record? Only the one the drag moved.
///
/// A merge checkpoints two records — the dragged item and the folder it went into — and
/// only the first of them moved. Writing the origin over both is what put a user's folder
/// on the spot their file had come from, so the rule is a function with a test rather than
/// a condition buried in a loop.
pub(crate) fn origin_applies(dragged: &str, id: &str) -> bool {
    dragged == id
}

/// A checkpoint that records *the dragged* record as it was before the drag.
///
/// `checkpoint` reads the live records, which after a drag is the place the drag left them
/// in. Undo of a move has to land on the old place, so the remembered origin is written
/// over that one record's position.
///
/// The id in `origin` matters: a merge checkpoints two records — the dragged item and the
/// folder it went into — and only the first of them moved. Writing the origin over both put
/// the folder on the dragged item's old spot, which is what a user sees as "undoing put my
/// folder where the file was".
pub(crate) fn checkpoint_with_origin(
    app: &AppHandle,
    label: &str,
    ids: &[&str],
    origin: Option<(&str, i32, i32)>,
) {
    let restore: Vec<undo::Restore> = {
        let state = app.state::<AppState>();
        let guard = match state.0.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        ids.iter()
            .map(|id| undo::Restore {
                id: (*id).to_string(),
                record: guard.widgets.get(*id).map(|rec| {
                    let mut value = serde_json::to_value(rec).unwrap_or(serde_json::Value::Null);
                    // only the record the drag moved: the others keep their live position
                    if let (Some((dragged, x, y)), Some(map)) = (origin, value.as_object_mut()) {
                        if origin_applies(dragged, id) {
                            map.insert("x".into(), serde_json::json!(x));
                            map.insert("y".into(), serde_json::json!(y));
                        }
                    }
                    value
                }),
            })
            .collect()
    };
    undo::push(label, restore);
}

/// A gesture is starting: snapshot exactly the records it is about to move.
///
/// Called on pointerdown — the only moment the "before" exists, because a drag writes the
/// record on every move. The rule this serves is at the top of `undo.rs`.
#[tauri::command]
pub(crate) fn floaty_gesture_begin(label: String, ids: Vec<String>, app: AppHandle) {
    let restore: Vec<undo::Restore> = {
        let state = app.state::<AppState>();
        let guard = match state.0.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        ids.iter()
            .map(|id| undo::Restore {
                id: id.clone(),
                record: guard.widgets.get(id).and_then(|rec| serde_json::to_value(rec).ok()),
            })
            .collect()
    };
    let label = if label.trim().is_empty() {
        "move".to_string()
    } else {
        label.trim().to_string()
    };
    undo::begin_gesture(&label, restore);
}

/// The gesture is over: one step for what it actually changed, or no step at all.
#[tauri::command]
pub(crate) fn floaty_gesture_end(app: AppHandle) -> Option<String> {
    // A gesture is the last chance to notice that it ended somewhere nothing draws. No
    // overlay window covers the band between two screens at different scales — the desktop's
    // own void — so a widget left there is drawn by nobody: invisible *and* unclickable, gone
    // until floaty is restarted. Tiles re-home at the drop (`floaty_dropped_inner`); panels
    // release through here instead and never did.
    //
    // Before the commit, not after: the step this gesture pushes has to hold the position the
    // widget actually ends at. Committing first would leave the step's forward state pointing
    // into the void, and redo would put the widget back where nothing can see it.
    rehome_stranded(&app, "gesture");
    let pushed = undo::commit_gesture(&|id| {
        let state = app.state::<AppState>();
        let guard = state.0.lock().ok()?;
        guard.widgets.get(id).and_then(|rec| serde_json::to_value(rec).ok())
    });
    if let Some(label) = pushed.as_deref() {
        persist(&app);
        log_line(&app, &format!("gesture: '{label}' is undoable now"));
    }
    pushed
}

/// A drag that ended without a merge still moved a floatie: that is undoable too.
pub(crate) fn record_move(app: &AppHandle, id: &str, origin: Option<(i32, i32)>) {
    let Some((ox, oy)) = origin else {
        return;
    };
    let now = {
        let state = app.state::<AppState>();
        let guard = match state.0.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.widgets.get(id).map(|rec| (rec.x, rec.y))
    };
    // a drop that landed where it started is not a change worth an undo step
    if now.is_none() || now == Some((ox, oy)) {
        return;
    }
    checkpoint_with_origin(app, "move", &[id], Some((id, ox, oy)));
}

pub(crate) fn drag_record_to(app: &AppHandle, id: &str, x: f64, y: f64) -> Option<usize> {
    remember_drag_origin(app, id);
    let list = screens(app);
    let (mut nx, mut ny) = (x.round(), y.round());
    let mut handover: Option<WidgetRecord> = None;
    let mut stray = false;
    {
        let state = app.state::<AppState>();
        let Ok(mut guard) = state.0.lock() else {
            return None;
        };
        let rec = guard.widgets.get_mut(id)?;
        // A position on no screen is only ever a moment *inside* a drag. The band between two
        // screens at different scales belongs to neither, and a drag crossing it has to be
        // able to pass through — but with no gesture open nothing is holding the pointer, so
        // this is where the widget would *stay*, and a widget on no screen is drawn by nobody:
        // invisible and unclickable. Measured: the page's own release placed the widget after
        // the gesture had ended, so its off-screen position landed on top of the re-home and
        // the log claimed a repair that did not survive it. Refuse the write and put the
        // widget back where a window can draw it.
        if screens::screen_of(&list, nx, ny).is_none() && !undo::gesture_pending() {
            stray = true;
        } else {
            // A drag may leave a *screen* — the band between two screens has to stay
            // crossable — but not the *desktop*: past the outermost edge there is no window at
            // all, so a widget dragged out there is drawn by nobody. The whole rectangle is
            // clamped, not the top-left: `screen_of` answers for the corner, which is how a
            // widget's body could hang over the right edge with only its corner inside.
            let (w, h) = plugins::size(&rec.kind, &rec.data);
            let (cx, cy) = screens::clamp_into(screens::union(&list), nx, ny, w, h, 0.0);
            nx = cx;
            ny = cy;
            let was = screens::screen_of(&list, rec.x as f64, rec.y as f64);
            let now = screens::screen_of(&list, nx, ny);
            rec.x = nx as i32;
            rec.y = ny as i32;
            if was != now {
                handover = Some(rec.clone());
            }
        }
    }
    if stray {
        log_line(
            app,
            &format!("drag: {id} -> {nx},{ny} is on no screen with no drag open — refusing it"),
        );
        rehome_stranded(app, "stray move");
        return None;
    }
    // Debounced by `persist`: a drag that ends off its own screen is not lost to a hard
    // kill before the release path runs.
    persist(app);
    if let Some(rec) = handover {
        app.emit("floaty-widget-updated", rec).ok();
    }
    // ...and on *every* move, not only when the owner changes: once the floatie has been
    // handed to the other window, that window is the one drawing it, and it learns where
    // the pointer is from here (measured: without this the floatie was handed over and
    // then stood still for the rest of the drag, with the button still down).
    app.emit(
        "floaty-drag-moved",
        DragMoved {
            id: id.to_string(),
            x: nx as i32,
            y: ny as i32,
        },
    )
    .ok();
    screens::screen_of(&list, nx, ny)
}

/// Open the folder the log lives in, which is what a bug report asks for next.
#[tauri::command]
pub(crate) fn floaty_open_log_folder(app: AppHandle) -> Result<(), String> {
    let dir = app_data_dir(&app);
    launch_target(&app, &dir.to_string_lossy())
}
