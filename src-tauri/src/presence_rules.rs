//! `presence_rules` — getting out of the way of a fullscreen app, and going quiet when idle.
//!
//! Moved out of `settings_sync.rs` verbatim.

use crate::*;
use tauri::{AppHandle, Emitter, Manager};

// ---------------------------------------------------------------------------------------
// Presence: getting out of the way of a fullscreen app, and going quiet when idle.
// ---------------------------------------------------------------------------------------

/// The state as last applied, one entry per desktop-layer window. The watcher decides every
/// couple of seconds; only a *change* does anything, so an already-quiet desktop is not
/// re-flattened every poll — and a change on one screen does not touch the others.
pub(crate) static PRESENCE: std::sync::Mutex<Option<Vec<(String, presence::State)>>> =
    std::sync::Mutex::new(None);

/// The rules in force, kept here so the watcher does not read the settings file 30 times
/// a minute. Written at startup and whenever the settings are saved.
pub(crate) static PRESENCE_RULES: std::sync::Mutex<Option<presence::Rules>> =
    std::sync::Mutex::new(None);

pub(crate) fn set_presence_rules(rules: presence::Rules) {
    if let Ok(mut guard) = PRESENCE_RULES.lock() {
        *guard = Some(rules);
    }
}

pub(crate) fn presence_rules() -> presence::Rules {
    PRESENCE_RULES
        .lock()
        .ok()
        .and_then(|guard| *guard)
        .unwrap_or_default()
}

pub(crate) fn rules_from(settings: &FloatSettings) -> presence::Rules {
    presence::Rules {
        hide_for_fullscreen: settings.hide_in_fullscreen,
        quiet_when_idle: settings.quiet_when_idle,
        idle_after: std::time::Duration::from_secs(settings.idle_minutes.min(600) as u64 * 60),
    }
}

/// Every window that draws the desktop — one per screen, plus the per-screen top layers.
pub(crate) fn desktop_layer_windows(app: &AppHandle) -> Vec<tauri::WebviewWindow> {
    app.webview_windows()
        .into_iter()
        .filter(|(label, _)| is_desktop_layer_label(label))
        .map(|(_, window)| window)
        .collect()
}

/// One desktop layer's answer, so a reader can see *which* screen went away. The state is
/// per screen because the windows are; a single `hidden` cannot describe this desktop.
#[derive(serde::Serialize)]
pub(crate) struct PresenceScreenReport {
    /// The desktop-layer window's label, which is also the screen it draws.
    pub(crate) label: String,
    /// The monitor's own name, when the label maps to one.
    pub(crate) screen: Option<String>,
    pub(crate) hidden: bool,
    pub(crate) quiet: bool,
    pub(crate) reason: Option<String>,
}

#[derive(serde::Serialize)]
pub(crate) struct PresenceReport {
    /// Hidden on *any* screen: what a one-line readout means by "the desktop is out of the
    /// way". `screens` says which.
    pub(crate) hidden: bool,
    /// Quiet on *every* screen — the shared audio capture stops only then.
    pub(crate) quiet: bool,
    pub(crate) reason: Option<String>,
    pub(crate) idle_ms: u64,
    /// The window in front, as the decision saw it — the field that explains a state
    /// nobody expected, so the diagnostics tab and a probe can both read it.
    pub(crate) foreground: Option<String>,
    pub(crate) foreground_shell: bool,
    pub(crate) foreground_ours: bool,
    /// One entry per desktop layer — the field that makes a per-screen rule legible.
    pub(crate) screens: Vec<PresenceScreenReport>,
    pub(crate) hide_in_fullscreen: bool,
    pub(crate) quiet_when_idle: bool,
    pub(crate) idle_minutes: u32,
}

pub(crate) fn presence_observations(app: &AppHandle) -> presence::Observations {
    let own: Vec<isize> = desktop_layer_windows(app)
        .iter()
        .filter_map(|w| w.hwnd().ok().map(|h| h.0 as isize))
        .collect();
    presence::observe(&own)
}

