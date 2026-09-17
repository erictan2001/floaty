//! Undo: what floaty changed, remembered well enough to put it back.
//!
//! The operations worth undoing are the ones that touch *the disk* — removing a
//! floatie recycles the file, pulling an item out of a folder moves it, dropping
//! one icon onto another moves files into a new folder and rewrites two records.
//! The rest (a drag, a tidy) is positions, which a widget knows how to restore
//! because it is the one that computed them.
//!
//! So a step is two lists and a name:
//!
//! - **records** — the widget records as they were, or `None` for an id the
//!   action created (`add_created`). Only the ids the action touches, never the
//!   whole store: an undo must not teleport a widget the user moved an hour ago.
//! - **disk** — file moves to reverse, newest first when it is applied. An empty
//!   destination means "this was recycled", which undoes through the Recycle Bin.
//!
//! The stack is deliberately small and deliberately in memory: a step is a
//! promise that this session can still put something back, and a promise that
//! only holds while the files it names are where it left them.

use serde::{Deserialize, Serialize};
use std::sync::{LazyLock, Mutex};

/// One record to put back (`Some`), or one that should not exist (`None`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Restore {
    pub id: String,
    /// The record exactly as it was, or `None` when the id did not exist yet.
    pub record: Option<serde_json::Value>,
}

/// What an action did to a path, because undoing it differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskAction {
    /// It was recycled: undo puts it back out of the Recycle Bin (`from`).
    Unrecycle,
    /// It moved `from` -> `to`: undo moves it back.
    Move,
    /// It created `from` (a folder made by grouping, a shortcut written into a
    /// folder): undo takes it away again.
    Discard,
}

/// One thing an action did to the disk, to reverse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskOp {
    pub action: DiskAction,
    pub from: String,
    pub to: String,
}

impl DiskOp {
    /// The file was recycled (`from`).
    pub fn recycled(path: &str) -> Self {
        Self {
            action: DiskAction::Unrecycle,
            from: path.to_string(),
            to: String::new(),
        }
    }

    /// The file was moved from `from` to `to`.
    pub fn moved(from: &str, to: &str) -> Self {
        Self {
            action: DiskAction::Move,
            from: from.to_string(),
            to: to.to_string(),
        }
    }

    /// The action created `path`.
    pub fn discard(path: &str) -> Self {
        Self {
            action: DiskAction::Discard,
            from: path.to_string(),
            to: String::new(),
        }
    }
}

/// One undoable action.
#[derive(Debug, Clone)]
pub struct Step {
    pub label: String,
    pub restore: Vec<Restore>,
    pub disk: Vec<DiskOp>,
}

/// How far back a session can reach. Small on purpose: these are the last few
/// things the user did, not a journal, and every step holds records in memory.
const MAX_STEPS: usize = 16;

static STACK: LazyLock<Mutex<Vec<Step>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Remember what an action is about to change. Returns the new depth, so the
/// caller can log it.
pub fn push(label: &str, restore: Vec<Restore>) -> usize {
    let mut stack = lock();
    stack.push(Step {
        label: label.to_string(),
        restore,
        disk: Vec::new(),
    });
    while stack.len() > MAX_STEPS {
        stack.remove(0);
    }
    stack.len()
}

/// Note a file move on the newest step (the one this command just pushed).
pub fn add_disk(op: DiskOp) {
    if let Some(step) = lock().last_mut() {
        step.disk.push(op);
    }
}

/// Note that an id did not exist before the action, so undoing it removes the id.
pub fn add_created(id: &str) {
    if let Some(step) = lock().last_mut() {
        step.restore.push(Restore {
            id: id.to_string(),
            record: None,
        });
    }
}

pub fn pop() -> Option<Step> {
    lock().pop()
}

/// Put a step back on top — how an undo that could not finish (a file the bin no
/// longer holds) stays available instead of being lost.
pub fn restore_top(step: Step) {
    lock().push(step);
}

pub fn depth() -> usize {
    lock().len()
}

pub fn last_label() -> Option<String> {
    lock().last().map(|s| s.label.clone())
}

