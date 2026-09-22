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

#[derive(Default)]
pub(crate) struct StoreData {
    pub(crate) widgets: HashMap<String, WidgetRecord>,
    pub(crate) next: u64,
    /// ids removed at runtime (remove / folder-merge): late saves from dying
    /// windows must not resurrect them
    pub(crate) dead: HashSet<String>,
}

pub(crate) struct AppState(pub(crate) Mutex<StoreData>);

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
