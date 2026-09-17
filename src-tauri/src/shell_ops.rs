//! Real file-system operations behind the file/folder floaties, so they behave
//! like the desktop instead of only moving widgets around: delete goes to the
//! Recycle Bin, "Open with" is the shell's own picker, Properties is the real
//! property sheet, and rename touches the file on disk.

use std::path::{Path, PathBuf};

/// Move a file or directory to the Recycle Bin (undoable, exactly like
/// deleting from Explorer). Falls back to an error the caller can surface.
#[cfg(windows)]
pub fn recycle(path: &Path) -> Result<(), String> {
    use windows::Win32::UI::Shell::{
        SHFileOperationW, SHFILEOPSTRUCTW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI,
        FOF_SILENT, FO_DELETE,
    };

    if !path.exists() {
        return Err(format!("{} no longer exists", path.display()));
    }
    let mut from = wide_multi(path);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: windows::core::PCWSTR(from.as_mut_ptr()),
        fFlags: (FOF_ALLOWUNDO.0 | FOF_NOCONFIRMATION.0 | FOF_SILENT.0 | FOF_NOERRORUI.0) as u16,
        ..Default::default()
    };
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 {
        return Err(format!("shell delete failed (code {code})"));
    }
    if op.fAnyOperationsAborted.as_bool() {
        return Err("delete was cancelled".into());
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn recycle(_path: &Path) -> Result<(), String> {
    Err("recycling is windows-only".into())
}

/// The shell's "Open with" dialog for a path (what Explorer's own menu opens).
pub fn open_with(path: &str) -> Result<(), String> {
    spawn_hidden("rundll32.exe", &["shell32.dll,OpenAs_RunDLL", path])
}

/// The argument that makes explorer select an item: the flag, then the path in
/// quotes. The quotes have to be part of the argument itself. Passed as a plain
/// argument, the standard Windows quoting wraps the whole `/select,<path>` token
/// whenever the path contains a space, and explorer cannot resolve that — it
/// opens the default folder instead of selecting anything, which is why
/// revealing a floatie with a space in its name silently did nothing.
pub fn reveal_args(path: &str) -> String {
    format!("/select,\"{path}\"")
}

/// Show the item selected in a File Explorer window.
#[cfg(windows)]
pub fn reveal(path: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const NO_WINDOW: u32 = 0x08000000;

    if !Path::new(path).exists() {
        return Err(format!("{path} no longer exists"));
    }
    std::process::Command::new("explorer.exe")
        .raw_arg(reveal_args(path))
        .creation_flags(NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not start explorer: {e}"))
}

#[cfg(not(windows))]
pub fn reveal(_path: &str) -> Result<(), String> {
    Err("shell verbs are windows-only".into())
}

/// The real property sheet for the item, via the shell's "properties" verb.
#[cfg(windows)]
pub fn properties(path: &str) -> Result<(), String> {
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_INVOKEIDLIST, SHELLEXECUTEINFOW};

    let verb = wide("properties");
    let file = wide(path);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST,
        lpVerb: windows::core::PCWSTR(verb.as_ptr()),
        lpFile: windows::core::PCWSTR(file.as_ptr()),
        nShow: 1, // SW_SHOWNORMAL
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| format!("properties failed: {e}"))
}

#[cfg(not(windows))]
pub fn properties(_path: &str) -> Result<(), String> {
    Err("properties sheet is windows-only".into())
}

/// Rename an item in place. Validates the name the way Explorer does and
/// refuses to clobber an existing sibling rather than silently replacing it.
pub fn rename(old: &Path, new_name: &str) -> Result<PathBuf, String> {
    let name = new_name.trim();
    if name.is_empty() {
        return Err("name cannot be empty".into());
    }
    if name == "." || name == ".." {
        return Err("invalid name".into());
    }
    if name.contains(['/', '\\']) {
        return Err("name cannot contain a path separator".into());
    }
    if name.contains(['<', '>', ':', '"', '|', '?', '*']) {
        return Err("name contains a character windows does not allow".into());
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err("name cannot end with a dot or a space".into());
    }
    let parent = old
        .parent()
        .ok_or_else(|| "item has no parent directory".to_string())?;
    if !old.exists() {
        return Err(format!("{} no longer exists", old.display()));
    }
    let dest = parent.join(name);
    if dest == old {
        return Ok(dest);
    }
    if dest.exists() {
        return Err(format!("'{name}' already exists here"));
    }
    std::fs::rename(old, &dest).map_err(|e| format!("rename failed: {e}"))?;
    Ok(dest)
}

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// SHFileOperation wants a double-null terminated list of paths.
#[cfg(windows)]
fn wide_multi(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    let mut buf: Vec<u16> = path.as_os_str().encode_wide().collect();
    buf.push(0);
    buf.push(0);
    buf
}

