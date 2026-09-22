//! `sleep_resume` — moved out of lib.rs verbatim.
//!
//! Visibility is `pub(crate)` because the rest of the crate calls in across the module
//! boundary now; nothing here changed shape on the way out.

use crate::*;
use tauri::{AppHandle, Emitter, Manager};

// ---------- sleep / resume ----------

/// Windows tells every top-level window when the machine goes down and comes
/// back. Nothing else re-creates what a sleep broke.
pub(crate) const WM_POWERBROADCAST: u32 = 0x0218;
/// Sent to every top-level window when a display is added, removed or re-scaled.
/// Spelled out here for the same reason as the line above: this module names its own
/// messages, and the crate's copy lives behind a module this file does not import.
pub const WM_DISPLAYCHANGE: u32 = 0x007E;
pub(crate) const PBT_APMRESUMESUSPEND: usize = 0x0007;
pub(crate) const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;
pub(crate) const PBT_APMRESUMECRITICAL: usize = 0x0006;
pub(crate) const PBT_POWERSETTINGCHANGE: usize = 0x8013;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Power::{
    RegisterPowerSettingNotification, POWERBROADCAST_SETTING,
};
use windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;
use windows::Win32::UI::WindowsAndMessaging::{DEVICE_NOTIFY_CALLBACK, DEVICE_NOTIFY_WINDOW_HANDLE};

/// Windows delivers power-setting changes through a pool thread when registered
/// in callback mode, which is the whole point: the callback still lands while the
/// app's own pumping thread is blocked, and it can therefore both *measure* the
/// wake-up and do the recovery from a thread that is not stuck.
/// 2 = no report yet, 1 = display on, 0 = display off
pub(crate) static DISPLAY_STATE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(2);
/// when the display-on callback last ran, in ms since the process started
pub(crate) static DISPLAY_ON_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[repr(C)]
pub(crate) struct DeviceNotifySubscribeParameters {
    pub(crate) callback: Option<
        unsafe extern "system" fn(
            context: *const core::ffi::c_void,
            kind: u32,
            setting: *const core::ffi::c_void,
        ) -> u32,
    >,
    pub(crate) context: *const core::ffi::c_void,
}

unsafe extern "system" fn display_setting_changed(
    _context: *const core::ffi::c_void,
    kind: u32,
    setting: *const core::ffi::c_void,
) -> u32 {
    if kind as usize == PBT_POWERSETTINGCHANGE && !setting.is_null() {
        let s = &*(setting as *const POWERBROADCAST_SETTING);
        if s.PowerSetting != GUID_CONSOLE_DISPLAY_STATE && s.DataLength >= 1 {
            if let Some(app) = shared_app() {
                log_line(
                    &app,
                    &format!(
                        "power: other setting changed ({:?} = {})",
                        s.PowerSetting,
                        s.Data[0]
                    ),
                );
            }
        }
        if s.PowerSetting == GUID_CONSOLE_DISPLAY_STATE && s.DataLength >= 1 {
            let state = s.Data[0] as u64;
            let previous = DISPLAY_STATE.swap(state, std::sync::atomic::Ordering::SeqCst);
            if previous != state {
                if let Some(app) = shared_app() {
                    // A real signal for widgets and plugins: a clock that redraws, a
                    // poll that costs a powershell, a pet that animates — all have a
                    // reason to stop while nobody is looking at the screen.
                    let _ = app.emit(
                        "floaty-display-changed",
                        serde_json::json!({ "display": if state == 1 { "on" } else { "off" } }),
                    );
                    log_line(
                        &app,
                        &format!(
                            "power: display {} (was {})",
                            if state == 1 { "on" } else { "off" },
                            if previous == 1 {
                                "on"
                            } else if previous == 0 {
                                "off"
                            } else {
                                "unknown"
                            }
                        ),
                    );
                }
            }
            if state == 1 {
                DISPLAY_ON_MS.store(elapsed_ms(), std::sync::atomic::Ordering::SeqCst);
                // Recover here, on this pool thread, instead of waiting for a
                // window message that a blocked main thread cannot process.
                recover_windows("display back on (power callback)");
            }
        }
    }
    0
}

