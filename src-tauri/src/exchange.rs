//! Files arriving from outside floaty: a drop from Explorer, or a paste.
//!
//! Everything here is about *paths*, not widgets: what the desktop does with them
//! afterwards (move into the pointed root, into the folder floatie under the drop, make
//! a record, place it where the pointer was) is `floaty_drop_paths` in `lib.rs`, which
//! already had that machinery for the two-floatie merge.
//!
//! The clipboard is the only new Win32 surface: `CF_HDROP`, which is the format Explorer
//! uses for "a copy of these files". The read and the write are both here, because a
//! paste that can only be tested against its own writer is a paste that agrees with
//! itself and nothing else — the verification uses PowerShell's `Get-Clipboard
//! -Format FileDropList` as the second opinion.

use std::path::PathBuf;

#[cfg(windows)]
mod win {
    use super::PathBuf;
    use windows::core::{BOOL, PCWSTR};
    use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable,
        OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::CF_HDROP;
    use windows::Win32::UI::Shell::{DragQueryFileW, DROPFILES, HDROP};

    /// Retry an open: another process can hold the clipboard for a few milliseconds at a
    /// time (a shell copy, a screenshot tool), and failing then would look like "paste
    /// does nothing".
    fn open() -> bool {
        for _ in 0..8 {
            if unsafe { OpenClipboard(Some(HWND::default())) }.is_ok() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        false
    }

    pub fn files() -> Vec<PathBuf> {
        let mut out = Vec::new();
        unsafe {
            if IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_err() {
                return out;
            }
            if !open() {
                return out;
            }
            if let Ok(handle) = GetClipboardData(CF_HDROP.0 as u32) {
                if !handle.is_invalid() {
                    let drop = HDROP(handle.0);
                    let count = DragQueryFileW(drop, 0xFFFF_FFFF, None);
                    for index in 0..count {
                        let len = DragQueryFileW(drop, index, None);
                        if len == 0 {
                            continue;
                        }
                        let mut buf = vec![0u16; len as usize + 1];
                        let written = DragQueryFileW(drop, index, Some(&mut buf));
                        if written > 0 {
                            out.push(PathBuf::from(String::from_utf16_lossy(
                                &buf[..written as usize],
                            )));
                        }
                    }
                }
            }
            let _ = CloseClipboard();
        }
        out
    }

    /// Put paths on the clipboard as a file list, the way Explorer's Copy does.
    pub fn set_files(paths: &[PathBuf]) -> Result<(), String> {
        if paths.is_empty() {
            return Err("nothing to copy".into());
        }
        // DROPFILES, then the paths as wide strings, then one extra NUL for the list.
        let mut names: Vec<u16> = Vec::new();
        for path in paths {
            names.extend(path.to_string_lossy().encode_utf16());
            names.push(0);
        }
        names.push(0);
        let header = std::mem::size_of::<DROPFILES>();
        let bytes = header + names.len() * 2;
        unsafe {
            let global = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|e| e.to_string())?;
            let mem = GlobalLock(global);
            if mem.is_null() {
                return Err("could not lock the clipboard block".into());
            }
            let drop = mem as *mut DROPFILES;
            (*drop).pFiles = header as u32;
            (*drop).fWide = BOOL::from(true);
            std::ptr::copy_nonoverlapping(
                names.as_ptr() as *const u8,
                (mem as *mut u8).add(header),
                names.len() * 2,
            );
            let _ = GlobalUnlock(global);

            if !open() {
                return Err("the clipboard is held by another program".into());
            }
            let _ = EmptyClipboard();
            let set = SetClipboardData(CF_HDROP.0 as u32, Some(HANDLE(global.0)));
            let _ = CloseClipboard();
            set.map_err(|e| format!("could not set the clipboard: {e}"))?;
        }
        Ok(())
    }

    /// The text on the clipboard, if it is text — the second half of a paste, so a copied
    /// path from a terminal or a browser address bar can be pasted onto the desktop.
    pub fn text() -> Option<String> {
        use windows::Win32::System::Ole::CF_UNICODETEXT;
        unsafe {
            if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_err() {
                return None;
            }
            if !open() {
                return None;
            }
            let mut out = None;
            if let Ok(handle) = GetClipboardData(CF_UNICODETEXT.0 as u32) {
                if !handle.is_invalid() {
                    let ptr = windows::Win32::System::Memory::GlobalLock(HGLOBAL(handle.0))
                        as *const u16;
                    if !ptr.is_null() {
                        let mut len = 0usize;
                        while *ptr.add(len) != 0 && len < 32_768 {
                            len += 1;
                        }
                        let slice = std::slice::from_raw_parts(ptr, len);
                        out = Some(String::from_utf16_lossy(slice));
                        let _ = windows::Win32::System::Memory::GlobalUnlock(HGLOBAL(handle.0));
                    }
                }
            }
            let _ = CloseClipboard();
            let _ = PCWSTR::null();
            out
        }
    }
}

/// The paths on the clipboard, if it holds files. Empty means "something else is on the
/// clipboard", which is not an error.
#[cfg(windows)]
pub fn clipboard_files() -> Vec<PathBuf> {
    win::files()
}

#[cfg(not(windows))]
pub fn clipboard_files() -> Vec<PathBuf> {
    Vec::new()
}

/// Put paths on the clipboard so Explorer and other apps see a file copy.
#[cfg(windows)]
pub fn set_clipboard_files(paths: &[PathBuf]) -> Result<(), String> {
    win::set_files(paths)
}

#[cfg(not(windows))]
pub fn set_clipboard_files(_paths: &[PathBuf]) -> Result<(), String> {
    Err("only Windows has a file clipboard".into())
}

