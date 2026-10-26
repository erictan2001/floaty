//! `drag_commands` — where a dragged floatie is, and who owns the gesture.
//!
//! Moved out of `diagnostics_commands.rs` verbatim.

use crate::*;
use tauri::{AppHandle, Emitter};

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
pub(crate) static DRAG_ORIGIN: std::sync::Mutex<Option<(String, i32, i32)>> =
    std::sync::Mutex::new(None);

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
        store::with(&app, |s| {
            s.get(id).and_then(|rec| serde_json::to_value(rec).ok())
        })
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
