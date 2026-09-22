//! `plugins` — moved out of lib.rs verbatim.
//!
//! Visibility is `pub(crate)` because the rest of the crate calls in across the module
//! boundary now; nothing here changed shape on the way out.

use crate::*;
use plugins::PluginInfo;
use tauri::{AppHandle, Emitter, Manager};

// ---------- plugins ----------

pub(crate) fn set_plugin_enabled(app: &AppHandle, id: &str, enabled: bool) {
    let mut s = load_settings(app);
    if enabled {
        s.disabled.retain(|d| d != id);
    } else if !s.disabled.iter().any(|d| d == id) {
        s.disabled.push(id.to_string());
    }
    if let Ok(json) = serde_json::to_string_pretty(&s) {
        write_text_atomic(&settings_file(app), &json);
    }
    app.emit("floaty-plugins-changed", &plugins::manifest(&s.disabled)).ok();
}

#[tauri::command]
pub(crate) fn floaty_plugins(app: AppHandle) -> Vec<PluginInfo> {
    let settings = load_settings(&app);
    let mut list = plugins::manifest(&settings.disabled);
    // Built-ins are floaty's own code and always approved; an installed plugin is
    // approved by *fingerprint*, so the approval covers the files the user read.
    let dir = plugins_dir(&app);
    for info in list.iter_mut() {
        if info.source == "installed" {
            info.trusted = plugin_approved(&settings, &dir.join(&info.id), &info.id);
        }
    }
    list
}

/// Approve what was already on the desktop before approvals existed.
///
/// The trust model arrived after these plugins did: their folders were installed
/// without a fingerprint being recorded, so on the first launch with it every one of
/// them reads as "not approved" and its widgets stop mounting — code the user chose
/// and has been looking at for months. A plugin that has a floatie of its kind on the
/// desktop is recorded as approved once, from the files it is running from.
///
/// A folder with no floaties is left alone: that is the case where "it appeared and
/// nobody approved it" is exactly the thing worth asking about.
pub(crate) fn migrate_plugin_trust(app: &AppHandle) {
    let dir = plugins_dir(app);
    let (installed, _) = plugins::install_from(&dir);
    if installed.is_empty() {
        return;
    }
    let kinds: std::collections::HashSet<String> = app
        .state::<AppState>()
        .0
        .lock()
        .map(|guard| guard.widgets.values().map(|rec| rec.kind.clone()).collect())
        .unwrap_or_default();
    let mut settings = load_settings(app);
    let mut changed = false;
    for entry in installed {
        // the manifest list answers "countdown v1.0.0"
        let id = entry.split_whitespace().next().unwrap_or("").to_string();
        if id.is_empty() || settings.plugin_trust.contains_key(&id) || !kinds.contains(&id) {
            continue;
        }
        if let Ok(fingerprint) = plugin_install::folder_fingerprint(&dir.join(&id)) {
            settings.plugin_trust.insert(id.clone(), fingerprint);
            changed = true;
            log_line(
                app,
                &format!("plugins: approved {id} by migration — it was already on the desktop"),
            );
        }
    }
    if changed {
        write_settings(app, &settings);
    }
}

/// Is this plugin's *current* code the code the user approved?
pub(crate) fn plugin_approved(settings: &FloatSettings, dir: &std::path::Path, id: &str) -> bool {
    let Some(approved) = settings.plugin_trust.get(id) else {
        return false;
    };
    // A folder that cannot be read is not approved: failing open here would mean a
    // plugin whose files were replaced by something unreadable still runs.
    plugin_install::folder_fingerprint(dir)
        .map(|now| now == *approved)
        .unwrap_or(false)
}

/// Approve an installed plugin, or take the approval back.
///
/// Approving records the fingerprint of the folder *as it is now*; withdrawing
/// forgets it, and its widgets stop mounting (they are not removed — nothing here
/// touches the desk, and approving again brings them back).
#[tauri::command]
pub(crate) fn floaty_plugin_trust(id: String, trusted: bool, app: AppHandle) -> Result<(), String> {
    if plugins::find(&id).is_some() {
        return Err(format!("{id} is one of floaty's own widgets"));
    }
    let dir = plugins_dir(&app).join(&id);
    if !dir.is_dir() {
        return Err(format!("{id} is not installed"));
    }
    let fingerprint = if trusted {
        Some(plugin_install::folder_fingerprint(&dir)?)
    } else {
        None
    };
    let mut settings = load_settings(&app);
    match fingerprint {
        Some(fingerprint) => {
            settings.plugin_trust.insert(id.clone(), fingerprint);
        }
        None => {
            settings.plugin_trust.remove(&id);
        }
    }
    write_settings(&app, &settings);
    log_line(
        &app,
        &format!(
            "plugins: {} {id}",
            if trusted {
                "approved"
            } else {
                "approval withdrawn for"
            }
        ),
    );
    reload_plugin_windows(&app);
    Ok(())
}


