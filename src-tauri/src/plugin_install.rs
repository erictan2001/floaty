//! Installing a plugin from an archive.
//!
//! A plugin is a folder, and until now getting one onto a desktop meant copying
//! that folder into `%APPDATA%\com.floaty.app\plugins` by hand — which is fine
//! for an author and awkward for everyone the author sends it to. This is the
//! other half: take a `.zip` (or a folder that is already on disk), unpack it
//! into a scratch directory, *validate it there*, and only then move it into the
//! plugins folder.
//!
//! Two rules do the work:
//!
//! - **Nothing lands in the plugins folder unvalidated.** A zip that turns out
//!   not to be a plugin costs a sentence in settings, rather than a folder name
//!   every later launch reports as rejected.
//! - **Replacing a plugin is a move, not a delete-then-write.** The installed
//!   copy is moved aside first and only dropped once the new one is in place, so
//!   a failure halfway through leaves the user with the plugin they had.

use crate::plugins::read_plugin;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What an install did, so settings can say it rather than guess.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInstall {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Files written into the plugins folder.
    pub files: usize,
    /// True when an earlier copy of this plugin was replaced.
    pub replaced: bool,
}

/// A plugin folder is small: a manifest and a module, plus maybe some art.
/// Anything past this is not a widget, and unpacking it is not our business.
const MAX_FILES: usize = 4000;
const MAX_BYTES: u64 = 128 * 1024 * 1024;

/// Install the plugin in `archive` into `plugins_dir`.
///
/// `archive` is a `.zip` or a folder. `replace` decides what happens when a
/// plugin with the same id is already installed: without it that is a refusal,
/// because overwriting someone's installed plugin (and its saved arrangements)
/// is not a thing to do by accident.
pub fn install_archive(
    archive: &Path,
    plugins_dir: &Path,
    replace: bool,
) -> Result<PluginInstall, String> {
    if !archive.exists() {
        return Err(format!("{} does not exist", archive.display()));
    }

    let scratch = scratch_dir()?;
    // Whatever happens next, the scratch copy goes: the plugin is either
    // installed or reported, and neither needs a second copy on disk.
    let result = install_into(archive, plugins_dir, replace, &scratch);
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

fn install_into(
    archive: &Path,
    plugins_dir: &Path,
    replace: bool,
    scratch: &Path,
) -> Result<PluginInstall, String> {
    std::fs::create_dir_all(scratch).map_err(|e| format!("no scratch folder: {e}"))?;

    // 1. Get the source folder: a folder taken as it is, a zip unpacked.
    let source = if archive.is_dir() {
        archive.to_path_buf()
    } else {
        let unpacked = scratch.join("unpacked");
        std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
        extract_zip(archive, &unpacked)?;
        unpacked
    };

    // 2. Find the plugin inside it (a zip usually wraps the folder, so one level
    //    down is the common shape) and validate it *before* it is installed.
    let found = find_plugin_dir(&source)?;
    let plugin = read_plugin(&found)?;

    // 3. Stage a copy, then move it into place.
    let staged = scratch.join(format!("{}.ready", plugin.id));
    let files = copy_dir(&found, &staged)?;

    let dest = plugins_dir.join(&plugin.id);
    std::fs::create_dir_all(plugins_dir).map_err(|e| format!("cannot use the plugins folder: {e}"))?;
    let backup = scratch.join(format!("{}.old", plugin.id));
    let had_previous = dest.exists();
    if had_previous {
        if !replace {
            return Err(format!(
                "{} is already installed{} — the archive holds v{}; install it again to replace that copy",
                plugin.id,
                installed_version(&plugins_dir.join(&plugin.id)),
                if plugin.version.is_empty() { "?" } else { &plugin.version }
            ));
        }
        move_dir(&dest, &backup)?;
    }

    if let Err(err) = move_dir(&staged, &dest) {
        // Put back what was there: a failed replace must not cost the plugin.
        if had_previous {
            let _ = std::fs::remove_dir_all(&dest);
            let _ = move_dir(&backup, &dest);
        }
        return Err(err);
    }
    if had_previous {
        let _ = std::fs::remove_dir_all(&backup);
    }

    Ok(PluginInstall {
        id: plugin.id,
        name: plugin.name,
        version: plugin.version,
        files,
        replaced: had_previous,
    })
}

/// The version of the copy already on disk, for the "already installed" message.
fn installed_version(dir: &Path) -> String {
    match read_plugin(dir) {
        Ok(p) if !p.version.is_empty() => format!(" (v{})", p.version),
        _ => String::new(),
    }
}

/// The folder inside an unpacked archive that holds `plugin.json`: the root
/// itself, or one level down (the usual case — a zip of a folder).
fn find_plugin_dir(root: &Path) -> Result<PathBuf, String> {
    if root.join("plugin.json").is_file() {
        return Ok(root.to_path_buf());
    }
    let mut found: Vec<PathBuf> = Vec::new();
    let entries = std::fs::read_dir(root).map_err(|e| format!("{}: {e}", root.display()))?;
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() && path.join("plugin.json").is_file() {
            found.push(path);
        }
    }
    match found.len() {
        0 => Err("no plugin.json in the archive (or in a folder directly inside it)".to_string()),
        1 => Ok(found.remove(0)),
        n => Err(format!(
            "the archive holds {n} plugins — install them one at a time ({})",
            found
                .iter()
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Unpack a zip with Windows' own extractor.
///
/// The paths travel in environment variables rather than in the command line:
/// PowerShell 5.1 mangles non-ASCII arguments, and a plugin path with an accent
/// or a CJK folder name in it is not a reason to fail.
fn extract_zip(zip: &Path, into: &Path) -> Result<(), String> {
    let extension = zip
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if extension != "zip" {
        return Err(format!(
            "{} is not a .zip (a plugin archive is a zip of the plugin folder)",
            zip.display()
        ));
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const NO_WINDOW: u32 = 0x08000000;
        let script = "$ErrorActionPreference = 'Stop'\n\
                      Expand-Archive -LiteralPath $env:FLOATY_ZIP -DestinationPath $env:FLOATY_DEST -Force\n";
        let output = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                script,
            ])
            .env("FLOATY_ZIP", zip)
            .env("FLOATY_DEST", into)
            .creation_flags(NO_WINDOW)
            .output()
            .map_err(|e| format!("could not run the extractor: {e}"))?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            let err = err.trim();
            return Err(format!(
                "the archive could not be unpacked: {}",
                if err.is_empty() { "unknown reason" } else { err }
            ));
        }
        refuse_traversal(into)?;
        Ok(())
    }

    #[cfg(not(windows))]
    {
        let _ = (zip, into);
        Err("installing a plugin from an archive is Windows-only for now".to_string())
    }
}

/// Belt and braces after an extraction: nothing may sit outside the scratch
/// folder, whatever the archive claimed (`Expand-Archive` already refuses `..`
/// entries, and this is the cheap second check that keeps that true if the
/// extractor is ever swapped).
fn refuse_traversal(root: &Path) -> Result<(), String> {
    for path in walk(root)? {
        if path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(format!(
                "the archive tried to write outside itself ({})",
                path.display()
            ));
        }
    }
    Ok(())
}