fn spawn_hidden(program: &str, args: &[&str]) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const NO_WINDOW: u32 = 0x08000000;
        std::process::Command::new(program)
            .args(args)
            .creation_flags(NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("could not start {program}: {e}"))
    }
    #[cfg(not(windows))]
    {
        let _ = (program, args);
        Err("shell verbs are windows-only".into())
    }
}

/// Put a recycled file back where it came from.
///
/// Three routes were tried and two are recorded here because they look right and
/// are not:
///
/// - **The bin's own `Restore` verb does nothing from a script.** `InvokeVerb()`
///   and `InvokeVerb('Restore')` both return success and leave the file in the
///   bin — measured for files and for directories, with the verb's name spelled
///   with and without its accelerator. So no shell verb.
/// - **The Shell namespace lags a fresh delete.** Listing `NameSpace(0xA)` and
///   matching `System.Recycle.DeletedFrom` + the item's name does find an entry —
///   a minute later. Measured right after a delete (which is when an undo is
///   pressed) it reports nothing, through six attempts over 1.5s, and that is
///   exactly what "undo does nothing" would look like.
///
/// So this reads the bin the way the shell stores it: beside each `$I<id>`
/// metadata file sits the file itself as `$R<id>`. `$I` holds the original path
/// and the deletion time, which is all a restore needs — move `$R` back, drop
/// `$I`. No COM, no namespace cache, no waiting.
#[cfg(windows)]
pub fn restore_from_bin(path: &str) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("no path to restore".into());
    }
    let target = Path::new(path);
    if target.exists() {
        return Err(format!("'{path}' is on disk already"));
    }

    // The newest entry for this path wins: the same file can be recycled more
    // than once, and undoing the most recent delete is what was asked for.
    let mut best: Option<(std::time::SystemTime, PathBuf, PathBuf)> = None;
    for (metadata, file, original, deleted_at) in bin_entries() {
        if !same_path(&original, path) {
            continue;
        }
        if best.as_ref().map(|(t, _, _)| deleted_at > *t).unwrap_or(true) {
            best = Some((deleted_at, file, metadata));
        }
    }

    let Some((_, file, metadata)) = best else {
        return Err(format!("'{path}' is no longer in the Recycle Bin"));
    };
    if !file.exists() {
        return Err(format!(
            "the Recycle Bin remembers '{path}' but no longer holds it"
        ));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::rename(&file, target).map_err(|e| format!("could not put '{path}' back: {e}"))?;
    // The metadata goes last: a crash in between leaves the file restored and a
    // stale row in the bin, which is visible and harmless — the other order
    // loses the file.
    let _ = std::fs::remove_file(&metadata);
    Ok(())
}

/// Every entry in every Recycle Bin this user can read, as
/// `($I path, $R path, original path, deleted at)`.
///
/// One bin per volume, each with a folder per SID; another user's folder simply
/// cannot be read and is skipped.
fn bin_entries() -> Vec<(PathBuf, PathBuf, String, std::time::SystemTime)> {
    let mut out = Vec::new();
    for letter in b'A'..=b'Z' {
        let root = PathBuf::from(format!("{}:\\$Recycle.Bin", letter as char));
        let Ok(sids) = std::fs::read_dir(&root) else {
            continue;
        };
        for sid in sids.filter_map(|e| e.ok()) {
            let Ok(entries) = std::fs::read_dir(sid.path()) else {
                continue;
            };
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                let Some(suffix) = name.strip_prefix("$I") else {
                    continue;
                };
                let Ok(bytes) = std::fs::read(entry.path()) else {
                    continue;
                };
                let Some((original, deleted_at)) = parse_recycled_metadata(&bytes) else {
                    continue;
                };
                let file = entry.path().with_file_name(format!("$R{suffix}"));
                out.push((entry.path(), file, original, deleted_at));
            }
        }
    }
    out
}