/// Forget everything. Tests need it; nothing in the app does, because a step
/// that has been applied is popped and a step that has not is still true.
#[cfg(test)]
pub fn clear() {
    lock().clear();
}

fn lock() -> std::sync::MutexGuard<'static, Vec<Step>> {
    STACK.lock().unwrap_or_else(|e| e.into_inner())
}

/// What one undo did, for the log and for settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoReport {
    /// The action that was reversed, in the words of whatever recorded it.
    pub label: String,
    /// Records put back.
    pub restored: usize,
    /// Records taken away again (ones the action created).
    pub removed: usize,
    /// Files that moved back, described for a human.
    pub files: Vec<String>,
    /// Steps left on the stack.
    pub remaining: usize,
}

/// What the UI shows on the undo control.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoState {
    pub depth: usize,
    /// The action the next undo would reverse.
    pub label: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn restore(id: &str) -> Restore {
        Restore {
            id: id.to_string(),
            record: Some(serde_json::json!({ "id": id, "kind": "app", "x": 10, "y": 20 })),
        }
    }

    #[test]
    fn the_last_action_is_the_first_one_undone() {
        clear();
        push("delete", vec![restore("app-1")]);
        push("tidy", vec![restore("app-2")]);
        assert_eq!(depth(), 2);
        assert_eq!(last_label().as_deref(), Some("tidy"));

        let step = pop().unwrap();
        assert_eq!(step.label, "tidy");
        assert_eq!(step.restore[0].id, "app-2");
        assert_eq!(pop().unwrap().label, "delete");
        assert!(pop().is_none());
        assert_eq!(last_label(), None);
    }

    #[test]
    fn disk_moves_and_created_ids_attach_to_the_step_that_asked_for_them() {
        clear();
        push("merge", vec![restore("app-1"), restore("app-2")]);
        add_disk(DiskOp::moved("C:\\d\\a.lnk", "C:\\d\\New folder\\a.lnk"));
        add_disk(DiskOp::discard("C:\\d\\New folder"));
        add_created("folder-9");
        push("tidy", vec![restore("app-3")]);
        // the newest step is untouched by what the older one recorded
        assert!(pop().unwrap().disk.is_empty());

        let step = pop().unwrap();
        assert_eq!(step.disk.len(), 2, "the move and the folder it created");
        assert_eq!(step.disk[0].action, DiskAction::Move);
        assert_eq!(step.disk[1].action, DiskAction::Discard);
        assert_eq!(step.restore.len(), 3, "two records and the folder it made");
        assert!(step.restore[2].record.is_none(), "a created id has no 'before'");
    }

    #[test]
    fn the_stack_forgets_the_oldest_step_rather_than_growing_for_ever() {
        clear();
        for i in 0..(MAX_STEPS + 5) {
            push(&format!("step {i}"), vec![restore("app-1")]);
        }
        assert_eq!(depth(), MAX_STEPS);
        assert_eq!(last_label().as_deref(), Some("step 20"));
    }

    #[test]
    fn a_step_that_could_not_be_applied_stays_available() {
        clear();
        push("delete", vec![restore("app-1")]);
        let step = pop().unwrap();
        assert_eq!(depth(), 0);
        restore_top(step);
        assert_eq!(depth(), 1);
        assert_eq!(last_label().as_deref(), Some("delete"));
    }

    #[test]
    fn a_frontend_checkpoint_round_trips_through_json() {
        // This is exactly what `floaty_undo_checkpoint` is handed: the records as
        // the page holds them, which is why `Restore` has to deserialize as well
        // as serialize.
        let payload = serde_json::json!([
            { "id": "app-1", "record": { "id": "app-1", "kind": "app", "x": 5, "y": 6, "data": {} } },
            { "id": "note-2", "record": serde_json::Value::Null },
        ]);
        let parsed: Vec<Restore> = serde_json::from_value(payload).unwrap();
        assert_eq!(parsed.len(), 2);
        assert!(parsed[0].record.is_some());
        assert!(parsed[1].record.is_none());
    }
}
