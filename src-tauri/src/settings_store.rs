//! `settings_store` — the settings type, its file, and the one writer.
//!
//! Moved out of `settings_sync.rs` verbatim.

use crate::*;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

/// Set when the settings file exists but cannot be read. A save must not replace
/// it with defaults before the user can get the file back.
static SETTINGS_READ_ONLY: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FloatSettings {
    #[serde(default = "default_gravity")]
    pub(crate) gravity: f64,
    #[serde(default = "default_bounce")]
    pub(crate) bounce: f64,
    #[serde(default = "default_single_click")]
    pub(crate) single_click: String,
    #[serde(default = "default_double_click")]
    pub(crate) double_click: String,
    /// live2d model library root picked by the user
    #[serde(default)]
    pub(crate) live2d_root: String,
    /// root directory where files and folders should be floaties
    #[serde(default)]
    pub(crate) files_root: String,
    /// plugin kinds whose windows stay closed
    #[serde(default)]
    pub(crate) disabled: Vec<String>,
    /// keep widgets visible on desktop when Show Desktop (Win+D) is triggered
    #[serde(default = "default_stay_on_desktop")]
    pub(crate) stay_on_desktop: bool,
    /// launch floaty when the user signs in (a Run entry in the registry)
    #[serde(default = "default_start_on_boot")]
    pub(crate) start_on_boot: bool,
    /// percentage of floaties that participate in animation (0 to 100)
    #[serde(default = "default_animated_ratio")]
    pub(crate) animated_ratio: f64,
    /// animation mode: "wave", "sync", "gentle", "static"
    #[serde(default = "default_animation_mode")]
    pub(crate) animation_mode: String,
    /// how far a resting icon rises, in px — the same travel in every mode.
    /// (Was multiplied by a second `floatiness` slider, which is folded in once
    /// by `migrate_float_settings`.)
    #[serde(default = "default_float_amplitude")]
    pub(crate) float_amplitude: f64,
    /// seconds per bob, in every mode
    #[serde(default = "default_float_period")]
    pub(crate) float_period: f64,
    /// wave only: how much of the cycle separates one icon from the next, as a
    /// percentage of one cycle
    #[serde(default = "default_float_spread")]
    pub(crate) float_spread: f64,
    /// ask before removing a floatie (file, folder or widget)
    #[serde(default = "default_confirm_remove")]
    pub(crate) confirm_remove: bool,
    /// visualizer sensitivity multiplier
    #[serde(default = "default_viz_gain")]
    pub(crate) viz_gain: f64,
    /// visualizer redraw rate (frames per second, while audio plays)
    #[serde(default = "default_viz_fps")]
    pub(crate) viz_fps: f64,
    /// The key that opens the launcher palette, e.g. "Ctrl+Alt+Space".
    #[serde(default = "default_palette_shortcut")]
    pub(crate) palette_shortcut: String,
    /// Get out of the way: hide the desktop while a fullscreen app is in front.
    #[serde(default = "default_true")]
    pub(crate) hide_in_fullscreen: bool,
    /// ...and stop animating when nobody has touched the machine for a while.
    #[serde(default = "default_true")]
    pub(crate) quiet_when_idle: bool,
    /// How long "a while" is, in minutes. 0 means never.
    #[serde(default = "default_idle_minutes")]
    pub(crate) idle_minutes: u32,
    /// Plugin id -> the fingerprint of the folder the user approved, for plugins
    /// they installed themselves. A plugin whose files have changed since is asked
    /// about again: approving code is not approving whatever replaces it later.
    #[serde(default)]
    pub(crate) plugin_trust: std::collections::HashMap<String, String>,
    /// system monitor sample period (milliseconds)
    #[serde(default = "default_sysmon_interval")]
    pub(crate) sysmon_interval: f64,
    /// version of the icon resolution pipeline the stored icons came from.
    /// Bumping `ICON_PIPELINE` re-resolves every stored icon once, which is how
    /// icons produced by an older (worse) pipeline get replaced.
    #[serde(default = "default_icon_pipeline")]
    pub(crate) icon_pipeline: u32,
    /// Whether the user has answered the first-run question about the root: that
    /// floaty treats its contents as the desktop, and that a merge moves real files
    /// inside it (ADR 0004). Unset on every store written before the guard existed,
    /// which is exactly what makes the first launch block — and it is one-way: a
    /// confirmation step the user has learned to click through is worse than none,
    /// so nothing ever clears it again.
    #[serde(default)]
    pub(crate) root_confirmed: bool,
}

