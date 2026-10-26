//! `arrangement` — the display-change side of the diagnostics commands.
//!
//! The windows follow a display change at once; the desktop waits for the shape
//! to stand still. Moved out of `diagnostics_commands.rs` verbatim.

use crate::*;
use tauri::{AppHandle, Emitter};

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
        &format!(
            "monitors: the desktop is now {}",
            screens::arrangement(union)
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
    let records: Vec<WidgetRecord> = store::with(app, |s| {
        moved.iter().filter_map(|id| s.get(id).cloned()).collect()
    });
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
            (
                rec.y,
                rec.x,
                rec.id.clone(),
                rec.kind.clone(),
                rec.data.clone(),
            )
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
            let (nx, ny) = screens::confine(list, rec.x as f64, rec.y as f64, w, h, 0.0)?;
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
