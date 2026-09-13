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