/// The folder names the first-run screen lists before it stops and says "and N more".
pub(crate) const ROOT_GUARD_NAMES: usize = 8;

/// Whether this launch was told to treat the root as already confirmed.
///
/// The headless probes drive a *running* app, and a fresh profile has no answer on
/// file, so every probe run would otherwise meet the blocking screen and measure
/// nothing. `FLOATY_ROOT_CONFIRMED=1` in the environment pre-seeds the answer for a
/// probe; a real launch never has it set, so a stranger still gets the guard. A bare
/// `FLOATY_ROOT_CONFIRMED=0` is how a probe asks for the *unconfirmed* state on a
/// machine that has already answered — that is what makes the screen itself testable.
///
/// It is scoped to the launch and to the question: it never reaches the store, so a
/// probe run cannot leave the user's install answered (a probe that writes into the
/// user's profile is a bug this project already knows about). A probe that means to
/// answer for good calls `floaty_confirm_root`, which is the real thing.
pub(crate) fn root_confirmed_from_env() -> Option<bool> {
    match std::env::var("FLOATY_ROOT_CONFIRMED") {
        Ok(v) if v.is_empty() || v == "0" => Some(false),
        Ok(_) => Some(true),
        Err(_) => None,
    }
}

/// The answer to give this launch: what the store says, unless the launch was told
/// what to answer.
pub(crate) fn root_confirmed_for_this_launch(settings: &FloatSettings) -> bool {
    root_confirmed_from_env().unwrap_or(settings.root_confirmed)
}

/// What the first-run screen shows about the folder floaty is about to adopt.
///
/// Counted the way the desktop would show it — one level deep, folders and files, and
/// which of the files are launchable — because that is what "adopting this folder"
/// means to the person reading it.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct RootGuard {
    /// Whether the user has already answered (ADR 0004): false means the screen.
    pub(crate) confirmed: bool,
    /// The folder being looked at: the stored root, or the one asked about.
    pub(crate) root: String,
    /// Whether that folder is there at all. "Missing" and "empty" are different
    /// things to be told, so they are different fields.
    pub(crate) exists: bool,
    pub(crate) items: usize,
    pub(crate) files: usize,
    pub(crate) folders: usize,
    /// Files that are launchable — the ones that become app floaties rather than
    /// file floaties.
    pub(crate) apps: usize,
    /// The first few names, in the order the filesystem lists them.
    pub(crate) names: Vec<String>,
    /// How many names `names` left out.
    pub(crate) more: usize,
}

