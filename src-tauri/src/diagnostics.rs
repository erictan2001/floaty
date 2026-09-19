//! What the app can say about itself: the log, the monitor inventory, the
//! heartbeat table, and the sizes on disk.
//!
//! Two things shaped it:
//!
//! - **The log rotates.** It used to be capped by writing an empty file once it
//!   passed 100KB, which is a *deletion* — and the evidence for whatever had just
//!   happened went with it (measured, twice in one session: the line that said
//!   why a window was missing was gone by the time it was read). Now `floaty.log`
//!   rolls into `floaty.log.1..3`, so the last few megabytes survive.
//! - **Every number here is one a bug report has needed**: which monitors exist
//!   and at what scale, where each window *actually* is (against where it thinks
//!   it is), whether a page still believes it is hidden, and how big the things on
//!   disk have grown.
//!
//! The globals stay in `lib.rs`; this module only touches the filesystem and
//! describes what it is handed.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// One file's worth of log before it rolls over, and how many to keep: the live
/// file plus `.1` … `.3`, so about 4MB of history.
pub const LOG_BYTES: u64 = 1_000_000;
const LOG_KEEP: usize = 3;

/// How much of the log the report carries back.
const TAIL_LINES: usize = 250;

#[derive(Debug, Clone, Serialize)]
pub struct LogInfo {
    pub path: String,
    pub bytes: u64,
    /// The most recent lines, oldest first.
    pub lines: Vec<String>,
    /// How many rolled-over files exist beside it.
    pub rotated: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileInfo {
    pub path: String,
    pub bytes: u64,
    pub backups: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DirInfo {
    pub path: String,
    pub files: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MonitorInfo {
    /// The device name, which is what stays put across replugs (`\\.\DISPLAY1`).
    pub key: String,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub primary: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowInfo {
    pub label: String,
    /// What the *app* did with it last: hidden windows are meant to be hidden.
    pub visible: bool,
    /// Where the window is, in physical px, and where its page thinks it is.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub layer: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BeatInfo {
    pub label: String,
    pub visibility: String,
    pub ms_ago: u64,
}

/// A label the heartbeat repair has tried to fix, and how many times.
#[derive(Debug, Clone, Serialize)]
pub struct RepairInfo {
    pub label: String,
    pub attempts: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub version: String,
    pub mode: String,
    pub os: String,
    pub started_ms_ago: u64,
    pub log: LogInfo,
    pub store: FileInfo,
    pub icons: DirInfo,
    pub records: u64,
    pub installed_plugins: Vec<String>,
    pub rejected_plugins: Vec<String>,
    /// "on" | "off" | "unknown" — the display state the app is tracking.
    pub display: String,
    pub heartbeats: Vec<BeatInfo>,
    pub repairs: Vec<RepairInfo>,
    pub monitors: Vec<MonitorInfo>,
    pub windows: Vec<WindowInfo>,
}

/// `<log>` beside `<log>.1`, `<log>.2` … Without `Path::with_extension`, which
/// would swap `floaty.log` for `floaty.1` and leave the log where it was.
pub fn rolled(path: &Path, n: usize) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{n}"));
    PathBuf::from(name)
}

pub fn should_roll(bytes: u64) -> bool {
    bytes > LOG_BYTES
}

/// Push the live log back one, dropping the oldest: `log` → `.1` → `.2` → `.3`.
pub fn roll(path: &Path) {
    let _ = std::fs::remove_file(rolled(path, LOG_KEEP));
    for n in (1..LOG_KEEP).rev() {
        let _ = std::fs::rename(rolled(path, n), rolled(path, n + 1));
    }
    let _ = std::fs::rename(path, rolled(path, 1));
}

/// How many rolled-over files are sitting beside the live log.
pub fn rolled_count(path: &Path) -> u64 {
    (1..=LOG_KEEP).filter(|n| rolled(path, *n).exists()).count() as u64
}

/// The last `lines` lines, oldest first, and the file's size.
pub fn read_log(path: &Path) -> (Vec<String>, u64) {
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let Ok(text) = std::fs::read_to_string(path) else {
        return (Vec::new(), bytes);
    };
    let all = text.lines();
    let count = all.clone().count();
    let lines = all
        .skip(count.saturating_sub(TAIL_LINES))
        .map(|line| line.to_string())
        .collect();
    (lines, bytes)
}

/// Files and bytes under a directory, one level or many. Symlinks are not
/// followed: the icon folder is flat now, but a junction in it must not send this
/// off into the filesystem.
pub fn dir_size(dir: &Path) -> (u64, u64) {
    let mut files = 0;
    let mut bytes = 0;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            let (f, b) = dir_size(&entry.path());
            files += f;
            bytes += b;
        } else if kind.is_file() {
            files += 1;
            bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    (files, bytes)
}

/// `"1.8 MB"` — the report is read by people.
pub fn human_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let value = bytes as f64;
    if value < KB {
        format!("{bytes} B")
    } else if value < KB * KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{:.2} MB", value / (KB * KB))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("floaty-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn rolling_keeps_the_last_few_files_and_never_loses_the_newest() {
        let dir = temp("roll");
        let log = dir.join("floaty.log");
        // four generations; the oldest two fall off
        for n in 0..4 {
            std::fs::write(&log, format!("generation {n}\n")).unwrap();
            roll(&log);
        }
        assert!(!log.exists(), "the live file is the one that moved");
        assert_eq!(
            std::fs::read_to_string(rolled(&log, 1)).unwrap(),
            "generation 3\n"
        );
        assert_eq!(
            std::fs::read_to_string(rolled(&log, 2)).unwrap(),
            "generation 2\n"
        );
        assert_eq!(
            std::fs::read_to_string(rolled(&log, 3)).unwrap(),
            "generation 1\n"
        );
        assert!(!rolled(&log, 4).exists(), "nothing older is kept");
        assert_eq!(rolled_count(&log), 3);
        // ...and the names are siblings of the log, not a mangled extension
        assert!(rolled(&log, 1).to_string_lossy().ends_with("floaty.log.1"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_rolled_log_is_read_back_oldest_line_first() {
        let dir = temp("tail");
        let log = dir.join("floaty.log");
        let text: String = (0..400).map(|n| format!("line {n}\n")).collect();
        std::fs::write(&log, text).unwrap();
        let (lines, bytes) = read_log(&log);
        assert!(bytes > 0);
        assert_eq!(lines.len(), TAIL_LINES, "only the tail comes back");
        assert_eq!(lines.last().unwrap(), "line 399");
        assert_eq!(
            lines.first().unwrap(),
            &format!("line {}", 400 - TAIL_LINES),
            "and it starts where the tail starts"
        );
        // a missing file is an empty report, not a panic
        assert_eq!(read_log(&dir.join("nothing.log")).0.len(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_size_of_a_folder_counts_the_files_under_it() {
        let dir = temp("size");
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("a.png"), vec![0u8; 10]).unwrap();
        std::fs::write(dir.join("nested/b.png"), vec![0u8; 5]).unwrap();
        let (files, bytes) = dir_size(&dir);
        assert_eq!(files, 2);
        assert_eq!(bytes, 15);
        assert_eq!(dir_size(&dir.join("missing")), (0, 0));
        assert_eq!(human_bytes(15), "15 B");
        assert_eq!(human_bytes(2048), "2.0 KB");
        assert_eq!(human_bytes(3 * 1024 * 1024), "3.00 MB");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_live_log_rolls_only_once_it_is_over_the_limit() {
        assert!(!should_roll(LOG_BYTES));
        assert!(should_roll(LOG_BYTES + 1));
    }
}
