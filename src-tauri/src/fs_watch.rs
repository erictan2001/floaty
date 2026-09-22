//! Follow the pointed root on disk.
//!
//! The desktop is meant to be a *view* of that folder: add a file to it, rename
//! one in Explorer, drop something in — the desktop should follow, without
//! anyone pressing sync. This module is the "follow" half: it watches the root
//! (recursively, so a folder floatie's own contents count too) and reports that
//! something changed. It never decides *what* to draw: the caller re-runs the
//! same reconcile the sync button runs, which is what keeps this module from
//! having to know what a floatie is.
//!
//! One thread watches one root. `retarget` is the only entry point: it starts a
//! watcher for a new root, stops the one for the old, and does nothing when the
//! root has not changed and its thread is still alive. A root that is not there
//! (a drive that is unplugged, a folder mid-recreate) is waited for, not treated
//! as "stop watching".
//!
//! Implementation notes that matter:
//!
//! - The watch filters on **names only** (`FILE_NOTIFY_CHANGE_FILE_NAME` /
//!   `DIR_NAME`). Content churn would otherwise fire a reconcile per write, and
//!   nothing on the desktop depends on a file's contents — a root backed by
//!   OneDrive is exactly where that would have hurt.
//! - Events are debounced (a quiet period, with a cap) because one user action
//!   arrives as a burst and a copy into the folder arrives as hundreds.
//! - Renames are paired off the wire (`RENAMED_OLD_NAME` + `RENAMED_NEW_NAME`
//!   arrive adjacent) and handed over as pairs, so a floatie can *move* with its
//!   file — keeping its position and its icon — instead of being dropped and
//!   re-added as a new one.
//! - The read is issued overlapped against an event, so the thread notices the
//!   stop flag about once a second instead of blocking until the next change in
//!   a folder nobody touches any more.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::AppHandle;

/// A file that moved: where it was, and where it went. Absolute paths: Windows
/// reports the names it hands over *relative to the watched directory*, so this
/// module joins them with the root before anyone else sees them.
pub type Renames = Vec<(String, String)>;

/// Quiet period after the last event before the reconcile runs.
const QUIET: Duration = Duration::from_millis(700);
/// …and the longest a steady stream of events may defer it.
const MAX_WAIT: Duration = Duration::from_millis(2500);
/// How long the thread waits for an event before it checks the stop flag.
const POLL: u32 = 750;
/// How long to wait before retrying a root that is not there.
const RETRY: Duration = Duration::from_secs(5);

/// The root being watched, and the flag its thread watches.
struct Watch {
    pub(crate) root: String,
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) alive: Arc<AtomicBool>,
}

static CURRENT: Mutex<Option<Watch>> = Mutex::new(None);

/// Point the watcher at `root` (`None` = stop watching).
///
/// Cheap by design: called from settings changes, from every sync and from the
/// wake-up recovery, so it must not restart a healthy watcher.
pub fn retarget(root: Option<String>, app: AppHandle) {
    let wanted = root.map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
    let mut current = match CURRENT.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let already_watching = current.as_ref().is_some_and(|w| {
        Some(w.root.as_str()) == wanted.as_deref() && w.alive.load(Ordering::Relaxed)
    });
    if already_watching {
        return;
    }

    if let Some(old) = current.take() {
        old.stop.store(true, Ordering::Relaxed);
        crate::log_line(&app, &format!("watch: stopped watching '{}'", old.root));
    }

    let Some(root) = wanted else {
        return;
    };

    let stop = Arc::new(AtomicBool::new(false));
    let alive = Arc::new(AtomicBool::new(true));
    *current = Some(Watch {
        root: root.clone(),
        stop: stop.clone(),
        alive: alive.clone(),
    });
    drop(current);

    let worker = app.clone();
    std::thread::spawn(move || {
        watch_root(root, stop, worker);
        alive.store(false, Ordering::Relaxed);
    });
}

/// The app-facing half of the loop: log what happens, and hand every batch to
/// the reconcile (`crate::root_changed`).
fn watch_root(root: String, stop: Arc<AtomicBool>, app: AppHandle) {
    let mut log = |msg: &str| crate::log_line(&app, msg);
    let mut sink = |renames: Renames, touched: bool| {
        if touched || !renames.is_empty() {
            crate::root_changed(&app, &renames);
        }
    };
    watch_loop(&root, &stop, &mut sink, &mut log);
}

