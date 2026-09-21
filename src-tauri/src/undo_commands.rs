//! `undo` — moved out of lib.rs verbatim.
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

// ---------- undo ----------

/// Remember the records an action is about to change, so one press can put them
/// back. Call it *before* the change: `undo::add_disk` and `undo::add_created`
/// fill in the rest while the action runs.
pub(crate) fn checkpoint(app: &AppHandle, label: &str, ids: &[&str]) {
    let restore: Vec<undo::Restore> = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().unwrap_or_else(|e| e.into_inner());
        ids.iter()
            .map(|id| undo::Restore {
                id: (*id).to_string(),
                record: guard
                    .widgets
                    .get(*id)
                    .and_then(|r| serde_json::to_value(r).ok()),
            })
            .collect()
    };
    let depth = undo::push(label, restore);
    log_line(
        app,
        &format!("undo: '{label}' can be undone ({depth} on the stack)"),
    );
}

/// Would this step change anything?
///
/// A command that pushed a checkpoint and then refused before touching the store
/// leaves a step that would make the undo button look dead — it would be applied
/// and do nothing. Those are dropped instead, and the press moves on to the next
/// step, so "undo" never means "nothing happened".
pub(crate) fn step_changes_anything(guard: &StoreData, step: &undo::Step) -> bool {
    if !step.disk.is_empty() {
        return true;
    }
    step.restore.iter().any(|r| {
        let now = guard
            .widgets
            .get(&r.id)
            .and_then(|rec| serde_json::to_value(rec).ok());
        now != r.record
    })
}

/// Apply one thing an action did to the disk, *forward* — what a redo does.
///
/// The mirror of `reverse_disk`, and the two share their operations: a `Move` applies
/// `from` -> `to` here and `to` -> `from` there, so a step means the same thing in both
/// directions and only the way it is applied differs.
pub(crate) fn apply_disk(op: &undo::DiskOp) -> Result<String, String> {
    match op.action {
        undo::DiskAction::Unrecycle => {
            let path = std::path::Path::new(&op.from);
            if !path.exists() {
                return Err(format!("'{}' is not there to remove again", op.from));
            }
            match shell_ops::recycle_checked(path) {
                Ok(shell_ops::Recycled::ToBin) => Ok(format!("'{}' to the Recycle Bin", op.from)),
                // The shell silently hard-deletes where there is no usable bin, so say what
                // actually happened rather than claiming a bin move (see `shell_ops::recycle`).
                Ok(shell_ops::Recycled::Permanently) => Ok(format!(
                    "'{}' deleted again (this machine has no usable Recycle Bin)",
                    op.from
                )),
                Err(err) => Err(format!("'{}': {err}", op.from)),
            }
        }
        undo::DiskAction::Move => {
            let from = std::path::Path::new(&op.from);
            let to = std::path::Path::new(&op.to);
            if !from.exists() {
                return Err(format!("'{}' is not there to move", op.from));
            }
            if to.exists() {
                return Err(format!("'{}' is in the way", op.to));
            }
            std::fs::rename(from, to).map_err(|e| format!("'{}' -> '{}': {e}", op.from, op.to))?;
            Ok(format!("'{}' -> '{}'", op.from, op.to))
        }
        undo::DiskAction::Discard => {
            // The action created this, so redo has to make it again. Only a directory can be
            // made again: a shortcut floaty wrote into a folder has no stored target, and
            // inventing one would put the wrong file on the desktop.
            let path = std::path::Path::new(&op.from);
            if path.exists() {
                return Ok(format!("'{}' is there again already", op.from));
            }
            if !op.dir {
                return Err(format!(
                    "'{}' cannot be made again: floaty wrote it as a shortcut or a copy, and only the folder a grouping makes can be rebuilt",
                    op.from
                ));
            }
            std::fs::create_dir_all(path).map_err(|e| format!("'{}': {e}", op.from))?;
            Ok(format!("'{}' made again", op.from))
        }
    }
}

