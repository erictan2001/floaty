//! `store` — moved out of lib.rs verbatim.
//!
//! Visibility is `pub(crate)` because the rest of the crate calls in across the module
//! boundary now; nothing here changed shape on the way out.

use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

// ---------- store ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WidgetRecord {
    pub(crate) id: String,
    pub(crate) kind: String,
    /// logical px (frontend converts via scaleFactor; builder takes logical)
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) data: serde_json::Value,
}

/// The desktop's records, and the rules for changing them.
///
/// Callers used to reach in: take the mutex, edit a map, drop the guard, then remember
/// to `persist`. That four-step protocol was repeated at every mutation site, so two
/// rules travelled with whoever happened to remember them — a removed id must not come
/// back (the tombstone set), and no change is durable until someone marks the store
/// dirty. Both live here now: `with` takes the lock once, the methods below keep the
/// tombstone and the dirty mark honest, and the maps are private to this module.
#[derive(Default)]
pub(crate) struct StoreData {
    widgets: HashMap<String, WidgetRecord>,
    next: u64,
    /// ids removed at runtime (remove / folder-merge): late saves from dying
    /// windows must not resurrect them
    dead: HashSet<String>,
    /// set by every method that changed something, read and cleared by `with`
    touched: bool,
}

impl StoreData {
    /// One record, by id.
    pub(crate) fn get(&self, id: &str) -> Option<&WidgetRecord> {
        self.widgets.get(id)
    }

    /// Every record.
    pub(crate) fn iter(&self) -> std::collections::hash_map::Values<'_, String, WidgetRecord> {
        self.widgets.values()
    }

    /// Take the records that were on disk back into memory, before anything is on
    /// screen. This is the store *loading*, not the store changing: it deliberately does
    /// not mark the store dirty, so a launch that reclassifies nothing leaves the 3MB
    /// file alone.
    pub(crate) fn load_records(&mut self, records: Vec<WidgetRecord>) {
        for rec in records {
            self.widgets.insert(rec.id.clone(), rec);
        }
    }

    /// How many records there are.
    pub(crate) fn count(&self) -> usize {
        self.widgets.len()
    }

    /// Whether this id was removed and must not come back.
    pub(crate) fn is_dead(&self, id: &str) -> bool {
        self.dead.contains(id)
    }

    /// Add or replace a record. A record that is here is live, so the tombstone goes:
    /// that is what makes undo's restore (and a folder-merge's re-add) work.
    pub(crate) fn put(&mut self, record: WidgetRecord) {
        self.dead.remove(&record.id);
        self.widgets.insert(record.id.clone(), record);
        self.touched = true;
    }

    /// Take a record out, and tombstone its id so a late save cannot bring it back.
    pub(crate) fn remove(&mut self, id: &str) -> Option<WidgetRecord> {
        let out = self.widgets.remove(id);
        self.dead.insert(id.to_string());
        self.touched = true;
        out
    }

    /// Change one record in place. `None` when there is no such record, and no change
    /// to write.
    pub(crate) fn edit<R>(
        &mut self,
        id: &str,
        f: impl FnOnce(&mut WidgetRecord) -> R,
    ) -> Option<R> {
        let out = self.widgets.get_mut(id).map(f);
        if out.is_some() {
            self.touched = true;
        }
        out
    }

    /// The number a new record gets.
    pub(crate) fn next_number(&mut self) -> u64 {
        self.next += 1;
        self.touched = true;
        self.next
    }

    /// The counter as it stands, without taking a number.
    pub(crate) fn next_counter(&self) -> u64 {
        self.next
    }

    /// Move the counter forward, past the ids already on disk (and back, when a merge
    /// or an undo puts a batch of them back).
    pub(crate) fn set_next(&mut self, n: u64) {
        self.next = n;
    }
}

/// The store's mutex, as Tauri-managed state. Private to this module: the rest of the
/// crate crosses at `with`, never at the lock.
pub(crate) struct AppState(Mutex<StoreData>);

/// The state `lib.rs` hands to Tauri at startup.
pub(crate) fn new_state() -> AppState {
    AppState(Mutex::new(StoreData::default()))
}

/// Take the records, change them, and have the change written.
///
/// The lock is taken once and released when `f` returns; the store is marked dirty only
/// if something in here changed it, and the background writer does the writing. Nothing
/// inside `f` may call back into the store — the mutex is not reentrant — so a caller
/// that needs a second look reads through `get`/`iter` or returns a value out.
pub(crate) fn with<R>(app: &AppHandle, f: impl FnOnce(&mut StoreData) -> R) -> R {
    let state = app.state::<AppState>();
    let mut guard = state.0.lock().unwrap_or_else(|e| e.into_inner());
    let out = f(&mut guard);
    if guard.touched {
        guard.touched = false;
        persist(app);
    }
    out
}