/// The screen a desktop-layer window draws, as the presence rules see it.
///
/// A per-screen overlay says so in its label (`desktop-overlay-2` is the second screen). A
/// `widget-*` window in per-widget mode does not, so it is placed by where it is. A window
/// whose screen cannot be worked out gets `None`, and the rule leaves it alone: hiding the
/// desktop on a guess is worse than leaving it up.
pub(crate) fn presence_screen_of(
    list: &[screens::Screen],
    window: &tauri::WebviewWindow,
) -> Option<presence::Rect> {
    let rect = |s: &screens::Screen| {
        presence::Rect::new(s.physical.x, s.physical.y, s.physical.w, s.physical.h)
    };
    if let Some(index) = overlay_index(window.label()) {
        return list.get(index).map(rect);
    }
    let at = window.outer_position().ok()?;
    let (x, y) = (at.x as f64, at.y as f64);
    list.iter()
        .find(|s| {
            x >= s.physical.x
                && y >= s.physical.y
                && x < s.physical.x + s.physical.w
                && y < s.physical.y + s.physical.h
        })
        .map(rect)
}

/// The decision for every desktop-layer window — one per screen — and the machine's answer.
///
/// Per screen, because the windows are: a fullscreen app in front of one screen takes that
/// screen's desktop away and leaves the others alone.
pub(crate) fn presence_states(
    app: &AppHandle,
    observed: &presence::Observations,
) -> (Vec<(String, presence::State)>, presence::State) {
    let rules = presence_rules();
    let list = screens(app);
    let states: Vec<(String, presence::State)> = desktop_layer_windows(app)
        .into_iter()
        .map(|window| {
            let screen = presence_screen_of(&list, &window);
            let state = presence::decide_for(
                observed.foreground.as_ref(),
                screen.as_ref(),
                &rules,
                observed.idle_ms,
            );
            (window.label().to_string(), state)
        })
        .collect();
    let summary = presence::summarise(&states.iter().map(|(_, s)| *s).collect::<Vec<_>>());
    (states, summary)
}

/// Show a window that presence hid.
///
/// `window.show()` is `SW_SHOW`: it activates and raises, which steals the
/// foreground from the app the user is in and puts the bottom layer back above
/// ordinary windows. The desktop layer comes back with `SW_SHOWNOACTIVATE`;
/// everything except the top layer also returns to the bottom band.
fn show_presence_window(window: &tauri::WebviewWindow) {
    #[cfg(windows)]
    if is_desktop_layer_label(window.label()) {
        if let Ok(hwnd) = window.hwnd() {
            desktop_pin::show_without_activating(
                hwnd.0 as isize,
                !is_top_overlay_label(window.label()),
            );
            return;
        }
    }
    let _ = window.show();
}

/// Hide the same way the window was shown. Tauri's own `hide()` consults a
/// cached visible flag that a raw `ShowWindow` never updates.
fn hide_presence_window(window: &tauri::WebviewWindow) {
    #[cfg(windows)]
    if is_desktop_layer_label(window.label()) {
        if let Ok(hwnd) = window.hwnd() {
            desktop_pin::hide_window(hwnd.0 as isize);
            return;
        }
    }
    let _ = window.hide();
}