/// Reverse one thing an action did to the disk.
pub(crate) fn reverse_disk(op: &undo::DiskOp) -> Result<String, String> {
    match op.action {
        undo::DiskAction::Unrecycle => {
            shell_ops::restore_from_bin(&op.from)?;
            Ok(format!("'{}' back from the Recycle Bin", op.from))
        }
        undo::DiskAction::Move => {
            let from = std::path::Path::new(&op.from);
            let to = std::path::Path::new(&op.to);
            if !to.exists() {
                return Err(format!("'{}' is not there to move back", op.to));
            }
            if from.exists() {
                return Err(format!("'{}' is in the way", op.from));
            }
            std::fs::rename(to, from)
                .map_err(|e| format!("'{}' -> '{}': {e}", op.to, op.from))?;
            Ok(format!("'{}' back to '{}'", op.to, op.from))
        }
        undo::DiskAction::Discard => {
            let path = std::path::Path::new(&op.from);
            if !path.exists() {
                return Ok(format!("'{}' was already gone", op.from));
            }
            if path.is_dir() {
                // Only if it is empty: an item that could not be moved back is a
                // reason to leave the folder alone, not to lose it.
                if std::fs::remove_dir(path).is_err() {
                    return Ok(format!("'{}' left in place (not empty)", op.from));
                }
            } else {
                std::fs::remove_file(path).map_err(|e| format!("'{}': {e}", op.from))?;
            }
            Ok(format!("'{}' taken away again", op.from))
        }
    }
}

/// The records a step names, as they are right now.
///
/// This is what the *other* direction has to restore: a step holds the state it goes to, so
/// the inverse of a step is the state it finds.
pub(crate) fn live_record(app: &AppHandle) -> impl Fn(&str) -> Option<serde_json::Value> + '_ {
    move |id: &str| {
        let state = app.state::<AppState>();
        let guard = state.0.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .widgets
            .get(id)
            .and_then(|rec| serde_json::to_value(rec).ok())
    }
}

/// Put a step's records into the store: the ones it names come back, the ones it created go
/// away. Returns how many of each, and the ids to re-emit whether they exist now or not.
///
/// Shared by undo and redo, because a step means the same thing in both directions — only
/// the state it carries differs.
pub(crate) fn apply_step_records(app: &AppHandle, step: &undo::Step) -> (usize, usize, Vec<String>) {
    let mut restored = 0usize;
    let mut removed = 0usize;
    let mut touched: Vec<String> = Vec::new();
    {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().unwrap_or_else(|e| e.into_inner());
        for entry in &step.restore {
            match &entry.record {
                Some(json) => match serde_json::from_value::<WidgetRecord>(json.clone()) {
                    Ok(rec) => {
                        // A restored record has to be allowed to live again: the tombstone
                        // is there to stop dying windows from resurrecting a removed
                        // widget, and this one is not dying, it is coming home.
                        guard.dead.remove(&rec.id);
                        guard.widgets.insert(rec.id.clone(), rec);
                        restored += 1;
                        touched.push(entry.id.clone());
                    }
                    Err(err) => log_line(
                        app,
                        &format!("step: {} could not be read back: {err}", entry.id),
                    ),
                },
                None => {
                    if guard.widgets.remove(&entry.id).is_some() {
                        removed += 1;
                    }
                    guard.dead.insert(entry.id.clone());
                    touched.push(entry.id.clone());
                }
            }
        }
    }
    (restored, removed, touched)
}

/// Rebuild exactly what changed. A remount covers both cases — a widget that was never on
/// screen and one that is, since the page unmounts first — and a folder takes its item list
/// with it.
pub(crate) fn emit_step_changes(app: &AppHandle, touched: &[String]) {
    let (now_here, now_gone): (Vec<WidgetRecord>, Vec<String>) = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().unwrap_or_else(|e| e.into_inner());
        touched.iter().fold(
            (Vec::new(), Vec::new()),
            |(mut here, mut gone), id| {
                match guard.widgets.get(id).cloned() {
                    Some(rec) => here.push(rec),
                    None => gone.push(id.clone()),
                }
                (here, gone)
            },
        )
    };
    for rec in &now_here {
        app.emit("floaty-widget-updated", rec).ok();
    }
    for id in &now_gone {
        hide_widget(app, id);
    }
}

/// Undo the last change: the records, and the files they stand for.
///
/// The files go first. If one of them cannot be put back — the Recycle Bin was
/// emptied in the meantime — the records are left exactly as they are and the
/// step stays on the stack, so restoring the file by hand in Explorer and
/// pressing undo again still works.
#[tauri::command]
pub(crate) async fn floaty_undo(app: AppHandle) -> Result<undo::UndoReport, String> {
    loop {
        let Some(step) = undo::pop() else {
            return Err("nothing left to undo".into());
        };
        {
            let state = app.state::<AppState>();
            let guard = state.0.lock().unwrap_or_else(|e| e.into_inner());
            if !step_changes_anything(&guard, &step) {
                log_line(
                    &app,
                    &format!("undo: '{}' would change nothing; skipped", step.label),
                );
                continue;
            }
        }

        // The state this undo is about to overwrite is the "after" of the action being
        // undone, and this is the only moment it exists: nothing recorded it when the action
        // ran. It becomes the step that redoes this one.
        let forward = undo::inverse_of(&step, &live_record(&app));

        let mut files: Vec<String> = Vec::new();
        for op in step.disk.iter().rev() {
            match reverse_disk(op) {
                Ok(what) => files.push(what),
                Err(err) => {
                    log_line(&app, &format!("undo: '{}' FAILED: {err}", step.label));
                    undo::restore_top(step);
                    return Err(err);
                }
            }
        }

        let (restored, removed, touched) = apply_step_records(&app, &step);
        persist(&app);
        emit_step_changes(&app, &touched);
        // Only after the work succeeded: a redo that could not finish leaves the future
        // unreachable rather than half-applied.
        undo::push_redo(forward);

        let remaining = undo::depth();
        log_line(
            &app,
            &format!(
                "undo: '{}' — {restored} record(s) back, {removed} removed, {} file move(s){}; {remaining} left to undo",
                step.label,
                files.len(),
                if files.is_empty() {
                    String::new()
                } else {
                    format!(": {}", files.join("; "))
                },
            ),
        );
        return Ok(undo::UndoReport {
            label: step.label,
            restored,
            removed,
            files,
            remaining,
        });
    }
}

