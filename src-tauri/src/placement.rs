//! `placement` — where an item goes, and what it is called when it gets there.
//!
//! Three loops used to answer "is this name free?", and they did not agree: items moved into a
//! folder numbered from `(1)`, items arriving on the desktop from `(2)`, a new folder from
//! `(2)`. Two rename paths answered "is this name allowed?" — one checked it the way Explorer
//! checks it, the other built its destination by hand and called `std::fs::rename` directly, so
//! a name that the window's rename refused was accepted when it came from the folder's.
//!
//! A name is now checked in one place and numbered in one place, in the shell's own scheme: the
//! unnumbered name is the first one, so the first collision is `(2)`. The interfaces are
//! [`checked_name`], [`free_name`], [`rename_in_place`], [`place_into`] and
//! [`create_new_folder`].
//!
//! Known limit, unchanged from before this module existed: whether a name is free is decided by
//! looking, and the move is a separate step — `rename` on Windows writes over whatever is there.
//! Two placements racing for one name is the case this does not close (see BACKLOG).

use std::path::{Path, PathBuf};

/// The name Windows itself gives a folder made by hand: the shell takes the first free one, so
/// a second folder is `New folder (2)`.
pub(crate) const NEW_FOLDER_BASE: &str = "New folder";

/// The name a person typed, checked the way Explorer checks it.
///
/// The refusals are the ones a person can act on: what Windows will not accept in a name, and
/// what would send the item somewhere other than where it looks like it is going.
pub(crate) fn checked_name(name: &str) -> Result<String, String> {
    let name = name.trim();
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
    Ok(name.to_string())
}

/// The next free version of `name` in `dir`: `report.txt`, `report (2).txt`, … — the shell's
/// scheme, so a name taken by Explorer and a name taken by a floatie resolve the same way.
///
/// Naming only: what is done with the path is the caller's (a move, a copy, a shortcut).
pub(crate) fn free_name(dir: &Path, name: impl AsRef<Path>) -> PathBuf {
    let name = name.as_ref();
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let stem = name.file_stem().map(|s| s.to_string_lossy().into_owned());
    let ext = name.extension().map(|e| e.to_string_lossy().into_owned());
    for n in 2..1000 {
        let next = match (&stem, &ext) {
            (Some(stem), Some(ext)) => format!("{stem} ({n}).{ext}"),
            (Some(stem), None) => format!("{stem} ({n})"),
            // no stem to number around (a name that is all extension): number the whole thing
            _ => format!("{} ({n})", name.to_string_lossy()),
        };
        let candidate = dir.join(next);
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(name)
}

/// Rename an item in place, and say where it ended up. The name is checked first, and a rename
/// onto a sibling is refused rather than numbered: asking for a name is asking for *that* name.
///
/// The exception is the item's own name in another case. Windows remembers one case at a time,
/// so `Marina` → `MARINA` is the same place spelled differently, not a collision.
pub(crate) fn rename_in_place(from: &Path, new_name: &str) -> Result<PathBuf, String> {
    let name = checked_name(new_name)?;
    let parent = from
        .parent()
        .ok_or_else(|| "item has no parent directory".to_string())?;
    if !from.exists() {
        return Err(format!("{} no longer exists", from.display()));
    }
    let dest = parent.join(&name);
    if dest == from {
        return Ok(dest);
    }
    if dest.exists() && !same_name_other_case(&dest, from) {
        return Err(format!("'{name}' already exists here"));
    }
    std::fs::rename(from, &dest).map_err(|e| format!("rename failed: {e}"))?;
    Ok(dest)
}

/// Do these two paths name the same item, differing only in the case of the last part?
fn same_name_other_case(a: &Path, b: &Path) -> bool {
    match (a.file_name(), b.file_name()) {
        (Some(a), Some(b)) => a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy()),
        _ => false,
    }
}

/// Move a file or folder that is already on disk into `dir`, under `name` or under its own name.
///
/// The `Some` name is a name this code chose (a shortcut's, a label's) and is checked; the
/// item's own name is not, because Windows already accepted it once. The move is a rename, so
/// this is for items on the same volume.
pub(crate) fn place_into(from: &Path, dir: &Path, name: Option<&str>) -> Result<PathBuf, String> {
    let wanted = match name {
        Some(name) => checked_name(name)?,
        None => from
            .file_name()
            .ok_or_else(|| format!("{} has no file name", from.display()))?
            .to_string_lossy()
            .into_owned(),
    };
    if !from.exists() {
        return Err(format!("{} no longer exists", from.display()));
    }
    let dest = free_name(dir, &wanted);
    if dest == from {
        return Ok(dest);
    }
    std::fs::rename(from, &dest).map_err(|e| format!("move {} FAILED: {e}", from.display()))?;
    Ok(dest)
}

