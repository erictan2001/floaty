//! `diagnostics` — moved out of lib.rs verbatim.
//!
//! Visibility is `pub(crate)` because the rest of the crate calls in across the module
//! boundary now; nothing here changed shape on the way out.

use crate::*;
use std::fs;
use tauri::{AppHandle, Emitter, Manager};

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

    let records = store::with(&app, |s| s.count() as u64);

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
        installed_plugins: plugins::labels(&installed),
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
/// The first two follow the report at once — a floatie is only drawn where an overlay
/// covers the screen, so the windows cannot wait for anything. The third does not: it *is*
/// the arrangement of the desktop, and Windows reports a shape it does not stay in more
/// often than that sounds like. Measured, in this machine's own log: `0,0 960x1440` at
/// 14:01:39, `0,0 1440x960` again at 14:01:42 (a fullscreen app changing the mode and
/// changing it back). Rearranging for that is what moved nineteen pinned floaties onto a
/// screen shape that stopped existing two seconds later, and left them in the pile the
/// second pass clamped them into — the "the layout is messed up after I turn the machine
/// off and on" bug report. So the rearrangement waits for the shape to stand still (see
/// `ARRANGEMENT_SETTLE_MS`), and the windows do not.
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
    // what cannot wait: the windows, and the pages' idea of the layout
    fit_desktop(&app);
    // what must: note the shape, and let the watch below rearrange for it once it has
    // stood still. A newer report replaces this one, which is the whole mechanism.
    if let Ok(mut pending) = PENDING_ARRANGEMENT.lock() {
        note_arrangement(&mut pending, arrangement_now(&app), now);
    }
}

/// The arrangement as one value, read from the platform right now.
pub(crate) fn arrangement_now(app: &AppHandle) -> String {
    screens::arrangement(screens::union(&screens(app)))
}

/// How long the arrangement has to stand still before the desktop is rearranged for it.
///
/// Three seconds: longer than the measured blip (the wrong shape was reported at
/// 14:01:39.850 and corrected at 14:01:42.364 — 2.5s), and shorter than a person's patience
/// with a floatie that is not on screen yet.
pub(crate) const ARRANGEMENT_SETTLE_MS: u64 = 3_000;

/// An arrangement Windows reported, and when it reported it.
pub(crate) struct ArrangementReport {
    pub fingerprint: String,
    pub at: u64,
}

/// The report the desktop is waiting on, if one is. One per app.
pub(crate) static PENDING_ARRANGEMENT: std::sync::Mutex<Option<ArrangementReport>> =
    std::sync::Mutex::new(None);

/// Note a report. A newer report replaces the older one: what is being waited for is the
/// shape that is here *now*, and a shape that has been superseded is never acted on.
pub(crate) fn note_arrangement(
    pending: &mut Option<ArrangementReport>,
    fingerprint: String,
    at: u64,
) {
    *pending = Some(ArrangementReport { fingerprint, at });
}

/// What may be acted on now: the arrangement that has stood still for `settle_ms`, and
/// nothing at all while the newest report is still fresh. Taking it spends it, so one
/// report is one rearrangement however often the watch ticks.
pub(crate) fn settled_arrangement(
    pending: &mut Option<ArrangementReport>,
    now: u64,
    settle_ms: u64,
) -> Option<String> {
    let report = pending.as_ref()?;
    if now.saturating_sub(report.at) < settle_ms {
        return None;
    }
    let fingerprint = report.fingerprint.clone();
    *pending = None;
    Some(fingerprint)
}