/// Register the display-state notification without binding it to a window, so no
/// window rebuild can make the app deaf to a wake-up.
pub fn watch_display_by_callback(app: &AppHandle) {
    static REGISTERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if REGISTERED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let params = Box::leak(Box::new(DeviceNotifySubscribeParameters {
        callback: Some(display_setting_changed),
        context: std::ptr::null(),
    }));
    // Registered in callback mode, so the handle is never needed again: the
    // callback outlives every window and every window rebuild.
    match unsafe {
        RegisterPowerSettingNotification(
            HANDLE(params as *mut _ as *mut core::ffi::c_void),
            &GUID_CONSOLE_DISPLAY_STATE,
            DEVICE_NOTIFY_CALLBACK,
        )
    } {
        Ok(_) => log_line(app, "power: display state is watched by callback (survives window rebuilds)"),
        Err(e) => log_line(app, &format!("power: could not register the display callback: {e}")),
    }
}

/// Does the pumping thread still answer? A window that stops answering for
/// seconds is what Windows paints as "(Not Responding)", and this records how long
/// it lasted. One no-op message every 2s.
pub(crate) fn start_pump_watchdog() {
    std::thread::spawn(|| {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SLOW_SINCE: AtomicU64 = AtomicU64::new(0);
        loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let Some(app) = shared_app() else { continue };
            let Some(w) = app.get_webview_window("desktop-overlay") else { continue };
            let Ok(hwnd) = w.hwnd() else { continue };
            let t0 = std::time::Instant::now();
            let mut res = 0usize;
            // No SMTO_ABORTIFHUNG: that flag returns immediately for a window
            // Windows already considers hung, which is exactly the case being
            // measured. Without it a blocked thread makes this time out, and the
            // timeout is the measurement.
            let answered = unsafe {
                windows::Win32::UI::WindowsAndMessaging::SendMessageTimeoutW(
                    windows::Win32::Foundation::HWND(hwnd.0),
                    0, // WM_NULL: a no-op, answered as soon as the queue is pumped
                    windows::Win32::Foundation::WPARAM(0),
                    windows::Win32::Foundation::LPARAM(0),
                    windows::Win32::UI::WindowsAndMessaging::SEND_MESSAGE_TIMEOUT_FLAGS(0),
                    1500,
                    Some(&mut res),
                )
            };
            let ms = t0.elapsed().as_millis() as u64;
            let _ = answered;
            let since = SLOW_SINCE.load(Ordering::SeqCst);
            if (ms > 400 || answered.0 == 0) && since == 0 {
                SLOW_SINCE.store(elapsed_ms(), Ordering::SeqCst);
                log_line(
                    &app,
                    &format!("pump: the overlay did not answer for {ms}ms — the main thread is blocked"),
                );
            } else if ms <= 400 && answered.0 != 0 && since != 0 {
                SLOW_SINCE.store(0, Ordering::SeqCst);
                log_line(
                    &app,
                    &format!(
                        "pump: answering again after {}ms of being blocked",
                        elapsed_ms().saturating_sub(since)
                    ),
                );
            }
        }
    });
}

pub(crate) static SHARED_APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();
pub(crate) static RESUME_CLOCK: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
pub(crate) static LAST_RESUME_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The app handle, for the window procedures that only get an HWND.
pub(crate) fn shared_app() -> Option<AppHandle> {
    SHARED_APP.get().cloned()
}

/// Put the pinned layer back in front of everything else.
#[cfg(windows)]
pub(crate) fn raise_top_layer(app: &AppHandle) {
    // one layer per screen: a pinned widget on the second screen needs its own window
    // in front, not the first screen's
    for (label, w) in app.webview_windows() {
        if label != TOP_LAYER && !label.starts_with(&format!("{TOP_LAYER}-")) {
            continue;
        }
        if let Ok(hwnd) = w.hwnd() {
            desktop_pin::raise_above_everything(hwnd.0 as isize);
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn raise_top_layer(_app: &AppHandle) {}

/// Keep the pinned layer above other *topmost* windows.
///
/// Being topmost is not enough on its own: Windows keeps every topmost window in
/// one band and orders that band by whoever raised last, so a remote-desktop
/// window that puts itself on top covers the pinned widgets (measured: RustDesk's
/// session window directly above `top-overlay`). Two assertions a second is what
/// "stay on top" costs — a `SetWindowPos` that does not move, resize or activate
/// anything — and the work only happens while the layer exists, which is only
/// while some widget is pinned.
pub(crate) fn start_top_layer_watchdog() {
    const INTERVAL_MS: u64 = 500;
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(INTERVAL_MS));
        let Some(app) = shared_app() else { continue };
        raise_top_layer(&app);
    });
}