/// The original path and the moment it was deleted, out of an `$I` file.
///
/// Version 2+ (Vista and later, so everything this build runs on) stores the path
/// as a count of UTF-16 code units followed by the text; version 1 stored it as
/// ANSI. Both are read, because a bin carried over from an old profile is not a
/// reason to refuse to put someone's file back.
fn parse_recycled_metadata(bytes: &[u8]) -> Option<(String, std::time::SystemTime)> {
    if bytes.len() < 28 {
        return None;
    }
    let version = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
    let deleted_at = filetime_to_system_time(u64::from_le_bytes(bytes[16..24].try_into().ok()?));

    let original = if version >= 2 {
        let units = u32::from_le_bytes(bytes[24..28].try_into().ok()?) as usize;
        let start = 28usize;
        let end = start.checked_add(units.checked_mul(2)?)?;
        let raw = bytes.get(start..end)?;
        let mut utf16: Vec<u16> = raw
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        while utf16.last() == Some(&0) {
            utf16.pop();
        }
        String::from_utf16(&utf16).ok()?
    } else {
        let rest = bytes.get(24..)?;
        let end = rest.iter().position(|b| *b == 0)?;
        String::from_utf8_lossy(&rest[..end]).to_string()
    };
    if original.is_empty() {
        return None;
    }
    Some((original, deleted_at))
}

/// Windows' 100ns-since-1601 clock into a `SystemTime`.
fn filetime_to_system_time(raw: u64) -> std::time::SystemTime {
    const EPOCH_DIFF_SECS: u64 = 11_644_473_600;
    let secs = raw / 10_000_000;
    let nanos = ((raw % 10_000_000) * 100) as u32;
    std::time::UNIX_EPOCH
        + std::time::Duration::new(secs.saturating_sub(EPOCH_DIFF_SECS), nanos.min(999_999_999))
}

/// Windows paths are case-insensitive, and a trailing separator is not a
/// difference worth failing a restore over.
fn same_path(a: &str, b: &str) -> bool {
    a.trim_end_matches(['\\', '/'])
        .eq_ignore_ascii_case(b.trim_end_matches(['\\', '/']))
}