/// The loop itself: open the root (waiting for it to appear), read, debounce
/// what comes back, hand each burst to `sink`.
///
fn wait_for_retry(stop: &AtomicBool) -> bool {
    for _ in 0..(RETRY.as_millis() as u32 / 250) {
        if stop.load(Ordering::Relaxed) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

fn watch_dir_session(
    dir: &mut DirWatch,
    root: &str,
    stop: &AtomicBool,
    sink: &mut dyn FnMut(Renames, bool),
    log: &mut dyn FnMut(&str),
) -> bool {
    let mut started: Option<Instant> = None;
    let mut last = Instant::now();
    let mut renames: Renames = Vec::new();
    let mut touched = false;
    let mut overflowed = false;
    let mut pending_old: Option<String> = None;
    let root_path = std::path::PathBuf::from(root);

    loop {
        if stop.load(Ordering::Relaxed) {
            return true;
        }
        if dir.wait(POLL) {
            match dir.drain(&mut pending_old) {
                Ok(batch) => {
                    if batch.touched || !batch.renames.is_empty() {
                        let now = Instant::now();
                        started.get_or_insert(now);
                        last = now;
                        touched |= batch.touched;
                        overflowed |= batch.overflow;
                        renames.extend(batch.renames.into_iter().map(|(from, to)| {
                            let join =
                                |name: &str| root_path.join(name).to_string_lossy().to_string();
                            (join(&from), join(&to))
                        }));
                    }
                }
                Err(err) => {
                    log(&format!("watch: read failed ({err}) — re-arming"));
                    return false;
                }
            }
            if dir.issue().is_err() {
                return false;
            }
        }

        let due = started.is_some_and(|at| last.elapsed() >= QUIET || at.elapsed() >= MAX_WAIT);
        if due {
            let burst = std::mem::take(&mut renames);
            if overflowed {
                log("watch: change queue overflowed — reconciling the whole root");
            }
            sink(burst, touched);
            started = None;
            touched = false;
            overflowed = false;
        }
    }
}

/// Kept apart from `watch_root` so it can be exercised against a real temp
/// folder in a test, with no app handle anywhere near it. `sink` runs on this
/// thread and must be quick.
fn watch_loop(
    root: &str,
    stop: &AtomicBool,
    sink: &mut dyn FnMut(Renames, bool),
    log: &mut dyn FnMut(&str),
) {
    let mut reported_missing = false;
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let mut dir = match DirWatch::open(std::path::Path::new(root)) {
            Ok(dir) => {
                log(&format!("watch: following '{root}'"));
                reported_missing = false;
                dir
            }
            Err(err) => {
                if !reported_missing {
                    reported_missing = true;
                    log(&format!("watch: '{root}' is not available yet ({err})"));
                }
                if wait_for_retry(stop) {
                    return;
                }
                continue;
            }
        };

        if dir.issue().is_err() {
            std::thread::sleep(Duration::from_millis(250));
            continue;
        }

        if watch_dir_session(&mut dir, root, stop, sink, log) {
            return;
        }
    }
}

// ---------- the Windows watch itself ----------

/// What one batch of notifications said.
#[cfg(windows)]
struct Batch {
    /// Something was created or deleted: worth a reconcile.
    pub(crate) touched: bool,
    /// The queue held more than the buffer could: names are missing from this
    /// batch, so the reconcile it triggers has to be a full one.
    pub(crate) overflow: bool,
    /// Renames, paired old → new.
    pub(crate) renames: Renames,
}

#[cfg(windows)]
struct DirWatch {
    pub(crate) handle: windows::Win32::Foundation::HANDLE,
    pub(crate) event: windows::Win32::Foundation::HANDLE,
    pub(crate) buffer: Vec<u8>,
    pub(crate) overlapped: windows::Win32::System::IO::OVERLAPPED,
}

#[cfg(windows)]
impl DirWatch {
    fn open(root: &std::path::Path) -> windows::core::Result<Self> {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        };
        use windows::Win32::System::Threading::CreateEventW;

        let wide: Vec<u16> = root
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        // FILE_FLAG_BACKUP_SEMANTICS is what lets a *directory* be opened at all;
        // overlapped so the read can be waited on with a timeout.
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                None,
            )
        }?;

        let event = match unsafe { CreateEventW(None, true, false, PCWSTR::null()) } {
            Ok(event) => event,
            Err(err) => {
                unsafe {
                    let _ = CloseHandle(handle);
                }
                return Err(err);
            }
        };

        let overlapped = windows::Win32::System::IO::OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };

        Ok(Self {
            handle,
            event,
            buffer: vec![0u8; 64 * 1024],
            overlapped,
        })
    }

    /// Start (or restart) the read.
    fn issue(&mut self) -> windows::core::Result<()> {
        use windows::Win32::Foundation::ERROR_IO_PENDING;
        use windows::Win32::Storage::FileSystem::{
            ReadDirectoryChangesW, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
        };
        use windows::Win32::System::Threading::ResetEvent;

        unsafe {
            let _ = ResetEvent(self.event);
            let res = ReadDirectoryChangesW(
                self.handle,
                self.buffer.as_mut_ptr() as *mut core::ffi::c_void,
                self.buffer.len() as u32,
                true, // subtrees: a folder floatie shows what is inside it
                FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME,
                None,
                Some(&mut self.overlapped),
                None,
            );
            match res {
                Ok(()) => Ok(()),
                // An overlapped call that is still running reports this; it is
                // the normal outcome, not a failure.
                Err(err)
                    if err.code() == windows::core::HRESULT::from_win32(ERROR_IO_PENDING.0) =>
                {
                    Ok(())
                }
                Err(err) => Err(err),
            }
        }
    }

    /// Wait up to `ms` for the read to complete. `false` on timeout.
    fn wait(&self, ms: u32) -> bool {
        use windows::Win32::Foundation::WAIT_OBJECT_0;
        use windows::Win32::System::Threading::WaitForSingleObject;

        unsafe { WaitForSingleObject(self.event, ms) == WAIT_OBJECT_0 }
    }

    /// Take what the completed call found and parse the batch.
    fn drain(&mut self, pending: &mut Option<String>) -> windows::core::Result<Batch> {
        use windows::Win32::System::IO::GetOverlappedResult;

        let mut bytes: u32 = 0;
        unsafe { GetOverlappedResult(self.handle, &self.overlapped, &mut bytes, false) }?;
        Ok(parse_batch(&self.buffer, bytes, pending))
    }
}