/// Where user plugins live: one folder each, `plugin.json` + its module.
pub(crate) fn plugins_dir(app: &AppHandle) -> std::path::PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join(plugins::PLUGINS_DIR_NAME)
}

/// The plugins folder (created if missing) so the settings window can offer
/// "install by dropping a folder in here".
#[tauri::command]
pub(crate) fn floaty_plugins_dir(app: AppHandle) -> Result<String, String> {
    let dir = plugins_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.to_string_lossy().to_string())
}

/// Show that folder in File Explorer.
#[tauri::command]
pub(crate) fn floaty_open_plugins_dir(app: AppHandle) -> Result<(), String> {
    let dir = plugins_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    shell_ops::reveal(&dir.to_string_lossy())
}

/// Make both windows pick up plugin changes without a restart.
///
/// The overlay and settings each cache the manifest *and* the modules they have
/// imported, so a new plugin is only real once both reload — which is why every
/// path that changes the plugins folder ends here.
pub(crate) fn reload_plugin_windows(app: &AppHandle) {
    for label in ["desktop-overlay", "settings"] {
        if let Some(w) = app.get_webview_window(label) {
            let _ = w.eval("location.reload()");
        }
    }
    app.emit("floaty-plugins-changed", &plugins::manifest(&load_settings(app).disabled))
        .ok();
}

/// Install a plugin from a `.zip` (or from a folder the user picked).
///
/// The archive is unpacked and validated in a scratch folder before anything
/// reaches the plugins folder, so the two failures worth distinguishing — "that
/// zip is not a plugin" and "that plugin is already installed" — both arrive as
/// a message in settings instead of a folder name every later launch rejects.
#[tauri::command]
pub(crate) async fn floaty_install_plugin(
    path: String,
    replace: bool,
    app: AppHandle,
) -> Result<plugin_install::PluginInstall, String> {
    let archive = std::path::PathBuf::from(&path);
    let dir = plugins_dir(&app);
    // Unpacking runs PowerShell and copies files: off the main thread, like
    // every other command that touches the disk for a second.
    let report = tauri::async_runtime::spawn_blocking(move || {
        plugin_install::install_archive(&archive, &dir, replace)
    })
    .await
    .map_err(|e| e.to_string())??;

    // Installing from an archive *is* the approval: the dialog showed the user what
    // is in it, who wrote it, what it replaces and which version of the plugin
    // contract it wants. A folder that appears in the plugins directory by some
    // other route is unapproved and stays that way until it is approved in the
    // Plugins tab.
    {
        let dir = plugins_dir(&app).join(&report.id);
        let mut settings = load_settings(&app);
        if let Ok(fingerprint) = plugin_install::folder_fingerprint(&dir) {
            settings.plugin_trust.insert(report.id.clone(), fingerprint);
            write_settings(&app, &settings);
        }
        log_line(
            &app,
            &format!("plugins: approved {} v{} after the install report", report.id, report.version),
        );
    }

    let (installed, rejected) = plugins::install_from(&plugins_dir(&app));
    log_line(
        &app,
        &format!(
            "plugins: {} {} v{} from {} ({} files, {} installed [{}]{})",
            if report.replaced { "replaced" } else { "installed" },
            report.id,
            report.version,
            path,
            report.files,
            installed.len(),
            installed.join(", "),
            if rejected.is_empty() {
                String::new()
            } else {
                format!(", {} rejected [{}]", rejected.len(), rejected.join("; "))
            }
        ),
    );
    reload_plugin_windows(&app);
    Ok(report)
}

/// What a plugin archive would do, without doing it.
///
/// The install confirm is built on this: an archive is described by the same code
/// that would install it, so what is offered and what lands cannot disagree.
#[tauri::command]
pub(crate) async fn floaty_inspect_plugin(path: String, app: AppHandle) -> Result<plugin_install::ArchiveReport, String> {
    let archive = std::path::PathBuf::from(&path);
    let dir = plugins_dir(&app);
    tauri::async_runtime::spawn_blocking(move || plugin_install::inspect_archive(&archive, &dir))
        .await
        .map_err(|e| e.to_string())?
}