/// Rearrange the desktop for an arrangement once it has stopped changing.
///
/// One thread for the whole app, ticking twice a second: a report is a moment, and what a
/// person sees is the shape that is still there a few seconds later.
pub(crate) fn start_arrangement_watch() {
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let Some(app) = shared_app() else { continue };
        let due = match PENDING_ARRANGEMENT.lock() {
            Ok(mut pending) => {
                settled_arrangement(&mut pending, elapsed_ms(), ARRANGEMENT_SETTLE_MS)
            }
            Err(_) => None,
        };
        let Some(fingerprint) = due else { continue };
        // The shape the report was about may have moved on since (nothing stops the display
        // changing again without saying so). A rearrangement is for the arrangement that is
        // here now, or for none at all.
        if arrangement_now(&app) != fingerprint {
            continue;
        }
        // Say so either way. A re-home that finds nothing says nothing, and then a log with
        // no "brought ..." lines reads the same whether the arrangement was rearranged for
        // and found whole or was never looked at at all — which is the shape of every wrong
        // theory about this path so far.
        let moved = rehome_stranded(&app, "monitors");
        if moved.is_empty() {
            log_line(
                &app,
                &format!("monitors: {fingerprint} settled, nothing to bring back"),
            );
        }
    });
}

/// Fit everything to the arrangement that exists now: the overlay windows, the pages
/// that hold a cached layout, and any floatie sitting off every screen.
///
/// Four callers and one implementation: `WM_DISPLAYCHANGE`, startup, the button in the
/// Diagnostics tab — and the palette when it moves itself to another screen, which is
/// also how this gets exercised without unplugging anything.
pub(crate) fn reconcile_desktop(app: &AppHandle) -> Vec<String> {
    fit_desktop(app);
    rehome_stranded(app, "monitors")
}