#[cfg(windows)]
impl Drop for DirWatch {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
            let _ = windows::Win32::Foundation::CloseHandle(self.event);
        }
    }
}

/// Walk the `FILE_NOTIFY_INFORMATION` chain.
///
/// A zero `bytes` means the buffer overflowed: the queue held more than 64 KiB
/// of changes, which the caller must treat as "anything may have changed".
///
/// The chain is walked bytewise off the buffer the call filled — not through the
/// struct — because the struct only *declares* one `u16` of file name: the name
/// lives in the buffer after the 12-byte header, so reading it out of a copied
/// struct gives whatever happened to follow it on the stack.
///
#[cfg(windows)]
fn parse_notification_entry(buffer: &[u8], offset: usize, end: usize) -> Option<(usize, u32, String)> {
    const HEADER: usize = 12;
    if offset + HEADER > end {
        return None;
    }
    let next = u32::from_le_bytes(buffer[offset..offset + 4].try_into().ok()?) as usize;
    let action = u32::from_le_bytes(buffer[offset + 4..offset + 8].try_into().ok()?);
    let name_bytes = u32::from_le_bytes(buffer[offset + 8..offset + 12].try_into().ok()?) as usize;
    let name_start = offset + HEADER;
    let name_end = (name_start + name_bytes).min(end);
    let u16_chars: Vec<u16> = buffer[name_start.min(end)..name_end]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let name = String::from_utf16_lossy(&u16_chars);
    Some((next, action, name))
}