/// Take a plugin off the desktop, folder and all.
///
/// `remove_widgets` is the second half of a question, never an assumption: a plugin
/// with floaties on the desktop cannot simply vanish, because its records would
/// then be drawn by a build that has no idea what they are. So the caller asks —
/// "remove its 3 floaties too?" — and either answer is honest.
#[tauri::command]
pub(crate) async fn floaty_uninstall_plugin(
    id: String,
    remove_widgets: bool,
    app: AppHandle,
) -> Result<plugin_install::PluginUninstall, String> {
    // Floaty's own widgets come first: "you cannot remove this one" is a more
    // fundamental answer than "it has floaties on the desktop", and letting the
    // widget guard answer first hid it (measured: removing `note` complained about
    // the note floatie instead of about `note` being built in).
    if plugins::find(&id).is_some() {
        return Err(format!("{id} is one of floaty's own widgets — it cannot be removed"));
    }

    let widgets: Vec<String> = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        guard
            .widgets
            .values()
            .filter(|rec| rec.kind == id)
            .map(|rec| rec.id.clone())
            .collect()
    };
    if !widgets.is_empty() && !remove_widgets {
        return Err(format!(
            "{} floatie(s) of that kind are on the desktop — remove them with it, or none",
            widgets.len()
        ));
    }

    let dir = plugins_dir(&app);
    let id_for_work = id.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
        plugin_install::uninstall(&id_for_work, &dir)
    })
    .await
    .map_err(|e| e.to_string())??;

    // The records go the way any removal goes: out of the store, tombstoned so a
    // dying window cannot put them back, and hidden.
    {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        for widget_id in &widgets {
            guard.widgets.remove(widget_id);
            guard.dead.insert(widget_id.clone());
        }
    }
    for widget_id in &widgets {
        hide_widget(&app, widget_id);
    }
    persist(&app);

    // Its kind is no longer in the manifest, so the records that used it are gone
    // too — and both windows have to be told.
    let (installed, rejected) = plugins::install_from(&plugins_dir(&app));
    log_line(
        &app,
        &format!(
            "plugins: uninstalled {} ({} files -> {}) and removed {} floatie(s); {} still installed [{}]{}",
            report.id,
            report.files,
            report.recycled_to,
            widgets.len(),
            installed.len(),
            installed.join(", "),
            if rejected.is_empty() {
                String::new()
            } else {
                format!(", {} rejected [{}]", rejected.len(), rejected.join("; "))
            }
        ),
    );
    reload_plugin_windows(&app);
    Ok(report)
}

/// Re-scan the plugins folder without restarting: what authors do after editing
/// a manifest. Returns the rejections so the settings window can show them.
#[tauri::command]
pub(crate) fn floaty_rescan_plugins(app: AppHandle) -> Result<Vec<String>, String> {
    let (installed, rejected) = plugins::install_from(&plugins_dir(&app));
    log_line(
        &app,
        &format!(
            "plugins: rescanned, {} installed [{}]{}",
            installed.len(),
            installed.join(", "),
            if rejected.is_empty() {
                String::new()
            } else {
                format!(", {} rejected [{}]", rejected.len(), rejected.join("; "))
            }
        ),
    );
    reload_plugin_windows(&app);
    Ok(rejected)
}

/// Show (enable) or hide (disable) every widget of one kind, in whichever mode
/// is running. Spawning windows here while the overlay is up was what put a
/// second copy of every file icon on the desktop.
pub(crate) fn apply_plugin_visibility(app: &AppHandle, kind: &str, enabled: bool) {
    let ids: Vec<String> = app
        .state::<AppState>()
        .0
        .lock()
        .map(|g| {
            g.widgets
                .values()
                .filter(|r| r.kind == kind)
                .map(|r| r.id.clone())
                .collect()
        })
        .unwrap_or_default();
    for wid in ids {
        if enabled {
            let rec: Option<WidgetRecord> = app
                .state::<AppState>()
                .0
                .lock()
                .ok()
                .and_then(|g| g.widgets.get(&wid).cloned());
            if let Some(r) = rec {
                show_widget(app, &r);
            }
        } else {
            hide_widget(app, &wid);
        }
    }
}

#[tauri::command]
pub(crate) fn floaty_set_plugin_enabled(id: String, enabled: bool, app: AppHandle) {
    log_line(&app, &format!("plugin {id} enabled={enabled}"));
    set_plugin_enabled(&app, &id, enabled);
    apply_plugin_visibility(&app, &id, enabled);
}

#[tauri::command]
pub(crate) fn floaty_add_launcher(name: String, path: String, app: AppHandle) -> Result<WidgetRecord, String> {
    if path.trim().is_empty() {
        return Err("empty path".into());
    }
    // App icons live in the pointed root like everything else: the launcher gets
    // a shortcut file there (a copy when the app is already floated through one,
    // which keeps its arguments and icon) and the record points at that file.
    let (name, path) = app_into_root(&app, &name, &path);
    let data = serde_json::json!({ "name": name, "target": path.clone() });
    // spawn near the top so it falls with gravity on arrival
    create_record_with(&app, kind_for_path(std::path::Path::new(&path), false), data, Some((200, 40)))
}