/// The half of an arrangement change that cannot wait: the overlays on the screens that
/// exist, and the pages told to re-read the layout they hold (drag bounds, and the floor a
/// falling icon rests on). Nothing here writes a record, so doing it at once is safe —
/// which is the point, because until it is done an overlay is the wrong size and every
/// floatie inside it is drawn in the wrong place.
fn fit_desktop(app: &AppHandle) -> screens::Rect {
    if let Err(e) = spawn_overlay_windows(app) {
        log_line(app, &format!("monitors: could not build the overlays: {e}"));
    }
    fit_overlay_to_monitor(app);
    close_extra_overlays(app);
    let union = screens::union(&screens(app));
    log_line(
        app,
        &format!("monitors: the desktop is now {}", screens::arrangement(union)),
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
    union
}

/// Bring back every floatie whose screen is gone, and say how many.
///
/// Two ways to end up off-screen, and both are silent: a display is unplugged while the
/// desktop covers it (the arrangement shrinks and the record keeps its old coordinates),
/// or the app starts after that happened. Either way the record is `pinned`, so the physics
/// will never move it, and nothing else in the app has an opinion about position — so
/// without this it is simply not on the screen any more.
/// Measured: two screens at 2.00/1.50, second one unplugged, and the floaties that had
/// been dragged there never came back.
///
/// What a place is taken *away* for is remembered on the record (`screens::Displaced`)
/// whenever the arrangement is what moved it, so the arrangement it belongs to can have it
/// back — a monitor replugged, a rotation undone, a shape the platform reported and then
/// took back. This is the difference between "the desktop is rearranged" and "the desktop
/// is rearranged and then it is *that* desktop, permanently".
pub(crate) fn rehome_stranded(app: &AppHandle, why: &str) -> Vec<String> {
    let list = screens(app);
    if list.is_empty() {
        return Vec::new();
    }
    // The callers that follow the *arrangement* remember what they move. The others are a
    // single widget a drag ended somewhere nothing draws: it never had a place for an
    // arrangement to come back to, so remembering one would invent a place it never chose.
    let remember = matches!(why, "monitors" | "startup");
    let moved = store::with(app, |s| rehome_into(s, &list, remember));
    if moved.is_empty() {
        return moved;
    }
    let records: Vec<WidgetRecord> =
        store::with(app, |s| moved.iter().filter_map(|id| s.get(id).cloned()).collect());
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

/// The re-home itself, over records and screens and nothing else — no app, no window, no
/// disk — so the one rule that matters can be a test rather than a reading of a log line.
///
/// `remember` is whether the place each move takes away is written down for the arrangement
/// it was chosen for (see `screens::Displaced`).
pub(crate) fn rehome_into(
    s: &mut store::StoreData,
    list: &[screens::Screen],
    remember: bool,
) -> Vec<String> {
    let here = screens::arrangement(screens::union(list));
    // Places that were given up for an arrangement that is not this one come home first:
    // they are then part of what the passes below lay themselves around.
    let mut moved: Vec<String> = restore_places(s, list, &here);

    // What is already on each screen, so a floatie coming back lands *beside* its
    // neighbours rather than under them.
    let mut taken: Vec<Vec<screens::Rect>> = vec![Vec::new(); list.len()];
    for rec in s.iter() {
        let (x, y) = (rec.x as f64, rec.y as f64);
        if let Some(index) = screens::screen_of(list, x, y) {
            let (w, h) = plugins::size(&rec.kind, &rec.data);
            taken[index].push(screens::Rect::new(x, y, w, h));
        }
    }

    // Top-down, left-to-right: with several coming back at once, which one ends up
    // where should not depend on a hash map's iteration order.
    let mut stranded: Vec<(i32, i32, String, String, serde_json::Value)> = s
        .iter()
        .filter(|rec| screens::screen_of(list, rec.x as f64, rec.y as f64).is_none())
        .map(|rec| {
            (rec.y, rec.x, rec.id.clone(), rec.kind.clone(), rec.data.clone())
        })
        .collect();
    stranded.sort_by_key(|(y, x, _, _, _)| (*y, *x));

    for (_, _, id, kind, data) in stranded {
        let Some(rec) = s.get(&id) else {
            continue;
        };
        let (x, y) = (rec.x as f64, rec.y as f64);
        let Some(index) = screens::nearest(list, x, y) else {
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
        let (to_x, to_y) = (nx as i32, ny as i32);
        let here = here.clone();
        s.edit(&id, |rec| {
            if remember {
                remember_place(rec, &here, to_x, to_y);
            }
            rec.x = to_x;
            rec.y = to_y;
        });
        moved.push(id);
    }

    // And whatever is left sticking out of the screen it is on comes back too. Its
    // top-left was on the screen and its body was not, and an overlay window *is* one
    // screen, so the part past the edge was drawn by nobody — invisible, and unclickable
    // wherever it overlapped nothing. Flush is allowed: this is a clamp, not a re-home,
    // so a widget a person pushed to the edge stays where they put it, just wholly on the
    // screen. A place a *shape* could not hold is remembered like any other: the shape that
    // made it stick out is exactly the one that is not here any more.
    // Picked out first, then written: the store hands out one record at a time, and
    // this pass is a read to decide and a write to apply.
    let flush: Vec<(String, i32, i32)> = s
        .iter()
        .filter_map(|rec| {
            let (w, h) = plugins::size(&rec.kind, &rec.data);
            if screens::within_one_screen(list, rec.x as f64, rec.y as f64, w, h) {
                return None;
            }
            let (nx, ny) =
                screens::confine(list, rec.x as f64, rec.y as f64, w, h, 0.0)?;
            Some((rec.id.clone(), nx as i32, ny as i32))
        })
        .collect();
    for (id, nx, ny) in flush {
        let here = here.clone();
        s.edit(&id, |rec| {
            if remember {
                remember_place(rec, &here, nx, ny);
            }
            rec.x = nx;
            rec.y = ny;
        });
        moved.push(id);
    }
    moved
}

/// Give back the places that were taken away for an arrangement that is not this one.
///
/// Runs before anything is moved for this arrangement, so a place that comes home is part
/// of what the passes below lay themselves around. The memory is dropped with the place it
/// held: a floatie that has come back is not waiting for anything.
fn restore_places(s: &mut store::StoreData, list: &[screens::Screen], here: &str) -> Vec<String> {
    let back: Vec<(String, i32, i32)> = s
        .iter()
        .filter_map(|rec| {
            let displaced = screens::Displaced::from_data(&rec.data)?;
            let (w, h) = plugins::size(&rec.kind, &rec.data);
            if !displaced.returns(here, list, rec.x, rec.y, w, h) {
                return None;
            }
            Some((rec.id.clone(), displaced.x, displaced.y))
        })
        .collect();
    for (id, x, y) in &back {
        s.edit(id, |rec| {
            rec.x = *x;
            rec.y = *y;
            if let Some(map) = rec.data.as_object_mut() {
                map.remove(screens::DISPLACED);
            }
        });
    }
    back.into_iter().map(|(id, _, _)| id).collect()
}

/// Write down the place a move is taking away, so the arrangement it was chosen for can
/// have it back.
///
/// A second displacement in a row keeps the *original* place: a floatie moved twice before
/// its arrangement returns belongs wherever it was before the first move, not in the pile
/// the first one put it in. A record that no longer stands where the last move left it has
/// been moved by the user since, and the place they chose is the newer truth.
fn remember_place(rec: &mut WidgetRecord, arrangement: &str, to_x: i32, to_y: i32) {
    let home = screens::Displaced::from_data(&rec.data)
        .filter(|d| d.to_x == rec.x && d.to_y == rec.y)
        .map(|d| (d.x, d.y))
        .unwrap_or((rec.x, rec.y));
    let displaced = screens::Displaced {
        arrangement: arrangement.to_string(),
        x: home.0,
        y: home.1,
        to_x,
        to_y,
    };
    if let Some(map) = rec.data.as_object_mut() {
        map.insert(screens::DISPLACED.to_string(), displaced.to_json());
    }
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
    let origin = store::with(app, |s| s.get(id).map(|rec| (rec.x, rec.y)));
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
    let restore: Vec<undo::Restore> = store::with(app, |s| {
        ids.iter()
            .map(|id| undo::Restore {
                id: (*id).to_string(),
                record: s.get(id).map(|rec| {
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
    });
    undo::push(label, restore);
}

/// A gesture is starting: snapshot exactly the records it is about to move.
///
/// Called on pointerdown — the only moment the "before" exists, because a drag writes the
/// record on every move. The rule this serves is at the top of `undo.rs`.
#[tauri::command]
pub(crate) fn floaty_gesture_begin(label: String, ids: Vec<String>, app: AppHandle) {
    let restore: Vec<undo::Restore> = store::with(&app, |s| {
        ids.iter()
            .map(|id| undo::Restore {
                id: id.clone(),
                record: s.get(id).and_then(|rec| serde_json::to_value(rec).ok()),
            })
            .collect()
    });
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
        store::with(&app, |s| s.get(id).and_then(|rec| serde_json::to_value(rec).ok()))
    });
    if let Some(label) = pushed.as_deref() {
        log_line(&app, &format!("gesture: '{label}' is undoable now"));
    }
    pushed
}

/// A drag that ended without a merge still moved a floatie: that is undoable too.
pub(crate) fn record_move(app: &AppHandle, id: &str, origin: Option<(i32, i32)>) {
    let Some((ox, oy)) = origin else {
        return;
    };
    let now = store::with(app, |s| s.get(id).map(|rec| (rec.x, rec.y)));
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
    // The record has to be there before anything else is judged: a drag for an id the store
    // no longer holds is simply not a drag, and used to leave here before the stray check
    // below — no log line, no re-home.
    let missing = store::with(app, |s| -> bool {
        if s.get(id).is_none() {
            return true;
        }
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
            return false;
        }
        s.edit(id, |rec| {
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
        });
        false
    });
    if missing {
        return None;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this module exists for, in one sentence: *the icons' layout is inconsistent —
    /// turn the machine off for a long time, reopen it, and the layout is messed up.*
    ///
    /// It is not a guess. This machine's log has the whole thing, at 14:01 on the day of the
    /// report: `monitors: the desktop is now 0,0 960x1440` at 14:01:39.850, then
    /// `monitors: brought 19 floatie(s) back onto a screen that still exists [...]` in the
    /// same second — every name in that list is one of the icons the trail had arranged on
    /// the right half of the screen — and then `the desktop is now 0,0 1440x960` again at
    /// 14:01:42.364, with `brought 15` more behind it. Nothing was plugged in: a fullscreen
    /// app changed the display mode and changed it back, and the desktop was rearranged for
    /// the shape in the middle.
    ///
    /// A floatie's place is `pinned`, so physics never moves it and nothing else in the app
    /// has an opinion about position: what a re-home does is permanent by construction. That
    /// is what makes a two-second lie about the screen shape into a permanently wrong
    /// layout.

    /// One screen, in the space records are stored in.
    fn screen(w: f64, h: f64) -> screens::Screen {
        screens::Screen {
            key: "\\\\.\\DISPLAY1".into(),
            name: "\\\\.\\DISPLAY1".into(),
            physical: screens::Rect::new(0.0, 0.0, w * 2.0, h * 2.0),
            logical: screens::Rect::new(0.0, 0.0, w, h),
            scale: 2.0,
            primary: true,
        }
    }

    /// The shape the machine is really in, and the one it was wrongly reported as.
    fn landscape() -> Vec<screens::Screen> {
        vec![screen(1440.0, 960.0)]
    }

    fn portrait() -> Vec<screens::Screen> {
        vec![screen(960.0, 1440.0)]
    }

    fn placed(s: &mut store::StoreData, id: &str, kind: &str, x: i32, y: i32) {
        s.put(store::WidgetRecord {
            id: id.into(),
            kind: kind.into(),
            x,
            y,
            // A place a person made: pinned, so nothing but a re-home may move it.
            data: serde_json::json!({ "pinned": true, "arranged": true }),
        });
    }

    /// The desktop from the trace: the icons the trail had arranged across the right of the
    /// screen (`folder-18`, `app-16`, `file-32` … are the names in the log's own list), two
    /// that were in the middle of it, and one panel whose body reaches past the middle.
    fn arranged_desktop() -> store::StoreData {
        let mut s = store::StoreData::default();
        for (id, kind, x, y) in [
            ("app-16", "app", 1071, 10),
            ("app-23", "app", 1306, 10),
            ("file-32", "file", 1318, 116),
            ("folder-18", "folder", 1075, 147),
            ("folder-25", "folder", 966, 145),
            ("file-41", "file", 1277, 587),
            ("folder-80", "folder", 1310, 483),
            ("folder-30", "folder", 1215, 677),
            ("folder-29", "folder", 1279, 838),
            ("folder-77", "folder", 1011, 724),
            ("folder-36", "folder", 1120, 728),
            ("app-20", "app", 1021, 838),
            ("folder-15", "folder", 90, 228),
            ("folder-31", "folder", 48, 329),
            ("file-69", "file", 268, 122),
            // A panel: on the portrait screen by its corner, off it by its body.
            ("note-1", "note", 900, 300),
        ] {
            placed(&mut s, id, kind, x, y);
        }
        s
    }

    fn places(s: &store::StoreData) -> Vec<(String, i32, i32)> {
        let mut out: Vec<(String, i32, i32)> =
            s.iter().map(|r| (r.id.clone(), r.x, r.y)).collect();
        out.sort();
        out
    }

    /// The ones nothing draws: not wholly on any screen that exists.
    fn off_every_screen(s: &store::StoreData, list: &[screens::Screen]) -> Vec<String> {
        let mut out: Vec<String> = s
            .iter()
            .filter(|r| {
                let (w, h) = plugins::size(&r.kind, &r.data);
                !screens::within_one_screen(list, r.x as f64, r.y as f64, w, h)
            })
            .map(|r| r.id.clone())
            .collect();
        out.sort();
        out
    }

    /// The captured trace, replayed the way the display-change path runs it: reports arrive
    /// when they arrived, the watch ticks twice a second between them, and the desktop is
    /// rearranged for a shape only once that shape has stopped changing.
    ///
    /// Returns `(shapes acted on, floaties moved)` — the first says the shape that stood
    /// still was not simply never looked at, the second is the damage.
    ///
    /// The clock is driven rather than slept on, so this is the same code the app runs and
    /// still finishes in milliseconds.
    fn replay(
        desktop: &mut store::StoreData,
        trace: &[(Vec<screens::Screen>, u64)],
    ) -> (usize, usize) {
        let mut pending = None;
        let mut acted = 0usize;
        let mut moved = 0usize;
        let last = trace.iter().map(|(_, at)| *at).max().unwrap_or(0);
        let mut arriving = 0usize;
        for tick in 0..=(last + 2 * ARRANGEMENT_SETTLE_MS) / 500 {
            let now = tick * 500;
            // what Windows said, when it said it
            while arriving < trace.len() && trace[arriving].1 <= now {
                let (list, at) = &trace[arriving];
                note_arrangement(&mut pending, screens::arrangement(screens::union(list)), *at);
                arriving += 1;
            }
            let Some(shape) = settled_arrangement(&mut pending, now, ARRANGEMENT_SETTLE_MS) else {
                continue;
            };
            // and what the machine is actually in at that moment
            let now_list: Vec<screens::Screen> = trace
                .iter()
                .rev()
                .find(|(_, at)| *at <= now)
                .map(|(list, _)| list.clone())
                .unwrap_or_default();
            if screens::arrangement(screens::union(&now_list)) != shape {
                continue;
            }
            acted += 1;
            moved += rehome_into(desktop, &now_list, true).len();
        }
        (acted, moved)
    }

    #[test]
    fn a_desktop_is_not_rearranged_for_a_shape_that_does_not_stay() {
        let mut desktop = arranged_desktop();
        let before = places(&desktop);
        assert_eq!(
            off_every_screen(&desktop, &landscape()),
            Vec::<String>::new(),
            "the fixture is a desktop that fits the shape it is on"
        );

        // 14:01:39 — a report of a shape the display is not going to stay in
        // 14:01:42 — the same display, 1440x960 again
        let (acted, moved) = replay(&mut desktop, &[(portrait(), 0), (landscape(), 2_500)]);

        assert_eq!(
            moved, 0,
            "the desktop is not moved for a shape that was reported and taken back"
        );
        assert_eq!(
            acted, 1,
            "... and the shape that did stand still was acted on, once"
        );
        assert_eq!(
            places(&desktop),
            before,
            "the desktop is where the user put it"
        );
    }

    #[test]
    fn a_place_belongs_to_the_arrangement_it_was_chosen_for() {
        let mut desktop = arranged_desktop();
        let before = places(&desktop);

        // A shape that stays — a screen really is portrait for a while, a dock is
        // unplugged — moves everything onto it, whole: nothing is left where nothing draws
        // it, and that is what the re-home is for.
        rehome_into(&mut desktop, &portrait(), true);
        assert_eq!(
            off_every_screen(&desktop, &portrait()),
            Vec::<String>::new(),
            "every floatie is drawn by an overlay again"
        );
        assert_ne!(places(&desktop), before, "a portrait screen is not this desktop's shape");

        // ... and when the arrangement it *was* chosen for comes back, so do the places.
        // Without this the four seconds of a fullscreen app changing mode are permanent.
        rehome_into(&mut desktop, &landscape(), true);
        assert_eq!(
            places(&desktop),
            before,
            "the place belongs to the arrangement it was chosen for"
        );
    }

    #[test]
    fn a_place_the_user_moved_since_is_not_given_back() {
        let mut desktop = arranged_desktop();
        rehome_into(&mut desktop, &portrait(), true);

        // The user drags one of the displaced floaties somewhere they want it. That place
        // is the newer truth, and the memory of the old one is spent.
        let (id, x, y) = desktop
            .iter()
            .find(|r| r.id == "folder-29")
            .map(|r| (r.id.clone(), r.x, r.y))
            .expect("folder-29 is on the desktop");
        assert!(x != 1279 || y != 838, "it was displaced for the portrait shape");
        desktop.edit(&id, |rec| {
            rec.x = 160;
            rec.y = 300;
        });

        rehome_into(&mut desktop, &landscape(), true);
        let rec = desktop.get(&id).expect("still there");
        assert_eq!(
            (rec.x, rec.y),
            (160, 300),
            "the place the user chose is not undone by an arrangement coming back"
        );
    }
}