/// What the next undo would reverse — and what the next redo would replay — for the
/// settings window's buttons.
#[tauri::command]
pub(crate) fn floaty_undo_state() -> undo::UndoState {
    undo::UndoState {
        depth: undo::depth(),
        label: undo::last_label(),
        redo_depth: undo::redo_depth(),
        redo_label: undo::redo_label(),
    }
}

/// Redo the last undone change: the files and the records, forward.
///
/// The mirror of `floaty_undo`, sharing both of its pieces: `apply_disk` the other way
/// round, and a step whose records are the state the action left behind — written by the
/// undo, because that is the only moment it exists. A new action clears the future
/// (`undo::push`), so a redo can never replay a step built on records that have since
/// changed.
#[tauri::command]
pub(crate) async fn floaty_redo(app: AppHandle) -> Result<undo::UndoReport, String> {
    loop {
        let Some(step) = undo::pop_redo() else {
            return Err("nothing left to redo".into());
        };
        {
            let state = app.state::<AppState>();
            let guard = state.0.lock().unwrap_or_else(|e| e.into_inner());
            if !step_changes_anything(&guard, &step) {
                log_line(
                    &app,
                    &format!("redo: '{}' would change nothing; skipped", step.label),
                );
                continue;
            }
        }

        // What this redo overwrites is the "before" of the action, and it goes back on the
        // undo stack — the two keys trade the same step back and forth.
        let back = undo::inverse_of(&step, &live_record(&app));

        let mut files: Vec<String> = Vec::new();
        // Forward order, which is what the action did: the folder a grouping made is created
        // before the items move into it (the undo walks the same list backwards).
        for op in step.disk.iter() {
            match apply_disk(op) {
                Ok(what) => files.push(what),
                Err(err) => {
                    log_line(&app, &format!("redo: '{}' FAILED: {err}", step.label));
                    undo::restore_redo_top(step);
                    return Err(err);
                }
            }
        }

        let (restored, removed, touched) = apply_step_records(&app, &step);
        persist(&app);
        emit_step_changes(&app, &touched);
        undo::push_from_redo(back);

        let remaining = undo::redo_depth();
        log_line(
            &app,
            &format!(
                "redo: '{}' — {restored} record(s) back, {removed} removed, {} file move(s){}; {remaining} left to redo",
                step.label,
                files.len(),
                if files.is_empty() {
                    String::new()
                } else {
                    format!(": {}", files.join("; "))
                },
            ),
        );
        return Ok(undo::UndoReport {
            label: step.label,
            restored,
            removed,
            files,
            remaining,
        });
    }
}

/// Let a page that is about to move widgets in bulk record a restore point —
/// how "tidy the desktop" and a trail arrangement get an undo. The records come
/// from the page because that is where the positions it is about to overwrite
/// are already in hand.
#[tauri::command]
pub(crate) fn floaty_undo_checkpoint(label: String, restore: Vec<undo::Restore>, app: AppHandle) -> usize {
    let depth = undo::push(&label, restore);
    log_line(
        &app,
        &format!("undo: checkpoint '{label}' from the desktop ({depth} on the stack)"),
    );
    depth
}

#[tauri::command]
pub(crate) fn floaty_remove(id: String, app: AppHandle) {
    log_line(&app, &format!("remove {id}"));
    checkpoint(&app, "remove", &[&id]);
    let state: State<'_, AppState> = app.state::<AppState>();
    if let Ok(mut guard) = state.0.lock() {
        guard.widgets.remove(&id);
        guard.dead.insert(id.clone());
    }
    persist(&app);
    close_widget_async(&app, &id);
}