/// Make the next free `New folder (n)` in `dir` and return its path.
///
/// The create is what reserves the name: a name Explorer (or a second drag) takes in between is
/// retried rather than written over, which is why this does not simply hand back a name.
pub(crate) fn create_new_folder(dir: &Path) -> Result<PathBuf, String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    for _ in 0..64 {
        let candidate = free_name(dir, NEW_FOLDER_BASE);
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("could not create a new folder".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("floaty-placement-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_name_is_checked_the_way_explorer_checks_it() {
        // the spaces around a name are not part of it
        assert_eq!(checked_name("  report.txt  ").unwrap(), "report.txt");
        for bad in [
            "",
            "   ",
            ".",
            "..",
            "a/b",
            "a\\b",
            "a:b",
            "a?b",
            "a*b",
            "trailing.",
        ] {
            assert!(checked_name(bad).is_err(), "{bad:?} should be refused");
        }
    }

    /// The one numbering scheme: the unnumbered name is the first one, so collisions start at
    /// (2) — the way the shell numbers them, and (1) is not a name the shell ever produces.
    #[test]
    fn a_collision_gets_a_number_rather_than_overwriting() {
        let dir = scratch("numbers");
        assert_eq!(free_name(&dir, "report.txt"), dir.join("report.txt"));
        std::fs::write(dir.join("report.txt"), b"first").unwrap();
        assert_eq!(free_name(&dir, "report.txt"), dir.join("report (2).txt"));
        std::fs::write(dir.join("report (2).txt"), b"second").unwrap();
        assert_eq!(free_name(&dir, "report.txt"), dir.join("report (3).txt"));
        assert_eq!(free_name(&dir, "fresh.txt"), dir.join("fresh.txt"));
        // no extension numbers the same way
        std::fs::write(dir.join("notes"), b"x").unwrap();
        assert_eq!(free_name(&dir, "notes"), dir.join("notes (2)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn placing_moves_the_item_and_numbers_past_what_is_there() {
        let dir = scratch("place");
        let other = scratch("place-src");
        std::fs::write(other.join("report.txt"), b"mine").unwrap();
        std::fs::write(dir.join("report.txt"), b"not mine").unwrap();

        let landed = place_into(&other.join("report.txt"), &dir, None).unwrap();
        assert_eq!(landed, dir.join("report (2).txt"));
        assert_eq!(std::fs::read(&landed).unwrap(), b"mine");
        assert_eq!(
            std::fs::read(dir.join("report.txt")).unwrap(),
            b"not mine",
            "the file that was there is untouched"
        );

        // a name this code chose is checked before anything moves
        std::fs::write(other.join("second.txt"), b"x").unwrap();
        assert!(place_into(&other.join("second.txt"), &dir, Some("bad/name")).is_err());
        assert!(place_into(&other.join("second.txt"), &dir, Some("chosen.txt")).is_ok());
        assert!(dir.join("chosen.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn renaming_refuses_a_name_that_is_taken() {
        let dir = scratch("rename");
        std::fs::write(dir.join("one.txt"), b"1").unwrap();
        std::fs::write(dir.join("two.txt"), b"2").unwrap();

        assert_eq!(
            rename_in_place(&dir.join("one.txt"), "three.txt").unwrap(),
            dir.join("three.txt")
        );
        let refused = rename_in_place(&dir.join("three.txt"), "two.txt").unwrap_err();
        assert!(refused.contains("already exists"), "{refused}");
        assert!(dir.join("three.txt").exists() && dir.join("two.txt").exists());

        // asking for the name it already has is not a rename, and neither is asking for it again
        assert_eq!(
            rename_in_place(&dir.join("three.txt"), "three.txt").unwrap(),
            dir.join("three.txt")
        );
        assert_eq!(std::fs::read(dir.join("two.txt")).unwrap(), b"2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Windows keeps one case per name: changing only the case is the same item, renamed.
    #[test]
    fn renaming_only_the_case_is_allowed() {
        let dir = scratch("case");
        let lower = dir.join("marina.txt");
        std::fs::write(&lower, b"same").unwrap();
        let upper = rename_in_place(&lower, "MARINA.TXT").unwrap();
        assert_eq!(upper.file_name().unwrap().to_string_lossy(), "MARINA.TXT");
        assert_eq!(std::fs::read(&upper).unwrap(), b"same");
        // the disk agrees, and there is still exactly one file: asking for the case is the
        // rename, not a second file beside the first
        let on_disk: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(on_disk, vec!["MARINA.TXT".to_string()], "{on_disk:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_folder_is_the_next_free_one_and_the_create_reserves_it() {
        let dir = scratch("newfolder");
        assert_eq!(create_new_folder(&dir).unwrap(), dir.join("New folder"));
        assert_eq!(create_new_folder(&dir).unwrap(), dir.join("New folder (2)"));
        // a folder Explorer made by hand in between is stepped over, not written into
        std::fs::create_dir(dir.join("New folder (3)")).unwrap();
        assert_eq!(create_new_folder(&dir).unwrap(), dir.join("New folder (4)"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