/// Hands the display-state notification to the first window that asks: Modern
/// Standby never sends a resume message, only this setting change, so without it
/// nothing knows the screen came back.
pub fn watch_display_state(hwnd: isize) {
    use std::sync::atomic::{AtomicIsize, Ordering};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::IsWindow;

    // The notification is bound to the window that registered it, so remember
    // which one that was: when that window is replaced (the recovery re-creates
    // a wedged overlay), the next window to ask takes over instead of leaving
    // the app deaf to the display coming back.
    static WATCHED: AtomicIsize = AtomicIsize::new(0);
    let watched = WATCHED.load(Ordering::SeqCst);
    if watched != 0
        && unsafe { IsWindow(Some(HWND(watched as *mut core::ffi::c_void))) }.as_bool()
    {
        return;
    }
    unsafe {
        let _ = RegisterPowerSettingNotification(
            HANDLE(hwnd as *mut core::ffi::c_void),
            &GUID_CONSOLE_DISPLAY_STATE,
            DEVICE_NOTIFY_WINDOW_HANDLE,
        );
    }
    WATCHED.store(hwnd, Ordering::SeqCst);
}

/// What to put back after the machine goes down and comes back.
///
/// Windows tells a window about a wake-up in two different ways: a classic sleep
/// sends `WM_POWERBROADCAST` with a resume code, while Modern Standby (what this
/// laptop does) sends *no resume message at all* — only a power-setting change
/// for `GUID_CONSOLE_DISPLAY_STATE`. Either way the webview is told nothing, and
/// after a standby Chromium can keep believing the window is occluded: the page
/// stays hidden, its timers are throttled and rAF stops, which is the "widgets
/// stuck for a long time" symptom.
///
/// So: refit the overlay to the monitor, put the desktop-level state back, nudge
/// the window bounds (a bounds change is what makes Chromium re-evaluate
/// occlusion), reload the windows and revive the samplers. Every top-level
/// window receives these messages, hence the debounce.
pub(crate) fn recover_windows(reason: &str) {
    use std::sync::atomic::Ordering;
    let now_ms = elapsed_ms();
    let last = LAST_RESUME_MS.load(Ordering::SeqCst);
    if now_ms.saturating_sub(last) < 15_000 {
        return;
    }
    LAST_RESUME_MS.store(now_ms, Ordering::SeqCst);

    let Some(app) = shared_app() else { return };
    let s = load_settings(&app);
    let windows: Vec<(String, tauri::WebviewWindow)> = app
        .webview_windows()
        .into_iter()
        // The hidden manager window renders nothing and is never rebuilt, so it
        // is not watched (it is still the window that holds the display-state
        // notification, which is what matters about it).
        .filter(|(label, _)| is_desktop_layer_label(label) || label == "settings")
        .collect();
    let started = std::time::Instant::now();
    let seen = DISPLAY_ON_MS.load(std::sync::atomic::Ordering::SeqCst);
    let lag = if seen > 0 { now_ms.saturating_sub(seen) } else { 0 };
    log_line(
        &app,
        &format!(
            "power: {reason} — checking {} windows (display reported {}ms ago)",
            windows.len(),
            lag
        ),
    );

    // Each phase reports its own elapsed time: the user sees "a long time to
    // recover", so the log has to say which step that time went into.
    fit_overlay_to_monitor(&app);
    log_line(
        &app,
        &format!("power: overlay refit (+{}ms)", started.elapsed().as_millis()),
    );

    let mut suspects: Vec<String> = Vec::new();
    for (label, w) in &windows {
        if let Ok(hwnd) = w.hwnd() {
            if label == "desktop-overlay" {
                desktop_pin::apply_overlay_desktop_pin(label, hwnd.0 as isize, s.stay_on_desktop);
            } else if label == "top-overlay" {
                desktop_pin::apply_overlay_desktop_pin(label, hwnd.0 as isize, false);
                // a wake-up can leave it buried under other topmost windows
                raise_top_layer(&app);
            } else if is_desktop_layer_label(label) {
                desktop_pin::apply_desktop_pin(hwnd.0 as isize, s.stay_on_desktop);
            } else if desktop_pin::restore_ordinary_window(hwnd.0 as isize) {
                // The settings window is a normal window and has to stay one:
                // this pass used to desktop-pin *every* label it did not
                // recognise, which took the settings window's taskbar button
                // away and gave it the desktop shell as its owner — after which
                // minimizing it lost it and dragging it slid about behind the
                // other windows. Give it back, and say so.
                log_line(
                    &app,
                    &format!("power: {label} handed back to the shell as an ordinary window"),
                );
            }
            // Raw Win32, on purpose: this is what wakes the compositor and makes
            // Chromium re-evaluate occlusion. Going through the webview's own API
            // queues behind the very renderer we are trying to wake — measured: a
            // reload was accepted in 0.4s and executed 29s later. Only the
            // desktop layer needs it: an ordinary window is not composited
            // against the desktop, and a size change mid-drag would fight the
            // user moving it.
            if is_desktop_layer_label(label) {
                nudge_bounds(hwnd.0 as isize);
            }
        }
        // Decide per window and only touch the ones that look broken: a page that
        // reported in shortly before the wake-up and said it was visible survived
        // the standby and must be left alone (reloading it would cost a remount
        // and then look like a failure in its own right).
        let (beat_ms, visibility) = last_beat(label);
        let age_s = if beat_ms == 0 {
            f64::INFINITY
        } else {
            now_ms.saturating_sub(beat_ms) as f64 / 1000.0
        };
        if beat_ms > 0 && age_s <= 20.0 && visibility != "hidden" {
            log_line(
                &app,
                &format!("power: {label} is alive (reported in {age_s:.0}s ago, {visibility})"),
            );
            continue;
        }
        log_line(
            &app,
            &format!("power: {label} looks stuck (last heard {age_s:.0}s ago, {visibility}) — reloading it"),
        );
        suspects.push(label.clone());
    }

    // a PDH query or a loopback stream can die in the sleep; revive them if a
    // widget is still watching (they are ref-counted, so this never double-counts)
    sysmon::ensure_running(&app, Some(s.sysmon_interval as u32));
    audio::ensure_running(&app, Some(s.viz_fps as u32));

    // The pointed folder had a sleep too: a change can be lost while the machine
    // is away, a drive can come back, and the watch can be sitting on a
    // notification queue that never fired. Re-mirror it and make sure it is
    // still being followed (`retarget` no-ops when the watcher is healthy).
    if s.files_root.trim().is_empty() {
        fs_watch::retarget(None, app.clone());
    } else {
        fs_watch::retarget(Some(s.files_root.clone()), app.clone());
        let _ = floaty_sync_files(None, app.clone());
        log_line(
            &app,
            &format!(
                "power: mirror re-checked (+{}ms)",
                started.elapsed().as_millis()
            ),
        );
    }

    log_line(
        &app,
        &format!(
            "power: pinned + repainted {} windows (+{}ms total)",
            windows.len(),
            started.elapsed().as_millis()
        ),
    );
    if suspects.is_empty() {
        log_line(&app, "power: nothing needed rebuilding");
        return;
    }

    let app2 = app.clone();
    std::thread::spawn(move || {
        // Reload first: that alone rebuilds every widget's timers and re-claims
        // the samplers. Only if the page still cannot say it is back (and
        // visible) does the window get built again — the heavy step, and the only
        // one that cannot be queued behind a wedged renderer.
        std::thread::sleep(std::time::Duration::from_millis(600));
        for label in &suspects {
            if let Some(w) = app2.get_webview_window(label) {
                match w.reload() {
                    Ok(()) => log_line(&app2, &format!("power: reloaded {label}")),
                    Err(e) => log_line(&app2, &format!("power: could not reload {label}: {e}")),
                }
            }
        }
        // A page that mounts a whole desktop takes a while (tens of seconds in a
        // dev build), so give it room before calling it dead.
        std::thread::sleep(std::time::Duration::from_secs(30));
        for label in &suspects {
            let (beat_ms, visibility) = last_beat(label);
            if beat_ms > now_ms && visibility != "hidden" {
                log_line(&app2, &format!("power: {label} came back after the reload ({visibility})"));
                continue;
            }
            if beat_ms > now_ms {
                log_line(
                    &app2,
                    &format!("power: {label} came back but still believes it is hidden — re-creating it"),
                );
            } else {
                log_line(&app2, &format!("power: {label} never reported in — re-creating it"));
            }
            recreate_window(&app2, label);
        }
    });
}