/// The app's own data folder (`%APPDATA%\com.floaty.app`), created if missing.
/// The store, the settings and the icons folder all live here, so they must all
/// agree on where that is.
pub(crate) fn app_data_dir(app: &AppHandle) -> std::path::PathBuf {
    let dir = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("floaty"));
    fs::create_dir_all(&dir).ok();
    dir
}

pub(crate) fn store_file(app: &AppHandle) -> std::path::PathBuf {
    app_data_dir(app).join("floaty-store.json")
}

/// Where stored icons live: a PNG per distinct icon, addressed by content.
pub(crate) fn icons_dir(app: &AppHandle) -> std::path::PathBuf {
    app_data_dir(app).join("icons")
}

/// Write a text file atomically, keeping the previous contents as `<name>.bak`.
///
/// The store used to be written in place: being killed mid-write left a
/// truncated JSON file, which then loaded as "no widgets at all" — that is how
/// the desktop sometimes came back empty.
pub(crate) fn write_text_atomic(path: &std::path::Path, text: &str) {
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, text).is_err() {
        return;
    }
    if fs::rename(&tmp, path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

/// Seconds between backup rotations while the app is running.
pub(crate) const BACKUP_EVERY_SECS: u64 = 60;
pub(crate) static LAST_BACKUP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Refresh `<name>.bak` from the current file. Rotated on a timer (and once at
/// startup) rather than on every save: a session that came up on an empty store
/// used to write its freshly re-synced contents straight over the only good
/// copy, which turned a recoverable crash into a permanent loss.
pub(crate) fn refresh_backup(path: &std::path::Path, force: bool) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if !force {
        let last = LAST_BACKUP.load(std::sync::atomic::Ordering::Relaxed);
        if last != 0 && now.saturating_sub(last) < BACKUP_EVERY_SECS {
            return;
        }
    }
    LAST_BACKUP.store(now, std::sync::atomic::Ordering::Relaxed);
    if path.exists() {
        let _ = fs::copy(path, path.with_extension("bak"));
    }
}

/// Set while a write is outstanding; the writer clears it before writing.
pub(crate) static STORE_DIRTY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Mark the store dirty and let the background writer do the work.
///
/// `floaty_save` used to serialise and write the whole store itself — ~3MB of
/// JSON with every icon base64-inlined. A single remount (every widget reports
/// the position the layout just gave it) is sixty of those, and on the main
/// thread that is a measured **18.8 seconds** of the app not answering, which is
/// the "Not Responding" the user sees after a wake-up or a restart. Coalescing
/// turns the sixty writes into one.
pub(crate) fn persist(app: &AppHandle) {
    STORE_DIRTY.store(true, std::sync::atomic::Ordering::SeqCst);
    static WRITER: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    WRITER.get_or_init(|| {
        std::thread::spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_millis(300));
            if !STORE_DIRTY.swap(false, std::sync::atomic::Ordering::SeqCst) {
                continue;
            }
            if let Some(app) = shared_app() {
                write_store_now(&app);
            }
        });
    });
    let _ = app;
}

/// Write the store immediately, on this thread. Used by the background writer and
/// by the exit path, which cannot wait for a timer.
pub(crate) fn write_store_now(app: &AppHandle) {
    // Serialize under the lock, write after releasing it: holding the store
    // mutex across file IO serialized every save/list/create and stalled the
    // settings window while widgets persisted positions in the background.
    let json = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().unwrap_or_else(|e| e.into_inner());
        let list: Vec<&WidgetRecord> = guard.widgets.values().collect();
        serde_json::to_string_pretty(&list).unwrap_or_else(|_| "[]".to_string())
    };
    let store = store_file(app);
    let started = std::time::Instant::now();
    let bytes = json.len();
    write_text_atomic(&store, &json);
    // never let an empty desktop become the backup
    if json.len() > 4 {
        refresh_backup(&store, false);
    }
    let ms = started.elapsed().as_millis();
    if ms > 120 {
        log_line(
            app,
            &format!(
                "store: wrote {:.1}MB in {}ms (this is the main thread; it used to happen                  once per widget, which is what made the app stop answering)",
                bytes as f64 / (1024.0 * 1024.0),
                ms
            ),
        );
    }
}

/// Parse a JSON file, falling back to its `.bak` copy when the file is missing
/// **or its JSON is damaged**. Returns the value and whether the backup was
/// needed — a truncated write must not be mistaken for "nothing to load".
pub(crate) fn read_json_with_backup<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
) -> Option<(T, bool)> {
    if let Ok(text) = fs::read_to_string(path) {
        if !text.trim().is_empty() {
            if let Ok(value) = serde_json::from_str::<T>(&text) {
                return Some((value, false));
            }
        }
    }
    let bak = path.with_extension("bak");
    if let Ok(text) = fs::read_to_string(&bak) {
        if let Ok(value) = serde_json::from_str::<T>(&text) {
            return Some((value, true));
        }
    }
    None
}