/// `pending` is where a half-seen rename lives between batches: a read can end
/// between the OLD and the NEW entry of one rename.
#[cfg(windows)]
fn parse_batch(buffer: &[u8], bytes: u32, pending: &mut Option<String>) -> Batch {
    use windows::Win32::Storage::FileSystem::{
        FILE_ACTION_ADDED, FILE_ACTION_MODIFIED, FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME,
        FILE_ACTION_RENAMED_OLD_NAME,
    };

    let mut batch = Batch {
        touched: false,
        overflow: false,
        renames: Vec::new(),
    };
    let end = (bytes as usize).min(buffer.len());
    if end == 0 {
        batch.touched = true;
        batch.overflow = true;
        return batch;
    }

    let mut offset = 0usize;
    while let Some((next, action, name)) = parse_notification_entry(buffer, offset, end) {
        if action == FILE_ACTION_RENAMED_OLD_NAME.0 {
            *pending = Some(name);
        } else if action == FILE_ACTION_RENAMED_NEW_NAME.0 {
            match pending.take() {
                Some(old) => batch.renames.push((old, name)),
                None => batch.touched = true,
            }
        } else if action == FILE_ACTION_ADDED.0 || action == FILE_ACTION_REMOVED.0 {
            *pending = None;
            batch.touched = true;
        } else if action == FILE_ACTION_MODIFIED.0 {
            *pending = None;
        }

        if next == 0 {
            break;
        }
        offset += next;
    }
    batch
}

/// What the loop needs from a directory watch: waitable, drainable, reopening
/// on error.
#[cfg(not(windows))]
struct DirWatch;

#[cfg(not(windows))]
struct Batch {
    pub(crate) touched: bool,
    pub(crate) renames: Renames,
}