/// Every path under `root`, files and directories, without following links.
fn walk(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut queue = vec![root.to_path_buf()];
    while let Some(dir) = queue.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let meta = entry
                .metadata()
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if meta.is_symlink() {
                return Err(format!(
                    "the archive contains a link ({}), which a plugin has no business shipping",
                    path.display()
                ));
            }
            if meta.is_dir() {
                queue.push(path.clone());
            }
            out.push(path);
        }
    }
    Ok(out)
}

/// Copy a directory tree, and report how many files it wrote.
fn copy_dir(src: &Path, dst: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    let mut files = 0usize;
    let mut bytes = 0u64;
    let entries = std::fs::read_dir(src).map_err(|e| format!("{}: {e}", src.display()))?;
    for entry in entries.filter_map(|e| e.ok()) {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let meta = entry
            .metadata()
            .map_err(|e| format!("{}: {e}", from.display()))?;
        if meta.is_symlink() {
            return Err(format!("{} is a link; plugins are files", from.display()));
        }
        if meta.is_dir() {
            files += copy_dir(&from, &to)?;
        } else {
            bytes += meta.len();
            if bytes > MAX_BYTES {
                return Err("this plugin is larger than a plugin should be".to_string());
            }
            std::fs::copy(&from, &to)
                .map_err(|e| format!("{}: {e}", to.display()))?;
            files += 1;
            if files > MAX_FILES {
                return Err("this plugin has more files than a plugin should have".to_string());
            }
        }
    }
    Ok(files)
}

/// Move a directory: a rename where that works, a copy and a delete where the
/// two ends are on different drives.
fn move_dir(src: &Path, dst: &Path) -> Result<(), String> {
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_dir(src, dst)?;
    std::fs::remove_dir_all(src).map_err(|e| format!("{}: {e}", src.display()))
}