#[cfg(not(windows))]
pub fn restore_from_bin(_path: &str) -> Result<(), String> {
    Err("the Recycle Bin is windows-only".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("floaty-shellops-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reveal_quotes_the_path_after_the_flag() {
        // Verified against explorer.exe on Windows 11: /select,C:\a b.txt (or the
        // whole token quoted) opens the default folder and selects nothing, while
        // /select,"C:\a b.txt" selects it.
        assert_eq!(
            reveal_args(r"C:\Users\me\Desktop\rent calculation.xlsx"),
            "/select,\"C:\\Users\\me\\Desktop\\rent calculation.xlsx\""
        );
    }

    #[test]
    fn rename_validates_the_way_the_shell_does() {
        let dir = temp_dir("rename");
        let file = dir.join("old.txt");
        std::fs::write(&file, b"x").unwrap();

        assert!(rename(&file, "").is_err());
        assert!(rename(&file, "  ").is_err());
        assert!(rename(&file, "a/b.txt").is_err());
        assert!(rename(&file, "a\\b.txt").is_err());
        assert!(rename(&file, "bad?.txt").is_err());
        assert!(rename(&file, "trailing. ").is_err());
        assert!(rename(&file, "..").is_err());

        let renamed = rename(&file, "new.txt").unwrap();
        assert_eq!(renamed.file_name().unwrap(), "new.txt");
        assert!(renamed.exists(), "the file itself must be renamed on disk");
        assert!(!file.exists());

        // does not clobber an existing sibling
        let other = dir.join("other.txt");
        std::fs::write(&other, b"y").unwrap();
        assert!(rename(&renamed, "other.txt").is_err());
        assert_eq!(std::fs::read(&other).unwrap(), b"y");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An `$I` file as Windows 10 writes it: version 2, a UTF-16 path.
    fn i_file_v2(original: &str, deleted_unix: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&2u64.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&((deleted_unix + 11_644_473_600) * 10_000_000).to_le_bytes());
        let units: Vec<u16> = original.encode_utf16().collect();
        bytes.extend_from_slice(&(units.len() as u32).to_le_bytes());
        for unit in &units {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes
    }

    /// The older layout: version 1, an ANSI path.
    fn i_file_v1(original: &str, deleted_unix: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&((deleted_unix + 11_644_473_600) * 10_000_000).to_le_bytes());
        bytes.extend_from_slice(original.as_bytes());
        bytes.push(0);
        bytes
    }

    #[test]
    fn a_recycled_files_metadata_parses_in_both_formats() {
        let unix = 1_700_000_000u64;
        let (path, when) =
            parse_recycled_metadata(&i_file_v2(r"C:\Users\me\Desktop\a b.txt", unix)).unwrap();
        assert_eq!(path, r"C:\Users\me\Desktop\a b.txt");
        assert_eq!(
            when.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
            unix,
            "the deletion time is what picks the newest of several entries"
        );
        // a name a byte-wise read would mangle
        let (path, _) =
            parse_recycled_metadata(&i_file_v2(r"C:\Users\me\Desktop\百度网盘.lnk", unix)).unwrap();
        assert_eq!(path, r"C:\Users\me\Desktop\百度网盘.lnk");
        // and the format an old profile's bin would still be in
        let (path, _) = parse_recycled_metadata(&i_file_v1(r"C:\old\thing.txt", unix)).unwrap();
        assert_eq!(path, r"C:\old\thing.txt");
        // junk, and a record with no name, are refused rather than restored
        // somewhere invented
        assert!(parse_recycled_metadata(&[]).is_none());
        assert!(parse_recycled_metadata(&[0u8; 40]).is_none());
        assert!(parse_recycled_metadata(&i_file_v2("", unix)).is_none());
    }

    #[test]
    fn paths_match_the_way_windows_compares_them() {
        assert!(same_path(r"C:\Users\Me\a.txt", r"c:\users\me\A.TXT"));
        assert!(same_path(r"C:\Users\Me\", r"C:\Users\Me"));
        assert!(!same_path(r"C:\Users\Me\a.txt", r"C:\Users\Me\b.txt"));
    }

    #[test]
    #[cfg(windows)]
    fn a_recycled_directory_comes_back_with_its_contents() {
        let dir = temp_dir("restore-dir");
        let folder = dir.join("Grouped");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("inside.txt"), b"still here").unwrap();

        recycle(&folder).unwrap();
        assert!(!folder.exists());

        restore_from_bin(&folder.to_string_lossy()).expect("the bin should hold the folder");
        assert!(folder.is_dir(), "the folder is back");
        assert_eq!(
            std::fs::read(folder.join("inside.txt")).unwrap(),
            b"still here",
            "with what was inside it"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The bin halves of one round trip: what `recycle` put in comes back out.
    #[test]
    #[cfg(windows)]
    fn a_recycled_file_can_be_put_back() {
        let dir = temp_dir("restore");
        let victim = dir.join("throwaway-restore.txt");
        std::fs::write(&victim, b"put me back").unwrap();

        recycle(&victim).expect("shell delete should succeed");
        assert!(!victim.exists());

        restore_from_bin(&victim.to_string_lossy()).expect("the bin should hold it");
        assert!(victim.exists(), "the file is back on disk");
        assert_eq!(std::fs::read(&victim).unwrap(), b"put me back");

        // and a path that was never recycled says so rather than pretending
        let never = dir.join("never-recycled.txt");
        let err = restore_from_bin(&never.to_string_lossy()).unwrap_err();
        assert!(err.contains("Recycle Bin"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(windows)]
    fn recycle_moves_a_file_to_the_bin() {
        let dir = temp_dir("recycle");
        let victim = dir.join("throwaway.txt");
        std::fs::write(&victim, b"delete me").unwrap();
        assert!(victim.exists());

        recycle(&victim).expect("shell delete should succeed");
        assert!(!victim.exists(), "the file must be gone from disk");
        assert!(recycle(&victim).is_err(), "a missing path is reported, not silently ignored");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