/// How many ms this process has been running, for the heartbeat bookkeeping.
pub(crate) fn elapsed_ms() -> u64 {
    RESUME_CLOCK.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

/// One-pixel resize and back, straight to the window: no webview API involved.
pub(crate) fn nudge_bounds(hwnd: isize) {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowRect, SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
        };
        let mut r = RECT::default();
        if unsafe { GetWindowRect(windows::Win32::Foundation::HWND(hwnd as *mut _), &mut r) }.is_ok() {
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            if w < 2 || h < 2 {
                return;
            }
            unsafe {
                let hwnd = windows::Win32::Foundation::HWND(hwnd as *mut _);
                let _ = SetWindowPos(hwnd, None, 0, 0, w + 1, h + 1, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
                let _ = SetWindowPos(hwnd, None, 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
        // a resize alone does not clear a stale surface: ask for a full repaint
        desktop_pin::force_repaint(hwnd);
    }
    #[cfg(not(windows))]
    let _ = hwnd;
}

/// Last heartbeat from a window, in ms since the process started (0 = never).
/// When the window last reported in, and what it said about its own visibility.
pub(crate) fn last_beat(label: &str) -> (u64, String) {
    heartbeats()
        .lock()
        .ok()
        .and_then(|m| m.get(label).cloned())
        .unwrap_or((0, String::new()))
}

/// Last report from each window: when it came in, and what the page thought its
/// own visibility was.
pub(crate) static HEARTBEATS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, (u64, String)>>,
> = std::sync::OnceLock::new();

pub(crate) fn heartbeats() -> &'static std::sync::Mutex<std::collections::HashMap<String, (u64, String)>> {
    HEARTBEATS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// The windows report in every few seconds so the wake-up recovery can tell a
/// live page from one that is wedged.
/// Per-window record of what we did about a stale "hidden" verdict.
pub(crate) static HEARTBEAT_REPAIRS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, (u64, u32)>>,
> = std::sync::OnceLock::new();

pub(crate) fn heartbeat_repairs() -> &'static std::sync::Mutex<std::collections::HashMap<String, (u64, u32)>> {
    HEARTBEAT_REPAIRS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[tauri::command]
pub(crate) fn floaty_heartbeat(label: String, visibility: Option<String>, app: AppHandle) {
    let now = elapsed_ms();
    let visibility = visibility.unwrap_or_else(|| "unknown".to_string());
    let mut first = false;
    let mut previous_visibility = String::new();
    if let Ok(mut m) = heartbeats().lock() {
        match m.get(&label) {
            None => first = true,
            Some((_, v)) => previous_visibility = v.clone(),
        }
        m.insert(label.clone(), (now, visibility.clone()));
    }
    if !first && previous_visibility != visibility {
        let display = DISPLAY_STATE.load(std::sync::atomic::Ordering::SeqCst);
        let display = if display == 1 {
            "on"
        } else if display == 0 {
            "off"
        } else {
            "unknown"
        };
        log_line(
            &app,
            &format!(
                "heartbeat: {label} went {previous_visibility} -> {visibility} (display {display})"
            ),
        );
    }
    if first {
        log_line(
            &app,
            &format!("heartbeat: {label} is reporting in ({visibility})"),
        );
        // The window is definitely finished being created by now: arm the resume
        // handling on every window and re-home the display-state notification if
        // the window that held it is gone. The flag matters: the desktop-layer
        // policy this arms is only for the windows that are the desktop —
        // arming it on the settings window is what made its minimize button do
        // nothing and stripped its taskbar button on the first drag.
        for (label, w) in app.webview_windows() {
            if let Ok(hwnd) = w.hwnd() {
                desktop_pin::install_subclass(hwnd.0 as isize, is_desktop_layer_label(&label));
                watch_display_state(hwnd.0 as isize);
            }
        }
    }

    // A page that believes it is hidden while the display is on is Chromium's
    // stale occlusion verdict, and a hidden page has its timers throttled — this
    // is the "widgets are stuck" state. Nudge the window so it re-checks; if it
    // still insists half a minute later, rebuild the page. Checked on every beat,
    // not just the first one.
    if visibility == "hidden" && DISPLAY_STATE.load(std::sync::atomic::Ordering::SeqCst) == 1 {
        // Only act when the counter actually advances: every hidden beat inside
        // the same 30s window used to re-run the repair and re-log it.
        let (attempts, acted) = match heartbeat_repairs().lock() {
            Ok(mut m) => {
                let e = m.entry(label.clone()).or_insert((0, 0));
                if now.saturating_sub(e.0) > 30_000 {
                    e.0 = now;
                    e.1 += 1;
                    (e.1, true)
                } else {
                    (e.1, false)
                }
            }
            Err(_) => (0, false),
        };
        if !acted {
            return;
        }
        match attempts {
            1 => {
                log_line(
                    &app,
                    &format!("power: {label} says hidden while the display is on — forcing the webview visible again"),
                );
                let (app2, label2) = (app.clone(), label.clone());
                std::thread::spawn(move || {
                    if let Some(w) = app2.get_webview_window(&label2) {
                        // Neither a 1px nudge nor hiding and showing the *window*
                        // clears this: the page's verdict comes from the WebView2
                        // controller's own visibility flag. Set it directly — this
                        // is the API the stale state lives in.
                        let _ = w.with_webview(|webview| {
                            let controller = webview.controller();
                            unsafe {
                                let _ = controller.SetIsVisible(true);
                            }
                        });
                        if let Ok(hwnd) = w.hwnd() {
                            nudge_bounds(hwnd.0 as isize);
                        }
                    }
                });
            }
            2 => {
                log_line(
                    &app,
                    &format!("power: {label} is still hidden after being forced visible — rebuilding the page"),
                );
                let (app2, label2) = (app.clone(), label.clone());
                std::thread::spawn(move || {
                    if let Some(w) = app2.get_webview_window(&label2) {
                        let _ = w.reload();
                    }
                });
            }
            _ => {}
        }
    } else if visibility != "hidden" {
        if let Ok(mut m) = heartbeat_repairs().lock() {
            m.remove(&label);
        }
    }
}

/// Replace a wedged window: nothing the page or the webview API does can be
/// trusted by this point, so close it for real and build it again.
pub(crate) fn recreate_window(app: &AppHandle, label: &str) {
    if is_desktop_layer_label(label) {
        if let Some(w) = app.get_webview_window(label) {
            let _ = w.destroy();
        }
        // Tauri's registry keeps a destroyed window for as long as the webview
        // takes to come down, and `spawn_overlay_window` bails out while it is
        // still listed — which once left the desktop with no widgets at all.
        // Wait for the registry to clear (up to 15s) and then build it again.
        for attempt in 1..=60 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            if app.get_webview_window(label).is_some() {
                continue;
            }
            // "top-overlay" and "top-overlay-2" are both the top layer
            let again = if label.starts_with(TOP_LAYER) {
                // only if something is still pinned: nothing pinned means the
                // layer is meant to be gone
                sync_top_overlay(app);
                Ok(())
            } else {
                spawn_overlay_windows(app).map(|_| ())
            };
            match again {
                Ok(()) => {
                    let waited = attempt as f64 * 0.25;
                    log_line(app, &format!("power: {label} re-created after {waited:.1}s"));
                    return;
                }
                Err(e) => log_line(app, &format!("power: {label} re-create failed: {e}")),
            }
        }
        log_line(
            app,
            &format!("power: the old {label} window never came down — leaving it alone"),
        );
        return;
    }
    // one widget per window: close it and show it again from its record
    let Some(rec) = record_for_window(app, label) else { return };
    close_widget_async(app, &rec.id);
    show_widget(app, &rec);
}

/// The record behind a per-widget window label, if that window has one.
pub(crate) fn record_for_window(app: &AppHandle, label: &str) -> Option<WidgetRecord> {
    let state = app.state::<AppState>();
    let guard = state.0.lock().ok()?;
    guard
        .widgets
        .values()
        .find(|r| widget_label(&r.id) == label)
        .cloned()
}

/// Whether this widget asked to sit above other windows.
///
/// Everything else floats in the desktop layer, and that layer is *one* window in
/// the overlay mode: a window-level "on top" set on it lifts every widget at once
/// (which is what "Pin on top" used to do). Widgets that want to be above other
/// applications are drawn in a second overlay that is always on top instead — see
/// `spawn_top_overlay` — and only that layer carries the flag.
pub(crate) fn wants_on_top(rec: &WidgetRecord) -> bool {
    rec.data.get("on_top").and_then(|v| v.as_bool()).unwrap_or(false)
}

/// The desktop layer's window, and the layer a pinned widget is drawn in.
pub(crate) const DESKTOP_LAYER: &str = "desktop-overlay";
pub(crate) const TOP_LAYER: &str = "top-overlay";

/// Whether the overlay mode is running (as opposed to one window per widget).
pub(crate) fn is_overlay_running(app: &AppHandle) -> bool {
    app.webview_windows()
        .keys()
        .any(|label| label == DESKTOP_LAYER || label.starts_with(&format!("{DESKTOP_LAYER}-")))
}