/// Count what is in a folder, one level deep.
pub(crate) fn root_preview(root: &str, confirmed: bool) -> RootGuard {
    let trimmed = root.trim();
    let dir = std::path::PathBuf::from(trimmed);
    let mut guard = RootGuard {
        confirmed,
        root: trimmed.to_string(),
        exists: dir.is_dir(),
        items: 0,
        files: 0,
        folders: 0,
        apps: 0,
        names: Vec::new(),
        more: 0,
    };
    // A folder that is not there is not an empty folder: say so rather than show
    // an empty list. An unreadable one is the same kind of answer.
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return guard;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        guard.items += 1;
        if path.is_dir() {
            guard.folders += 1;
        } else {
            guard.files += 1;
            if is_launchable_target(&path) {
                guard.apps += 1;
            }
        }
        if guard.names.len() < ROOT_GUARD_NAMES {
            guard
                .names
                .push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    guard.more = guard.items.saturating_sub(guard.names.len());
    guard
}

/// The first-run guard's question: the folder floaty would adopt, and what is in it.
///
/// `root` lets the screen look at a folder the user has just picked *without* adopting
/// it: nothing is stored until `floaty_confirm_root` is answered.
#[tauri::command]
pub(crate) fn floaty_root_guard(root: String, app: AppHandle) -> RootGuard {
    let settings = load_settings(&app);
    let confirmed = root_confirmed_for_this_launch(&settings);
    let asked = if root.trim().is_empty() {
        settings.files_root
    } else {
        root
    };
    root_preview(&asked, confirmed)
}

/// The user has answered the first-run question: adopt this folder as the desktop.
///
/// `root` is the folder the screen was showing, when the user picked one there instead
/// of pointing at it from the settings window. The flag is set whatever the answer was
/// about the folder — "there is nothing in it yet" is still an answer, and the next
/// root the user picks is a *change*, which warns and proceeds rather than blocking.
#[tauri::command]
pub(crate) fn floaty_confirm_root(root: String, app: AppHandle) -> RootGuard {
    let mut settings = load_settings(&app);
    let picked = root.trim().to_string();
    let changed = !picked.is_empty() && !picked.eq_ignore_ascii_case(settings.files_root.trim());
    if changed {
        settings.files_root = picked;
    }
    settings.root_confirmed = true;
    write_settings(&app, &settings);
    if !settings.files_root.trim().is_empty() {
        allow_asset_path(&app, &settings.files_root);
    }
    if changed {
        // Adopted now means mirrored now: waiting for the next launch would leave the
        // user looking at the empty desktop they just agreed to.
        let _ = sync_root(None, app.clone(), true, None);
    }
    app.emit("floaty-settings-changed", &settings).ok();
    log_line(
        &app,
        &format!(
            "first run: root confirmed — {}",
            if settings.files_root.trim().is_empty() {
                "no folder set yet".to_string()
            } else {
                settings.files_root.trim().to_string()
            }
        ),
    );
    root_preview(&settings.files_root, true)
}

/// Bump when the icon resolver changes what it produces, so already-stored
/// icons are refreshed once. v2: shell item image + alpha-preserving PNG.
/// v3: pick the route whose artwork actually fills the frame.
pub(crate) const ICON_PIPELINE: u32 = 5;

/// The launcher key. Ctrl+Alt+Space is free on a stock Windows, and unlike
/// Alt+Space (the window menu) or Ctrl+Space (the IME switcher on a CJK install)
/// it is not spoken for.
pub(crate) fn default_palette_shortcut() -> String {
    "Ctrl+Alt+Space".to_string()
}

pub(crate) fn default_true() -> bool {
    true
}

/// Ten minutes of no input at all — not ten minutes of the *app* being idle, which is a
/// different thing and would flatten the desktop while somebody reads a web page.
pub(crate) fn default_idle_minutes() -> u32 {
    10
}

pub(crate) fn default_icon_pipeline() -> u32 {
    // Old settings files predate the field; 0 means "resolve everything once".
    0
}

pub(crate) fn default_stay_on_desktop() -> bool {
    true
}

pub(crate) fn default_start_on_boot() -> bool {
    false
}

pub(crate) fn default_float_amplitude() -> f64 {
    6.0
}

pub(crate) fn default_float_period() -> f64 {
    10.0
}

pub(crate) fn default_float_spread() -> f64 {
    10.0
}

pub(crate) fn default_confirm_remove() -> bool {
    true
}

pub(crate) fn default_viz_gain() -> f64 {
    1.0
}

pub(crate) fn default_viz_fps() -> f64 {
    30.0
}

pub(crate) fn default_sysmon_interval() -> f64 {
    1000.0
}

pub(crate) fn default_animated_ratio() -> f64 {
    100.0
}

pub(crate) fn default_animation_mode() -> String {
    "wave".to_string()
}

pub(crate) fn default_gravity() -> f64 {
    2600.0
}
pub(crate) fn default_bounce() -> f64 {
    0.45
}
pub(crate) fn default_single_click() -> String {
    "drop".to_string()
}
pub(crate) fn default_double_click() -> String {
    "launch".to_string()
}

pub(crate) fn settings_file(app: &AppHandle) -> std::path::PathBuf {
    app_data_dir(app).join("floaty-settings.json")
}

/// Write the settings the way the settings pane does — pretty JSON, written
/// atomically — for the few commands that change them on their own.
///
/// The previous good copy is kept as `floaty-settings.bak` after a successful
/// write. `read_json_with_backup` already knows how to read that name; before
/// this, nothing ever wrote it, so one wrong-typed field reset every setting.
pub(crate) fn write_settings(app: &AppHandle, settings: &FloatSettings) {
    if SETTINGS_READ_ONLY.load(std::sync::atomic::Ordering::SeqCst) {
        log_line(
            app,
            "settings: not writing — the existing file could not be read",
        );
        return;
    }
    let Some(json) = serde_json::to_string_pretty(settings).ok() else {
        return;
    };
    let path = settings_file(app);
    if !write_text_atomic(&path, &json) {
        log_line(app, "settings: write failed");
        return;
    }
    // The .bak is the last *successful* settings file, not whatever malformed
    // text the main file held before this save.
    let _ = std::fs::copy(&path, path.with_extension("bak"));
}

/// Fold the settings a file written before the float sliders were made honest.
///
/// `floatiness` multiplied `float_amplitude`, so the two rows moved the same
/// thing and neither number was the travel; and `float_spread` was a percentage
/// of a stagger that never reached the tiles (the delay lost the cascade), on a
/// 0-200 scale whose 100 meant "15% of a cycle between neighbours". An existing
/// look therefore has to survive both changes: the multiplier is folded into
/// the height once, and the spread moves onto its new scale — percent of one
/// cycle between neighbouring icons.
///
/// Both results are rounded to the step of the slider that will show them (0.5px
/// and 1%), because a range input snaps to its own grid: a file holding 12.4
/// would otherwise draw its handle at 12 and write 12 the moment it was touched.
///
/// The presence of `floatiness` is what marks a file as old, and it is dropped
/// so this runs exactly once.
pub(crate) fn migrate_float_settings(value: &mut serde_json::Value) {
    let Some(map) = value.as_object_mut() else {
        return;
    };
    let Some(legacy) = map.remove("floatiness").and_then(|v| v.as_f64()) else {
        return;
    };
    if legacy.is_finite() && legacy > 0.0 {
        if let Some(amp) = map.get("float_amplitude").and_then(|v| v.as_f64()) {
            let folded = ((amp * legacy).clamp(0.0, 16.0) * 2.0).round() / 2.0;
            map.insert("float_amplitude".into(), serde_json::json!(folded));
        }
    }
    if let Some(spread) = map.get("float_spread").and_then(|v| v.as_f64()) {
        let rescaled = (spread * 0.15).clamp(0.0, 100.0).round();
        map.insert("float_spread".into(), serde_json::json!(rescaled));
    }
}

pub(crate) fn load_settings(app: &AppHandle) -> FloatSettings {
    let path = settings_file(app);
    // An unreadable file is not a settings file with defaults: writing the
    // defaults would replace the values the app could not read. Keep the file
    // and stay read-only until the next launch.
    if path.exists() && std::fs::read_to_string(&path).is_err() {
        SETTINGS_READ_ONLY.store(true, std::sync::atomic::Ordering::SeqCst);
        log_line(
            app,
            "settings: the existing file could not be read; using defaults read-only",
        );
    }
    // Read through `Value` first so an old file can be migrated; anything that
    // is not an object falls back to the plain typed read below.
    let parsed: Option<FloatSettings> = read_json_with_backup::<serde_json::Value>(&path)
        .map(|(mut value, _)| {
            migrate_float_settings(&mut value);
            value
        })
        .and_then(|value| serde_json::from_value::<FloatSettings>(value).ok())
        .or_else(|| read_json_with_backup::<FloatSettings>(&path).map(|(s, _)| s));
    match parsed {
        Some(s) => FloatSettings {
            single_click: if s.single_click.is_empty() {
                default_single_click()
            } else {
                s.single_click
            },
            double_click: if s.double_click.is_empty() {
                default_double_click()
            } else {
                s.double_click
            },
            animation_mode: if s.animation_mode.is_empty() {
                default_animation_mode()
            } else {
                s.animation_mode
            },
            palette_shortcut: if s.palette_shortcut.trim().is_empty() {
                default_palette_shortcut()
            } else {
                s.palette_shortcut
            },
            ..s
        },
        None => FloatSettings {
            gravity: default_gravity(),
            bounce: default_bounce(),
            single_click: default_single_click(),
            double_click: default_double_click(),
            live2d_root: String::new(),
            files_root: String::new(),
            disabled: Vec::new(),
            stay_on_desktop: default_stay_on_desktop(),
            start_on_boot: default_start_on_boot(),
            animated_ratio: default_animated_ratio(),
            animation_mode: default_animation_mode(),
            float_amplitude: default_float_amplitude(),
            float_period: default_float_period(),
            float_spread: default_float_spread(),
            confirm_remove: default_confirm_remove(),
            viz_gain: default_viz_gain(),
            viz_fps: default_viz_fps(),
            sysmon_interval: default_sysmon_interval(),
            palette_shortcut: default_palette_shortcut(),
            hide_in_fullscreen: default_true(),
            quiet_when_idle: default_true(),
            idle_minutes: default_idle_minutes(),
            plugin_trust: std::collections::HashMap::new(),
            icon_pipeline: default_icon_pipeline(),
            root_confirmed: false,
        },
    }
}

#[tauri::command]
pub(crate) async fn floaty_get_settings(app: AppHandle) -> FloatSettings {
    load_settings(&app)
}

#[tauri::command]
pub(crate) fn floaty_set_settings(settings: FloatSettings, app: AppHandle) -> FloatSettings {
    // icon_pipeline is the backend's own bookkeeping (it decides whether stored
    // icons need re-resolving); the settings UI does not know about it, so keep
    // whatever is already on disk instead of letting a save reset it to 0.
    let stored = load_settings(&app);
    let previous_root = stored.files_root.clone();
    let stored_pipeline = stored.icon_pipeline;
    let mut s = FloatSettings {
        gravity: settings.gravity.clamp(0.0, 8000.0),
        bounce: settings.bounce.clamp(0.0, 0.95),
        single_click: match settings.single_click.as_str() {
            "hop" | "nothing" => settings.single_click,
            _ => "drop".to_string(),
        },
        double_click: match settings.double_click.as_str() {
            "drop" | "nothing" => settings.double_click,
            _ => "launch".to_string(),
        },
        live2d_root: settings.live2d_root,
        files_root: settings.files_root,
        disabled: settings.disabled,
        stay_on_desktop: settings.stay_on_desktop,
        start_on_boot: settings.start_on_boot,
        animated_ratio: settings.animated_ratio.clamp(0.0, 100.0),
        animation_mode: match settings.animation_mode.as_str() {
            "sync" | "gentle" | "static" => settings.animation_mode,
            _ => "wave".to_string(),
        },
        float_amplitude: settings.float_amplitude.clamp(0.0, 16.0),
        float_period: settings.float_period.clamp(2.0, 60.0),
        float_spread: settings.float_spread.clamp(0.0, 100.0),
        confirm_remove: settings.confirm_remove,
        viz_gain: settings.viz_gain.clamp(0.1, 4.0),
        viz_fps: settings.viz_fps.clamp(5.0, 60.0),
        sysmon_interval: settings.sysmon_interval.clamp(250.0, 10_000.0),
        // A shortcut that does not parse is not saved: the alternative is a
        // launcher that can never be opened again, and the typo would be invisible.
        palette_shortcut: match palette::parse_shortcut(settings.palette_shortcut.trim()) {
            Some(accelerator) => accelerator,
            None => stored.palette_shortcut.clone(),
        },
        hide_in_fullscreen: settings.hide_in_fullscreen,
        quiet_when_idle: settings.quiet_when_idle,
        // Ten hours is not a threshold, it is a typo; zero means never.
        idle_minutes: settings.idle_minutes.min(600),
        // The approvals are the backend's own bookkeeping, like icon_pipeline: a
        // settings form that does not show them must not be able to clear them.
        plugin_trust: stored.plugin_trust.clone(),
        icon_pipeline: stored_pipeline.max(settings.icon_pipeline),
        // ...and so is the first-run answer (ADR 0004): the settings window has no
        // row for it, so a save from there must not be able to put the guard back
        // and block a user who has already answered. `floaty_confirm_root` is the
        // only thing that sets it — and note that `stored` is the *file*, so a probe
        // running with FLOATY_ROOT_CONFIRMED set does not write its override here.
        root_confirmed: stored.root_confirmed,
    };
    // Start on boot is a registry entry, not a preference: write it now and, if
    // that fails, put the old value back rather than save a checkbox that claims
    // something Windows does not agree with.
    if s.start_on_boot != stored.start_on_boot && !set_start_on_boot(&app, s.start_on_boot) {
        s.start_on_boot = stored.start_on_boot;
    }
    write_settings(&app, &s);
    if previous_root != s.files_root {
        purge_root_records(&app, &previous_root, &s.files_root);
    }
    if !s.files_root.trim().is_empty() {
        allow_asset_path(&app, &s.files_root);
    }
    if !s.live2d_root.trim().is_empty() {
        allow_asset_path(&app, &s.live2d_root);
    }
    #[cfg(windows)]
    {
        // The window procedure reads this flag on every minimize/show-desktop
        // message; a save is what makes the toggle take effect without a restart.
        desktop_pin::set_stay_on_desktop_global(s.stay_on_desktop);
        for (label, win) in app.webview_windows() {
            if let Ok(hwnd) = win.hwnd() {
                let hwnd = hwnd.0 as isize;
                if is_top_overlay_label(&label) {
                    // the always-on-top layer is never parented into the desktop
                    desktop_pin::apply_overlay_desktop_pin(&label, hwnd, false);
                } else if is_desktop_overlay_label(&label) {
                    desktop_pin::apply_overlay_desktop_pin(&label, hwnd, s.stay_on_desktop);
                } else if is_desktop_layer_label(&label) {
                    desktop_pin::apply_desktop_pin(&label, hwnd, s.stay_on_desktop);
                } else if desktop_pin::restore_ordinary_window(hwnd) {
                    // settings, the manager: ordinary windows. A settings save is
                    // frequent enough to repair one that an earlier pass pinned,
                    // and saying so is what makes the repair visible in the log.
                    log_line(
                        &app,
                        &format!(
                            "settings: {label} handed back to the shell as an ordinary window"
                        ),
                    );
                }
            }
        }
    }
    app.emit("floaty-settings-changed", &s).ok();
    log_line(&app, "settings updated");

    // A new root is a new folder to follow: mirror it once and point the watcher
    // at it (the sync does that itself, including stopping the watcher when the
    // root was cleared), so the desktop shows the folder straight away rather
    // than at the next launch.
    if stored.files_root.trim() != s.files_root.trim() {
        let _ = floaty_sync_files(None, app.clone());
    }
    // A new launcher key is registered now, so the settings row can say whether
    // the key it just took is really held.
    if stored.palette_shortcut.trim() != s.palette_shortcut.trim() {
        apply_palette_shortcut(&app);
    }
    // The presence watcher reads its rules from memory, not from this file every two
    // seconds, so a change here is what it notices.
    set_presence_rules(rules_from(&s));
    s
}
