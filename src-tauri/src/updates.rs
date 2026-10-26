//! `updates` — the manual update check and install.
//!
//! Moved out of `settings_sync.rs` verbatim.

use crate::*;
use tauri::{AppHandle, Emitter};

/// What an update check found, in the terms the settings row can show.
#[derive(serde::Serialize)]
pub(crate) struct UpdateReport {
    pub(crate) available: bool,
    pub(crate) current: String,
    pub(crate) version: Option<String>,
    pub(crate) notes: Option<String>,
    /// Why the check could not be made — a published app that cannot reach GitHub should
    /// say so rather than looking up to date.
    pub(crate) error: Option<String>,
}

/// Ask the release feed whether there is a newer floaty.
///
/// Rust rather than the updater's JS API on purpose: the check and the install are not
/// things the *page* should be trusted with, and keeping them here means no new webview
/// permission and no new global key.
#[tauri::command]
pub(crate) async fn floaty_check_update(app: AppHandle) -> UpdateReport {
    use tauri_plugin_updater::UpdaterExt;
    let current = app.package_info().version.to_string();
    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(e) => {
            return UpdateReport {
                available: false,
                current,
                version: None,
                notes: None,
                error: Some(format!("no update feed is configured ({e})")),
            }
        }
    };
    match updater.check().await {
        Ok(Some(update)) => UpdateReport {
            available: true,
            current,
            version: Some(update.version.clone()),
            notes: update.body.clone(),
            error: None,
        },
        Ok(None) => UpdateReport {
            available: false,
            current,
            version: None,
            notes: None,
            error: None,
        },
        Err(e) => UpdateReport {
            available: false,
            current,
            version: None,
            notes: None,
            // Offline, a corporate proxy, a manifest that does not parse: all of it is
            // "could not check", none of it is "you are up to date".
            error: Some(e.to_string()),
        },
    }
}

/// Download and install the update the check found, then restart into it.
#[tauri::command]
pub(crate) async fn floaty_install_update(app: AppHandle) -> Result<String, String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "there is no newer version".to_string())?;
    let version = update.version.clone();
    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    let _ = app.emit(
        "floaty-update-installed",
        serde_json::json!({ "version": version }),
    );
    log_line(&app, &format!("update: {version} installed, restarting"));
    app.restart();
}