/// Put the desktop where the decision says it should be, and say so once.
pub(crate) fn apply_presence(
    app: &AppHandle,
    states: &[(String, presence::State)],
    summary: presence::State,
) {
    let previous = PRESENCE
        .lock()
        .ok()
        .and_then(|mut guard| guard.replace(states.to_vec()));
    if previous.as_deref() == Some(states) {
        return;
    }
    // Only the screens that changed are touched: a fullscreen app on one screen hides that
    // screen's overlay and leaves the other screens' windows exactly as they were.
    let changed: Vec<&(String, presence::State)> = states
        .iter()
        .filter(|(label, state)| {
            let before = previous
                .as_ref()
                .and_then(|prev| prev.iter().find(|(l, _)| l == label).map(|(_, s)| s));
            if before != Some(state) {
                return true;
            }
            // A recreated window keeps the label but starts visible; the cached
            // decision would otherwise say "already hidden" forever.
            app.get_webview_window(label)
                .and_then(|w| w.is_visible().ok())
                .is_some_and(|visible| visible != !state.hidden)
        })
        .collect();
    for (label, state) in changed {
        if let Some(window) = app.get_webview_window(label) {
            let visible = window.is_visible().unwrap_or(true);
            if state.hidden {
                if visible {
                    hide_presence_window(&window);
                }
            } else if !visible {
                show_presence_window(&window);
            }
            // To this window only, and with its own state: the desktop layers are separate
            // windows, and a page must not act on another screen's verdict. A broadcast
            // would let a hidden screen's visualizer start the audio again.
            let _ = app.emit_to(
                tauri::EventTarget::webview_window(label.as_str()),
                "floaty-presence",
                serde_json::json!({
                    "hidden": state.hidden,
                    "quiet": state.quiet,
                    "reason": state.reason,
                    "machine": { "hidden": summary.hidden, "quiet": summary.quiet },
                }),
            );
            match state.reason {
                Some(why) => log_line(
                    app,
                    &format!(
                        "presence: {label} {} ({why}) — desktop {}",
                        if state.hidden { "hidden" } else { "quiet" },
                        if state.hidden {
                            "off screen"
                        } else {
                            "on screen, still"
                        }
                    ),
                ),
                None => log_line(
                    app,
                    &format!("presence: {label} back to normal input and no fullscreen app"),
                ),
            }
        }
    }
    if summary.quiet {
        // The loopback capture is the one thing that costs while nothing is happening;
        // the visualizer asks for it again when the machine wakes up (it listens for
        // the same event), so this is a pause rather than a switch that stays off. It
        // stops only when *every* screen is quiet — one screen left is enough to want it.
        audio::stop();
    }
}

/// Watch the machine, from a thread of its own: the poll is two Win32 calls and a
/// comparison, so it is cheap enough to keep running, and cheap enough to leave alone
/// when it decides nothing has changed.
pub(crate) fn start_presence_watch(app: AppHandle) {
    std::thread::spawn(move || loop {
        let observed = presence_observations(&app);
        let (states, summary) = presence_states(&app, &observed);
        apply_presence(&app, &states, summary);
        std::thread::sleep(std::time::Duration::from_millis(2000));
    });
}

/// The presence state, for the diagnostics tab and for probes.
#[tauri::command]
pub(crate) fn floaty_presence(app: AppHandle) -> PresenceReport {
    let settings = load_settings(&app);
    let observed = presence_observations(&app);
    let (states, summary) = presence_states(&app, &observed);
    let list = screens(&app);
    let fg = observed.foreground.as_ref();
    PresenceReport {
        hidden: states.iter().any(|(_, s)| s.hidden),
        quiet: summary.quiet,
        reason: states
            .iter()
            .find_map(|(_, s)| s.reason)
            .map(|r| r.to_string()),
        idle_ms: observed.idle_ms,
        foreground: fg.map(|f| {
            format!(
                "{}x{} at {},{}",
                f.rect.w.round(),
                f.rect.h.round(),
                f.rect.x.round(),
                f.rect.y.round()
            )
        }),
        foreground_shell: fg.is_some_and(|f| f.shell),
        foreground_ours: fg.is_some_and(|f| f.ours),
        screens: states
            .iter()
            .map(|(label, state)| PresenceScreenReport {
                label: label.clone(),
                screen: overlay_index(label)
                    .and_then(|index| list.get(index))
                    .map(|s| s.key.clone()),
                hidden: state.hidden,
                quiet: state.quiet,
                reason: state.reason.map(|r| r.to_string()),
            })
            .collect(),
        hide_in_fullscreen: settings.hide_in_fullscreen,
        quiet_when_idle: settings.quiet_when_idle,
        idle_minutes: settings.idle_minutes,
    }
}
