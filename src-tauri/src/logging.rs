//! `logging` — moved out of lib.rs verbatim.
//!
//! Visibility is `pub(crate)` because the rest of the crate calls in across the module
//! boundary now; nothing here changed shape on the way out.

use crate::*;
use std::fs;
use tauri::{AppHandle, Manager};

// ---------- logging ----------

/// `[hh:mm:ss.mmm]`, so a recovery that took ten seconds says so in the log.
pub(crate) fn timestamp() -> String {
    #[cfg(windows)]
    {
        use windows::Win32::System::SystemInformation::GetLocalTime;
        let t = unsafe { GetLocalTime() };
        format!(
            "[{:02}:{:02}:{:02}.{:03}]",
            t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
        )
    }
    #[cfg(not(windows))]
    {
        String::new()
    }
}

/// Log lines now come from several threads (power callbacks, the pump watchdog);
/// without this the appends interleave.
pub(crate) static LOG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn log_line(app: &AppHandle, msg: &str) {
    let _serialised = LOG_LOCK.lock();
    let msg = &format!("{} {msg}", timestamp());
    eprintln!("[floaty] {msg}");
    let Ok(dir) = app.path().app_data_dir().inspect(|d| {
        fs::create_dir_all(d).ok();
    }) else {
        return;
    };
    let path = dir.join("floaty.log");
    // Roll rather than wipe: emptying the file destroys exactly the evidence
    // someone is looking for (see `diagnostics.rs`).
    //
    // The note about it goes into the same append instead of through `log_line`,
    // which would deadlock on the lock this function is holding. It is worth
    // writing down: "the lines I was looking for are gone" now has a visible
    // reason and a file to look in.
    use std::fmt::Write as _;
    let mut line = String::new();
    let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if diagnostics::should_roll(size) {
        diagnostics::roll(&path);
        let _ = writeln!(
            line,
            "{} log was {} — rolled to floaty.log.1",
            timestamp(),
            diagnostics::human_bytes(size)
        );
    }
    let _ = writeln!(line, "{msg}");
    use std::io::Write as _;
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()))
        .ok();
}
