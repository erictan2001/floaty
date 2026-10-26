//! `asset_scope` — which directories the webview is allowed to load an asset from.
//!
//! Moved out of `settings_sync.rs` verbatim.

use crate::*;
use tauri::AppHandle;

/// Returns the directory path that should be allowed in the asset protocol scope.
///
/// If `p` is a file (or has a file extension such as `.model3.json`), its parent directory
/// is returned so that sibling assets (textures, physics, motions) are accessible.
pub(crate) fn scope_target_for_path(p: &std::path::Path) -> &std::path::Path {
    // A directory is the scope, even when its name has a dot: `acme.2026` is
    // not a file, and taking its parent would allow every other client folder.
    if p.is_dir() {
        return p;
    }
    let is_file_like = p.is_file() || p.extension().is_some();
    if is_file_like {
        p.parent().unwrap_or(p)
    } else {
        p
    }
}

/// Dynamically allow a directory (and all its subdirectories) in Tauri's asset protocol scope.
///
/// This provides sandboxed access to user-chosen directories (such as a custom `files_root`
/// or `live2d_root` on an external drive or secondary partition) without opening up the entire
/// filesystem with a wildcard scope (Principle of Least Privilege).
pub(crate) fn allow_asset_directory(app: &AppHandle, path: impl AsRef<std::path::Path>) {
    let p = path.as_ref();
    if p.as_os_str().is_empty() {
        return;
    }
    let target = scope_target_for_path(p);

    let scope = app.asset_protocol_scope();
    let mut allowed = false;

    if scope.allow_directory(target, true).is_ok() {
        allowed = true;
    }
    if let Ok(canon) = target.canonicalize() {
        if scope.allow_directory(&canon, true).is_ok() {
            allowed = true;
        }
    }
    if p != target {
        let _ = scope.allow_file(p);
        if let Ok(canon) = p.canonicalize() {
            let _ = scope.allow_file(&canon);
        }
    }

    if allowed {
        log_line(
            app,
            &format!("asset scope: allowed directory '{}'", target.display()),
        );
    } else {
        log_line(
            app,
            &format!(
                "asset scope: failed to allow directory '{}'",
                target.display()
            ),
        );
    }
}

/// Dynamically allow a directory or file path string in Tauri's asset protocol scope.
pub(crate) fn allow_asset_path(app: &AppHandle, path_str: &str) {
    let trimmed = path_str.trim();
    if !trimmed.is_empty() {
        allow_asset_directory(app, std::path::Path::new(trimmed));
    }
}