/// The clipboard's text, as a candidate path, when it holds one.
#[cfg(windows)]
pub fn clipboard_text() -> Option<String> {
    win::text()
}

#[cfg(not(windows))]
pub fn clipboard_text() -> Option<String> {
    None
}

/// Turn a clipboard's text into paths to paste: a multi-line list of real paths, or a
/// single one. Anything that does not exist on disk is dropped — pasting a sentence onto
/// the desktop is not a reason to create a file called after it.
pub fn paths_from_text(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(|line| line.trim().trim_matches('"').to_string())
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect()
}

/// How a path arriving from outside should be brought in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Arrival {
    /// Already here (or same volume, same folder): nothing to copy, just a place to be.
    Move,
    /// A different volume: copy, the way Explorer does, and leave the original.
    Copy,
    /// Inside a program's install directory: a shortcut, because moving it would take the
    /// program out of the directory it needs to run from.
    Shortcut,
}

/// Is this path inside somewhere Windows owns — a program's install directory, or the
/// system itself? Those are the paths where a move breaks something.
pub fn is_program_dir(path: &std::path::Path) -> bool {
    path.components().any(|c| {
        let text = c.as_os_str().to_string_lossy().to_ascii_lowercase();
        text == "program files"
            || text == "program files (x86)"
            || text == "windows"
            || text == "programdata"
    })
}

/// Are these two paths on the same volume? (`C:\` vs `D:\`.)
pub fn same_volume(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (a.components().next(), b.components().next()) {
        (Some(x), Some(y)) => x
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&y.as_os_str().to_string_lossy()),
        _ => false,
    }
}

/// The rule for a path dropped onto the desktop from outside.
///
/// `already_here` is the path being inside the folder the desktop shows, which is what
/// makes a cut-and-paste of your own file a move rather than a second copy of it.
pub fn arrival(src: &std::path::Path, already_here: bool, dest_dir: &std::path::Path) -> Arrival {
    if already_here || same_volume(src, dest_dir) && !is_program_dir(src) {
        Arrival::Move
    } else if is_program_dir(src) {
        Arrival::Shortcut
    } else {
        Arrival::Copy
    }
}

/// Should this paste land in a *folder floatie*'s directory, given the drop point is
/// inside it? The rule is the one `floaty_dropped` uses for the merge preview: the point
/// against the tile's rect, inflated by 12px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

pub const SLOP: f64 = 12.0;

pub fn hits(tile: &Hit, px: f64, py: f64) -> bool {
    px >= tile.x - SLOP
        && px < tile.x + tile.w + SLOP
        && py >= tile.y - SLOP
        && py < tile.y + tile.h + SLOP
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_becomes_only_the_paths_that_exist() {
        let real = std::env::temp_dir();
        let text = format!(
            "  \"{}\"  \n\nnot a path at all\n{}",
            real.display(),
            real.join("definitely-not-here-12345").display()
        );
        let paths = paths_from_text(&text);
        assert_eq!(paths.len(), 1, "only the real one survives: {paths:?}");
        assert_eq!(paths[0], real);
    }

    #[test]
    fn a_drop_moves_within_a_volume_and_copies_across_one() {
        let temp = std::path::Path::new("C:/Users/erict/AppData/Local/Temp");
        let desktop = std::path::Path::new("C:/Users/erict/OneDrive/Desktop");
        let other_drive = std::path::Path::new("D:/stuff/report.txt");
        assert_eq!(
            arrival(&temp.join("report.txt"), false, desktop),
            Arrival::Move,
            "a file on the same drive is moved, as Explorer does"
        );
        assert_eq!(arrival(&other_drive, false, desktop), Arrival::Copy);
        assert_eq!(
            arrival(&desktop.join("report.txt"), true, desktop),
            Arrival::Move,
            "a paste of your own file moves it rather than duplicating it"
        );
    }

    #[test]
    fn a_program_from_an_install_directory_becomes_a_shortcut() {
        let exe = std::path::Path::new("C:/Program Files/Some App/app.exe");
        let desktop = std::path::Path::new("C:/Users/erict/OneDrive/Desktop");
        assert!(is_program_dir(exe));
        assert_eq!(arrival(exe, false, desktop), Arrival::Shortcut);
        assert_eq!(
            arrival(&std::path::Path::new("C:/Windows/System32/notepad.exe"), false, desktop),
            Arrival::Shortcut
        );
        // ...but a file in Documents, on the same drive, is still a move
        assert_eq!(
            arrival(&std::path::Path::new("C:/Users/erict/Documents/a.txt"), false, desktop),
            Arrival::Move
        );
    }

    #[test]
    fn a_hit_is_the_tile_inflated_by_the_same_slop_the_drop_uses() {
        let tile = Hit {
            x: 100.0,
            y: 200.0,
            w: 92.0,
            h: 112.0,
        };
        assert!(hits(&tile, 100.0, 200.0), "the tile's own corner");
        // the left slop runs from 88 (tile.x - 12) to the tile
        assert!(hits(&tile, 100.0 - SLOP, 200.0), "the far edge of the slop still hits");
        assert!(!hits(&tile, 100.0 - SLOP - 1.0, 200.0), "one px beyond it does not");
        // and the right edge: 100 + 92 + 12 = 204 is outside, 203 is inside
        assert!(hits(&tile, 203.0, 200.0));
        assert!(!hits(&tile, 204.0, 200.0));
        assert!(!hits(&tile, 100.0, 200.0 + 112.0 + SLOP), "below the slop is outside");
    }
}