/// Keep the damaged file instead of throwing it away, so a bad write is still
/// diagnosable after the fact.
pub(crate) fn quarantine(path: &std::path::Path) {
    if path.exists() {
        let _ = fs::rename(path, path.with_extension("corrupt"));
    }
}

pub(crate) fn load_all(app: &AppHandle) -> Vec<WidgetRecord> {
    let path = store_file(app);
    match read_json_with_backup::<Vec<WidgetRecord>>(&path) {
        Some((list, from_backup)) => {
            if from_backup {
                log_line(
                    app,
                    &format!("store unreadable; recovered {} widgets from backup", list.len()),
                );
                quarantine(&path);
                if let Ok(json) = serde_json::to_string_pretty(&list) {
                    write_text_atomic(&path, &json);
                }
            }
            list
        }
        None => {
            if path.exists() {
                log_line(app, "store and backup both unreadable; desktop starts empty");
                quarantine(&path);
            }
            vec![]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str) -> WidgetRecord {
        WidgetRecord {
            id: id.to_string(),
            kind: "note".to_string(),
            x: 0,
            y: 0,
            data: serde_json::json!({ "text": "hi" }),
        }
    }

    /// The rule the tombstone exists for: a window that saves a record after its floatie
    /// was removed (a folder-merge, a delete) must not bring it back.
    #[test]
    fn a_removed_id_is_dead_and_putting_it_back_revives_it() {
        let mut store = StoreData::default();
        store.put(rec("note-1"));
        assert!(!store.is_dead("note-1"));

        let gone = store.remove("note-1");
        assert!(gone.is_some(), "remove hands the record back");
        assert!(store.get("note-1").is_none());
        assert!(store.is_dead("note-1"), "a late save must be refused");

        // undo of a remove puts it back, and the id is live again
        store.put(rec("note-1"));
        assert!(!store.is_dead("note-1"));
        assert_eq!(store.count(), 1);
    }

    /// The dirty mark is what decides whether a 3MB file gets written: looking must not
    /// set it, and every real change must.
    #[test]
    fn only_a_change_marks_the_store_dirty() {
        let mut store = StoreData::default();
        store.put(rec("note-1"));
        store.touched = false;

        let _ = store.get("note-1");
        let _ = store.iter().count();
        let _ = store.count();
        let _ = store.is_dead("note-1");
        store.set_next(7);
        assert!(!store.touched, "reading is not a change");
        assert_eq!(store.next_counter(), 7);

        store.put(rec("note-1"));
        assert!(store.touched, "a put is a change");
        store.touched = false;

        assert!(store.edit("note-1", |r| r.x = 120).is_some());
        assert!(store.touched, "an edit is a change");
        assert_eq!(store.get("note-1").unwrap().x, 120);
        store.touched = false;

        assert!(store.remove("note-1").is_some());
        assert!(store.touched, "a remove is a change");
    }

    /// `edit` is the only way to touch a record in place, so it has to say whether there
    /// was one — callers turn `None` into "widget not found".
    #[test]
    fn edit_answers_whether_the_record_was_there() {
        let mut store = StoreData::default();
        assert!(store.edit("nothing-here", |r| r.x = 1).is_none());
        assert!(!store.touched, "no record, nothing to write");

        store.put(rec("note-1"));
        store.touched = false;
        let out = store.edit("note-1", |r| {
            r.y = 40;
            "done"
        });
        assert_eq!(out, Some("done"));
        assert_eq!(store.get("note-1").unwrap().y, 40);
    }

    /// Loading the store is not changing it: a launch that reclassifies nothing must
    /// leave the file alone (that is why this is not `put` in a loop).
    #[test]
    fn loading_the_records_from_disk_is_not_a_change() {
        let mut store = StoreData::default();
        store.load_records(vec![rec("note-1"), rec("clock-2")]);
        assert_eq!(store.count(), 2);
        assert!(!store.touched);
    }

    /// The counter belongs to the store: ids must not repeat after a restart, and a
    /// merge or an undo may have to move it.
    #[test]
    fn the_counter_is_taken_and_moved_by_the_store() {
        let mut store = StoreData::default();
        assert_eq!(store.next_number(), 1);
        assert_eq!(store.next_number(), 2);
        assert_eq!(store.next_counter(), 2);

        store.set_next(9);
        assert_eq!(store.next_number(), 10);
    }
}