/// A private scratch folder for one install.
fn scratch_dir() -> Result<PathBuf, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("floaty-plugin-install-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("no scratch folder: {e}"))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "floaty-install-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn manifest(id: &str, version: &str) -> String {
        format!(
            r#"{{ "id": "{id}", "name": "Test {id}", "description": "a test plugin",
                 "version": "{version}", "apiVersion": 1, "entry": "index.js",
                 "size": {{ "w": 200, "h": 100 }} }}"#
        )
    }

    /// A plugin folder as an author would ship it: manifest + module + an asset.
    fn make_plugin(root: &Path, id: &str, version: &str) {
        write(&root.join("plugin.json"), &manifest(id, version));
        write(&root.join("index.js"), "export default { mount() {} };\n");
        write(&root.join("art/face.svg"), "<svg/>");
    }

    #[test]
    fn a_zip_of_a_folder_or_the_files_themselves_both_work() {
        let dir = temp("find");
        // the usual shape: the zip wraps the plugin folder
        let wrapped = dir.join("wrapped/countdown");
        make_plugin(&wrapped, "countdown", "1.0.0");
        assert_eq!(find_plugin_dir(&dir.join("wrapped")).unwrap(), wrapped);
        // and the shape where the files are at the top level
        let flat = dir.join("flat");
        make_plugin(&flat, "countdown", "1.0.0");
        assert_eq!(find_plugin_dir(&flat).unwrap(), flat);
        // nothing that looks like a plugin
        std::fs::create_dir_all(dir.join("empty/thing")).ok();
        assert!(find_plugin_dir(&dir.join("empty")).is_err());
        // two plugins in one archive is a question, not an install
        let two = dir.join("two");
        make_plugin(&two.join("a"), "aaa", "1.0.0");
        make_plugin(&two.join("b"), "bbb", "1.0.0");
        let err = find_plugin_dir(&two).unwrap_err();
        assert!(err.contains("2 plugins"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn installing_a_folder_copies_it_whole_and_then_refuses_to_clobber_it() {
        let dir = temp("install");
        let source = dir.join("countdown");
        make_plugin(&source, "countdown", "1.0.0");
        let plugins = dir.join("plugins");

        let done = install_archive(&source, &plugins, false).unwrap();
        assert_eq!(done.id, "countdown");
        assert_eq!(done.version, "1.0.0");
        assert_eq!(done.files, 3, "manifest, module and the asset");
        assert!(!done.replaced);
        assert!(plugins.join("countdown/plugin.json").is_file());
        assert!(plugins.join("countdown/art/face.svg").is_file());

        // the same plugin again: a refusal, and the installed copy is untouched
        let again = install_archive(&source, &plugins, false).unwrap_err();
        assert!(again.contains("already installed"), "{again}");
        assert!(plugins.join("countdown/index.js").is_file());

        // Replacing is a move, and the *tree* is replaced: v2's module is what
        // lands, an asset v2 dropped is gone, and one it added is there.
        write(
            &source.join("index.js"),
            "export default { mount() { /* v2 */ } };\n",
        );
        write(&source.join("plugin.json"), &manifest("countdown", "2.0.0"));
        std::fs::remove_file(source.join("art/face.svg")).unwrap();
        write(&source.join("extra.js"), "export const extra = 2;\n");
        let replaced = install_archive(&source, &plugins, true).unwrap();
        assert!(replaced.replaced);
        assert_eq!(replaced.version, "2.0.0");
        assert!(std::fs::read_to_string(plugins.join("countdown/index.js"))
            .unwrap()
            .contains("v2"));
        assert!(plugins.join("countdown/extra.js").is_file());
        assert!(
            !plugins.join("countdown/art/face.svg").exists(),
            "the installed copy is the archive, not the archive plus leftovers"
        );

        // The staging copy is never left where the *plugins folder* could see it:
        // a second folder holding this plugin's manifest is a duplicate id, and
        // every later launch would report the plugin as rejected.
        let stray: Vec<String> = std::fs::read_dir(&plugins)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "countdown")
            .collect();
        assert!(stray.is_empty(), "only the plugin itself is in there: {stray:?}");

        // and a folder that is not a plugin is refused before anything is copied
        let junk = dir.join("junk");
        write(&junk.join("readme.txt"), "hello");
        let err = install_archive(&junk, &plugins, true).unwrap_err();
        assert!(err.contains("no plugin.json"), "{err}");
        assert!(!plugins.join("junk").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_broken_manifest_is_reported_by_the_installer_not_by_the_next_launch() {
        let dir = temp("broken");
        let source = dir.join("oops");
        // a built-in id is not installable: it would shadow floaty's own widget
        write(&source.join("plugin.json"), &manifest("note", "1.0.0"));
        let plugins = dir.join("plugins");
        let err = install_archive(&source, &plugins, true).unwrap_err();
        assert!(err.contains("note"), "{err}");
        assert!(!plugins.join("note").exists(), "nothing was installed");

        // invalid JSON says so, too
        write(&source.join("plugin.json"), "{ not json");
        let err = install_archive(&source, &plugins, true).unwrap_err();
        assert!(err.contains("JSON"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_non_zip_archive_and_a_missing_path_are_refused_with_a_reason() {
        let dir = temp("badzip");
        let not_a_zip = dir.join("plugin.rar");
        write(&not_a_zip, "not a zip at all");
        let err = install_archive(&not_a_zip, &dir.join("plugins"), false).unwrap_err();
        assert!(err.contains("not a .zip"), "{err}");
        let err = install_archive(&dir.join("nope.zip"), &dir.join("plugins"), false).unwrap_err();
        assert!(err.contains("does not exist"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