/// Directory watching is Windows-only in this app; elsewhere the loop just
/// reports that the root is unavailable, which is logged once.
#[cfg(not(windows))]
impl DirWatch {
    fn open(_root: &std::path::Path) -> Result<Self, String> {
        Err("directory watching is not implemented on this platform".into())
    }
    fn issue(&mut self) -> Result<(), String> {
        Err("directory watching is not implemented on this platform".into())
    }
    fn wait(&self, _ms: u32) -> bool {
        true
    }
    fn drain(&mut self, _pending: &mut Option<String>) -> Result<Batch, String> {
        Err("directory watching is not implemented on this platform".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};

    /// Collect batches until one satisfies `ok`, or the deadline passes.
    fn wait_for(
        rx: &Receiver<(Renames, bool)>,
        timeout: Duration,
        mut ok: impl FnMut(&(Renames, bool)) -> bool,
    ) -> Option<(Renames, bool)> {
        let deadline = Instant::now() + timeout;
        let mut seen: Vec<(Renames, bool)> = Vec::new();
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(batch) => {
                    if ok(&batch) {
                        return Some(batch);
                    }
                    seen.push(batch);
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        None
    }

    /// One `FILE_NOTIFY_INFORMATION` entry, with its name, as the API lays it
    /// out: three `u32`s and then the name in UTF-16, tightly packed.
    #[cfg(windows)]
    fn entry(action: u32, name: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0u32.to_le_bytes()); // NextEntryOffset, filled below
        bytes.extend_from_slice(&action.to_le_bytes());
        bytes.extend_from_slice(&((name.len() * 2) as u32).to_le_bytes());
        for unit in name.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    #[cfg(windows)]
    fn chain(entries: &[(u32, &str)]) -> Vec<u8> {
        let mut parts: Vec<Vec<u8>> = entries.iter().map(|(a, n)| entry(*a, n)).collect();
        let mut out = Vec::new();
        for i in 0..parts.len() {
            let next = if i + 1 == parts.len() {
                0
            } else {
                parts[i].len() as u32
            };
            parts[i][0..4].copy_from_slice(&next.to_le_bytes());
            out.extend_from_slice(&parts[i]);
        }
        out
    }

    /// The parser has to keep the names: reading them out of the struct instead
    /// of the buffer produced garbage (`mo\0\0\0\0…`) that no rename could match.
    #[cfg(windows)]
    #[test]
    fn the_batch_parser_pairs_a_rename_and_keeps_the_names() {
        use windows::Win32::Storage::FileSystem::{
            FILE_ACTION_ADDED, FILE_ACTION_RENAMED_NEW_NAME, FILE_ACTION_RENAMED_OLD_NAME,
        };

        let buffer = chain(&[
            (FILE_ACTION_ADDED.0, "added.txt"),
            (FILE_ACTION_RENAMED_OLD_NAME.0, "before.txt"),
            (FILE_ACTION_RENAMED_NEW_NAME.0, "after.txt"),
        ]);
        let batch = parse_batch(&buffer, buffer.len() as u32, &mut None);
        assert!(batch.touched, "a create has to ask for a reconcile");
        assert_eq!(
            batch.renames,
            vec![("before.txt".to_string(), "after.txt".to_string())]
        );

        // A lone NEW_NAME is a file moved in from outside, not a pair.
        let lone = chain(&[(FILE_ACTION_RENAMED_NEW_NAME.0, "arrived.txt")]);
        let batch = parse_batch(&lone, lone.len() as u32, &mut None);
        assert!(batch.touched && batch.renames.is_empty());

        // An overflowed queue says "anything may have changed" — and is flagged
        // as such, so the reconcile it asks for can be reported as a full pass.
        let overflow = parse_batch(&buffer, 0, &mut None);
        assert!(overflow.touched && overflow.renames.is_empty());
        assert!(overflow.overflow, "an overflowed queue has to say so");

        // An ordinary create is not an overflow: the log tells them apart.
        let plain = parse_batch(&buffer, buffer.len() as u32, &mut None);
        assert!(plain.touched && !plain.overflow);

        // The two halves can land in different reads: the OLD is remembered.
        let mut pending = None;
        let old_only = chain(&[(FILE_ACTION_RENAMED_OLD_NAME.0, "split.txt")]);
        let first = parse_batch(&old_only, old_only.len() as u32, &mut pending);
        assert!(!first.touched && first.renames.is_empty());
        let new_only = chain(&[(FILE_ACTION_RENAMED_NEW_NAME.0, "joined.txt")]);
        let second = parse_batch(&new_only, new_only.len() as u32, &mut pending);
        assert_eq!(
            second.renames,
            vec![("split.txt".to_string(), "joined.txt".to_string())]
        );
    }

    /// A real directory, a real watch: the thing that has to work for the
    /// desktop to follow the folder.
    #[test]
    fn the_watch_reports_a_new_file_and_a_rename() {
        let dir = std::env::temp_dir().join(format!("floaty-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        // a file that already exists, so the rename below has something to move
        std::fs::write(dir.join("before.txt"), b"x").expect("seed file");

        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let root = dir.to_string_lossy().to_string();
        let thread_stop = stop.clone();
        std::thread::spawn(move || {
            let mut log = |_msg: &str| {};
            let mut sink = move |renames: Renames, touched: bool| {
                let _ = tx.send((renames, touched));
            };
            watch_loop(&root, &thread_stop, &mut sink, &mut log);
        });

        // A create can land before the thread has issued its first read; keep
        // touching a file until the watch reports something.
        let mut reported_create = false;
        for attempt in 0..12 {
            std::fs::write(dir.join(format!("probe-{attempt}.txt")), b"x").expect("probe");
            if wait_for(&rx, Duration::from_millis(700), |_| true).is_some() {
                reported_create = true;
                break;
            }
        }
        assert!(reported_create, "the watch never reported a file appearing");

        // Let the queue settle, then rename a known file and insist on the pair.
        let _ = wait_for(&rx, Duration::from_millis(400), |_| false);
        std::fs::write(dir.join("moving.txt"), b"x").expect("file to move");
        let seen_create = wait_for(&rx, Duration::from_secs(5), |(renames, touched)| {
            *touched || !renames.is_empty()
        });
        assert!(
            seen_create.is_some(),
            "the watch missed the file it has to rename"
        );

        std::fs::rename(dir.join("moving.txt"), dir.join("moved.txt")).expect("rename");
        let paired = wait_for(&rx, Duration::from_secs(5), |(renames, _)| {
            renames.iter().any(|(from, _)| from.ends_with("moving.txt"))
        });
        // The names come off the wire *relative to the watched directory*: the
        // pair has to arrive as full paths, or nothing can match a record — the
        // live version of this shipped once and quietly re-created the floatie
        // at a new spot instead of moving it.
        let (from, to) = paired.expect("the watch did not pair the rename").0[0].clone();
        assert_eq!(std::path::Path::new(&from), dir.join("moving.txt"));
        assert_eq!(std::path::Path::new(&to), dir.join("moved.txt"));

        stop.store(true, Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
