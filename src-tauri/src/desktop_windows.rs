//! `windows` — moved out of lib.rs verbatim.
//!
//! Visibility is `pub(crate)` because the rest of the crate calls in across the module
//! boundary now; nothing here changed shape on the way out.

use crate::*;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

// ---------- windows ----------

pub(crate) fn widget_label(id: &str) -> String {
    format!("widget-{id}")
}

/// Which windows belong to the desktop layer: the two overlays, and floaties
/// drawn as their own windows.
///
/// Those are the ones that lose their taskbar button on purpose — they *are*
/// the desktop. Every other window the app opens (the settings window, the
/// hidden manager) is an ordinary window: it keeps its decorations, its taskbar
/// button and its minimize behaviour, and a pass that walks every label has to
/// say so, or it turns the settings window into a taskbar-less tool window that
/// cannot be minimized back or moved properly.
pub(crate) fn is_desktop_layer_label(label: &str) -> bool {
    label == DESKTOP_LAYER
        || label == TOP_LAYER
        || label.starts_with(&format!("{DESKTOP_LAYER}-"))
        || label.starts_with(&format!("{TOP_LAYER}-"))
        || label.starts_with("widget-")
}

/// The screen an overlay window belongs to, read from its label: `desktop-overlay`
/// and `top-overlay` are the first screen's, `desktop-overlay-2` the second's.
pub(crate) fn overlay_index(label: &str) -> Option<usize> {
    let rest = label
        .strip_prefix(DESKTOP_LAYER)
        .or_else(|| label.strip_prefix(TOP_LAYER))?;
    if rest.is_empty() {
        return Some(0);
    }
    rest.strip_prefix('-')?.parse::<usize>().ok()
}

/// The label an overlay on screen `index` has. Screen 0 keeps the plain label, so a
/// one-screen desktop keeps exactly the windows it always had.
pub(crate) fn overlay_label(base: &str, index: usize) -> String {
    if index == 0 {
        base.to_string()
    } else {
        format!("{base}-{index}")
    }
}

/// Every screen, as floaty models it: physical px (where a window goes) and logical px
/// (where a record lives), with the scale that relates them.
pub(crate) fn screens(app: &AppHandle) -> Vec<screens::Screen> {
    let primary = app
        .primary_monitor()
        .ok()
        .flatten()
        .and_then(|m| m.name().map(|n| n.to_string()));
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let name = m.name().map(|n| n.to_string()).unwrap_or_default();
            let scale = m.scale_factor();
            screens::Screen {
                primary: Some(name.clone()) == primary,
                key: name.clone(),
                name,
                physical: screens::Rect::new(
                    m.position().x as f64,
                    m.position().y as f64,
                    m.size().width as f64,
                    m.size().height as f64,
                ),
                logical: screens::Rect::new(
                    (m.position().x as f64 / scale).round(),
                    (m.position().y as f64 / scale).round(),
                    (m.size().width as f64 / scale).round(),
                    (m.size().height as f64 / scale).round(),
                ),
                scale,
            }
        })
        .collect()
}

/// A page's place in the arrangement: the screen its window covers, and the desktop
/// as a whole. An overlay uses this to know which records are its own and where they
/// go: `local = virtual - screen.logical.origin`, in the window's own css px.
#[derive(serde::Serialize)]
pub(crate) struct OverlayArea {
    pub(crate) index: usize,
    pub(crate) screen: screens::Screen,
    pub(crate) desktop: screens::Rect,
}

#[tauri::command]
pub(crate) fn floaty_overlay_area(window: tauri::WebviewWindow, app: AppHandle) -> Option<OverlayArea> {
    let list = screens(&app);
    let index = overlay_index(window.label())?;
    Some(OverlayArea {
        index,
        screen: list.get(index)?.clone(),
        desktop: screens::union(&list),
    })
}

#[tauri::command]
pub(crate) fn floaty_screens(app: AppHandle) -> Vec<screens::Screen> {
    screens(&app)
}

/// Extensions the Windows shell launches directly. Anything else that is not a
/// directory is a *file*: it floats with a document icon and opens with its
/// default app instead of being spawned like an executable.
pub(crate) const LAUNCHABLE_EXTS: &[&str] = &[
    "exe", "lnk", "url", "bat", "cmd", "com", "scr", "msi", "appref-ms", "ps1", "psm1", "vbs",
    "vbe", "js", "jse", "wsf", "wsh", "hta", "cpl", "msc", "reg", "jar", "ahk",
];

pub(crate) fn is_launchable_target(path: &std::path::Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => LAUNCHABLE_EXTS.iter().any(|k| ext.eq_ignore_ascii_case(k)),
        None => false,
    }
}

/// Widget kind for a filesystem entry: folder, launchable app, or loose file.
pub(crate) fn kind_for_path(path: &std::path::Path, is_dir: bool) -> &'static str {
    if is_dir {
        "folder"
    } else if is_launchable_target(path) {
        "app"
    } else {
        "file"
    }
}

/// Widget kinds that describe a single launchable path (icon resolution, launch,
/// layout and folder grouping treat them the same). The manifest owns the list.
pub(crate) fn is_path_kind(kind: &str) -> bool {
    plugins::is_path_kind(kind)
}

/// Where the desktop is: the union of every screen, in the space records live in.
///
/// It used to be the primary monitor, which meant a widget on a second screen was laid
/// out outside the window and never seen. Now it is the whole arrangement — and with
/// one overlay window *per screen* (see `spawn_overlay_windows`) this is only what a
/// widget's `monitorArea()` reports, not where anything is drawn.
pub(crate) fn overlay_rect(app: &AppHandle) -> (f64, f64, f64, f64) {
    let list = screens(app);
    if list.is_empty() {
        return (0.0, 0.0, 1920.0, 1080.0);
    }
    log_mixed_scale(app, &list);
    let union = screens::union(&list);
    (union.x, union.y, union.w, union.h)
}

/// Say once, in the log, when the arrangement spans scale factors.
///
/// It used to be a warning that the other screen would be drawn at the wrong size;
/// with an overlay per screen each window is created *on* its own monitor, and
/// WebView2 re-rasterizes to that monitor's scale (measured: moving a window onto a
/// 150% screen takes its `devicePixelRatio` from 2 to 1.5). It stays in the log as
/// context for a bug report.
pub(crate) fn log_mixed_scale(app: &AppHandle, list: &[screens::Screen]) {
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if list.len() < 2 || SAID.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let first = list[0].scale;
    if list.iter().all(|s| (s.scale - first).abs() < 0.001) {
        return;
    }
    let scales: Vec<String> = list.iter().map(|s| format!("{:.2}", s.scale)).collect();
    log_line(
        app,
        &format!(
            "monitors: {} screens at different scale factors [{}] — each overlay is created on its own screen, so each renders at its own scale",
            list.len(),
            scales.join(", ")
        ),
    );
}

/// Every overlay, on its own screen: a resolution or DPI change while the machine was
/// asleep (a dock, a projector, another scaling) leaves the windows the wrong size and
/// the widgets outside them looking stuck.
pub(crate) fn fit_overlay_to_monitor(app: &AppHandle) {
    let list = screens(app);
    for (label, _) in app.webview_windows() {
        let Some(index) = overlay_index(&label) else {
            continue;
        };
        match list.get(index) {
            Some(screen) => {
                if let Some(win) = app.get_webview_window(&label) {
                    place_on_screen(app, &win, screen);
                }
            }
            None => close_extra_overlays(app),
        }
    }
}


/// One desktop-layer overlay per screen, plus a top-layer overlay on each screen that
/// has a pinned widget.
///
/// **This is what fixes a mixed-DPI desktop.** One window covering every screen has
/// one scale factor, so a widget on a 150% screen next to a 200% one was drawn at
/// 200% — 33% too big. A window created (or moved) onto a screen takes that screen's
/// scale: measured, moving the manager window onto the 150% screen took its
/// `devicePixelRatio` from 2 to 1.5.
///
/// With a single screen the labels and the geometry are exactly what this always
/// built, so nothing about one monitor changes.
pub(crate) fn spawn_overlay_windows(app: &AppHandle) -> tauri::Result<()> {
    for (index, screen) in screens(app).iter().enumerate() {
        let label = overlay_label(DESKTOP_LAYER, index);
        if app.get_webview_window(&label).is_none() {
            spawn_desktop_overlay(app, &label, screen)?;
        }
    }
    // the top layer exists only while something is pinned, per screen
    sync_top_overlay(app);
    Ok(())
}

/// One desktop overlay, on one screen: built with the screen's logical rect and then
/// put exactly where the platform says that screen is.
///
/// The second step matters: the builder only takes *logical* sizes and converts them
/// with the scale factor the window has at birth, which is the primary's — so a
/// window for the 150% screen would land at physical 1920 * 2 instead of 1920 * 1.5.
/// Setting the physical rectangle afterwards is what puts it on its screen (and
/// gives it that screen's scale).
pub(crate) fn spawn_desktop_overlay(
    app: &AppHandle,
    label: &str,
    screen: &screens::Screen,
) -> tauri::Result<()> {
    log_line(
        app,
        &format!(
            "spawn {label} on screen {} at {},{} size {}x{} @{}x",
            screen.name,
            screen.physical.x,
            screen.physical.y,
            screen.physical.w,
            screen.physical.h,
            screen.scale
        ),
    );
    let win = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html#/overlay".into()))
        .title("Floaty Desktop")
        .inner_size(screen.logical.w, screen.logical.h)
        .position(screen.logical.x, screen.logical.y)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .resizable(false)
        .skip_taskbar(true)
        .always_on_top(false)
        .build()?;

    place_on_screen(app, &win, screen);

    #[cfg(windows)]
    {
        let stay = load_settings(app).stay_on_desktop;
        if let Ok(hwnd) = win.hwnd() {
            desktop_pin::apply_overlay_desktop_pin(label, hwnd.0 as isize, stay);
        }
    }

    Ok(())
}

/// Put a window exactly on a screen, in physical px — and doing it through the window
/// rather than the builder is what lets WebView2 pick up that screen's scale.
pub(crate) fn place_on_screen(app: &AppHandle, win: &tauri::WebviewWindow, screen: &screens::Screen) {
    use tauri::{PhysicalPosition, PhysicalSize};
    if let Ok(size) = win.inner_size() {
        let scale = win.scale_factor().unwrap_or(1.0);
        let same_size = (size.width as f64 - screen.physical.w).abs() < 2.0
            && (size.height as f64 - screen.physical.h).abs() < 2.0;
        let same_scale = (scale - screen.scale).abs() < 0.01;
        if same_size && same_scale {
            return;
        }
    }
    log_line(
        app,
        &format!("{}: closing the gap to screen {}", win.label(), screen.name),
    );
    let _ = win.set_position(PhysicalPosition::new(screen.physical.x, screen.physical.y));
    let _ = win.set_size(PhysicalSize::new(
        screen.physical.w.max(1.0) as u32,
        screen.physical.h.max(1.0) as u32,
    ));
}

/// Take down the overlays of screens that are gone. Anything on them is re-homed by
/// `rehome_stranded` before this runs.
pub(crate) fn close_extra_overlays(app: &AppHandle) {
    let count = screens(app).len().max(1);
    for (label, _) in app.webview_windows() {
        if let Some(index) = overlay_index(&label) {
            if index >= count {
                log_line(app, &format!("{label}: screen {index} is gone, taking it down"));
                if let Some(w) = app.get_webview_window(&label) {
                    let _ = w.close();
                }
            }
        }
    }
}

/// The layer for the widgets that were pinned above other windows, on one screen.
///
/// A pinned widget cannot be drawn in the desktop overlay (the whole window is in the
/// desktop layer, so the flag there lifts every widget at once), and it cannot have a
/// window of its own either: an icon is 92x112, and its right-click menu is taller than
/// that — a menu drawn in a window that size is clipped to the window. So pinned
/// widgets get a second, full-screen overlay that *is* always on top, built while at
/// least one widget on that screen is pinned and taken down again when none is.
pub(crate) fn spawn_top_overlay(app: &AppHandle, index: usize) -> tauri::Result<()> {
    let label = overlay_label(TOP_LAYER, index);
    if app.get_webview_window(&label).is_some() {
        return Ok(());
    }
    let Some(screen) = screens(app).get(index).cloned() else {
        return Ok(());
    };
    log_line(
        app,
        &format!(
            "spawn {label} on screen {} at {},{} size {}x{} @{}x",
            screen.name, screen.physical.x, screen.physical.y, screen.physical.w, screen.physical.h, screen.scale
        ),
    );
    let win = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html#/overlay".into()))
        .title("Floaty On Top")
        .inner_size(screen.logical.w, screen.logical.h)
        .position(screen.logical.x, screen.logical.y)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .resizable(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .focused(false)
        .build()?;

    place_on_screen(app, &win, &screen);

    #[cfg(windows)]
    {
        if let Ok(hwnd) = win.hwnd() {
            desktop_pin::apply_overlay_desktop_pin(&label, hwnd.0 as isize, false);
        }
    }
    // and straight to the front: the band it has to win is the topmost one
    raise_top_layer(app);
    Ok(())
}

pub(crate) fn spawn_top_overlay_async(app: &AppHandle, index: usize) {
    let handle = app.clone();
    std::thread::spawn(move || {
        let h2 = handle.clone();
        if let Err(e) = handle.run_on_main_thread(move || {
            if let Err(e) = spawn_top_overlay(&h2, index) {
                log_line(&h2, &format!("spawn {} FAILED: {e}", overlay_label(TOP_LAYER, index)));
            }
        }) {
            log_line(&handle, &format!("main-thread dispatch FAILED for the top layer: {e}"));
        }
    });
}

/// Bring the always-on-top layers in line with the records: a screen with a pinned
/// widget gets one, a screen with none takes its layer down — an idle screen is not
/// paying for a whole second full-screen webview.
pub(crate) fn sync_top_overlay(app: &AppHandle) {
    let list = screens(app);
    let mut pinned_on = vec![0usize; list.len()];
    // Collected before anything is counted: holding the lock while creating windows is
    // how a deadlock gets built.
    let pinned: Vec<(f64, f64)> = {
        let state = app.state::<AppState>();
        let Ok(guard) = state.0.lock() else {
            return;
        };
        guard
            .widgets
            .values()
            .filter(|r| wants_on_top(r))
            .map(|r| (r.x as f64, r.y as f64))
            .collect()
    };
    for (x, y) in pinned {
        if let Some(index) = screens::screen_of(&list, x, y) {
            pinned_on[index] += 1;
        }
    }
    for (index, count) in pinned_on.iter().enumerate() {
        let label = overlay_label(TOP_LAYER, index);
        let exists = app.get_webview_window(&label).is_some();
        if *count > 0 && !exists {
            log_line(app, &format!("{count} pinned widget(s): building {label}"));
            spawn_top_overlay_async(app, index);
        } else if *count == 0 && exists {
            log_line(app, &format!("nothing is pinned on that screen: taking {label} down"));
            if let Some(w) = app.get_webview_window(&label) {
                let _ = w.close();
            }
        }
    }
}

/// Pin a widget above other windows, or let it back down into the desktop layer.
///
/// The flag cannot be set on the window that holds the widget: the desktop
/// overlay holds *every* widget, so setting it there lifted the whole desktop
/// layer — which is what "Pin on top" used to do to all of them. The widget is
/// re-homed into the top layer instead, and back when it is unpinned. Both
/// layers mount the same plugins from the same records, and both are told to
/// reconcile, so each one draws exactly the widgets that belong to it.
#[tauri::command]
pub(crate) fn floaty_set_on_top(id: String, on_top: bool, app: AppHandle) -> Result<(), String> {
    let rec = {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        let rec = guard.widgets.get_mut(&id).ok_or("widget not found")?;
        if let Some(obj) = rec.data.as_object_mut() {
            obj.insert("on_top".to_string(), serde_json::Value::Bool(on_top));
        }
        rec.clone()
    };
    persist(&app);
    sync_top_overlay(&app);
    // straight to the front when something is pinned, rather than waiting for the
    // next watchdog tick
    raise_top_layer(&app);

    // Whichever layer was drawing it drops it, and the layer that should draw it
    // mounts it again: `removed` first, so nothing is mounted twice.
    app.emit("floaty-widget-removed", &id).ok();
    app.emit("floaty-widget-added", &rec).ok();
    log_line(&app, &format!("on_top {id} = {on_top}"));
    Ok(())
}

#[tauri::command]
pub(crate) async fn floaty_update_hit_rects(label: String, rects: Vec<desktop_pin::HitRect>) {
    desktop_pin::set_hit_rects(label, rects);
}

#[tauri::command]
pub(crate) fn floaty_set_overlay_dragging(label: String, dragging: bool) {
    desktop_pin::set_dragging(label, dragging);
}

/// Create a widget window without blocking the calling (command) thread.
/// Window creation must run on the main thread; blocking a command thread on
/// that dispatch wedged the settings UI whenever the main loop was slow to
/// pump it, so hand it to a throwaway thread and return immediately.
pub(crate) fn spawn_widget_async(app: &AppHandle, rec: &WidgetRecord) {
    let handle = app.clone();
    let owned = rec.clone();
    std::thread::spawn(move || {
        let h2 = handle.clone();
        let id = owned.id.clone();
        if let Err(e) = handle.run_on_main_thread(move || {
            if let Err(e) = spawn_widget(&h2, &owned) {
                log_line(&h2, &format!("spawn FAILED for {id}: {e}"));
            }
        }) {
            log_line(&handle, &format!("main-thread dispatch FAILED: {e}"));
        }
    });
}

pub(crate) fn spawn_widget(app: &AppHandle, rec: &WidgetRecord) -> tauri::Result<()> {
    let label = widget_label(&rec.id);
    if app.get_webview_window(&label).is_some() {
        return Ok(());
    }
    let (w, h) = plugins::size(&rec.kind, &rec.data);
    let url = format!("index.html#/{}/{}", rec.kind, rec.id);
    log_line(app, &format!("spawn {} at {},{}", label, rec.x, rec.y));
    let win = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title("Floaty")
        .inner_size(w, h)
        .position(rec.x as f64, rec.y as f64)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        // delegates resizability to the plugin definition
        .resizable(plugins::resizable(&rec.kind))
        .skip_taskbar(true)
        // desktop layer for everything by default; pin-on-top is a *layer* the
        // backend owns (see `spawn_top_overlay`), not a per-window flag
        .always_on_top(false)
        .build()?;
    #[cfg(windows)]
    {
        let stay = load_settings(app).stay_on_desktop;
        if let Ok(hwnd) = win.hwnd() {
            desktop_pin::apply_desktop_pin(hwnd.0 as isize, stay);
        }
    }
    Ok(())
}

/// Show a widget in whichever mode is running: the overlays mount it from the
/// record, per-window mode spawns its own window. Every "make this widget
/// visible" path must go through here — spawning a window while an overlay is up
/// puts a second copy of the widget on the desktop.
///
/// Every overlay hears the event and mounts the widget only if it belongs to
/// that overlay's layer (see `overlays`), which is what keeps a pinned widget in
/// the top layer and everything else in the desktop layer.
pub(crate) fn show_widget(app: &AppHandle, rec: &WidgetRecord) {
    if is_overlay_running(app) {
        app.emit("floaty-widget-added", rec).ok();
    } else {
        spawn_widget_async(app, rec);
    }
}

/// Counterpart of `show_widget`: hide it again without leaving anything behind.
pub(crate) fn hide_widget(app: &AppHandle, id: &str) {
    if is_overlay_running(app) {
        app.emit("floaty-widget-removed", id).ok();
    } else {
        close_widget_async(app, id);
    }
}

pub(crate) fn create_record_with(
    app: &AppHandle,
    kind: &str,
    data: serde_json::Value,
    at: Option<(i32, i32)>,
) -> Result<WidgetRecord, String> {
    if !plugins::exists(kind) {
        return Err("unknown widget kind".into());
    }
    {
        let s = load_settings(app);
        if s.disabled.iter().any(|d| d == kind) {
            // creating a disabled plugin's widget re-enables the plugin
            set_plugin_enabled(app, kind, true);
        }
    }
    let rec = {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        guard.next += 1;
        let n = guard.next;
        let id = format!("{kind}-{n}");
        let (x, y) = at.unwrap_or((140 + ((n as i32 * 47) % 480), 140 + ((n as i32 * 31) % 320)));
        let rec = WidgetRecord {
            id: id.clone(),
            kind: kind.to_string(),
            x,
            y,
            data,
        };
        guard.widgets.insert(id, rec.clone());
        rec
    };
    persist(app);
    show_widget(app, &rec);
    Ok(rec)
}

pub(crate) fn create_record(app: &AppHandle, kind: &str) -> Result<WidgetRecord, String> {
    let data = plugins::default_data(kind);
    create_record_with(app, kind, data, None)
}

pub(crate) fn show_settings(app: &AppHandle) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window("settings") {
        log_line(app, "settings window exists, showing");
        w.show()?;
        w.set_focus()?;
        // An earlier session's desktop-layer pass could have left it a taskbar-less
        // tool window; opening it is the moment to hand it back.
        #[cfg(windows)]
        if let Ok(hwnd) = w.hwnd() {
            if desktop_pin::restore_ordinary_window(hwnd.0 as isize) {
                log_line(app, "settings: handed back to the shell as an ordinary window");
            }
        }
        return Ok(());
    }
    log_line(app, "creating settings window");
    let w = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("Floaty Settings")
        .inner_size(500.0, 660.0)
        .transparent(false)
        .decorations(true)
        .resizable(true)
        .skip_taskbar(false)
        .build()?;
    log_line(app, &format!("settings built, visible={:?}", w.is_visible()));
    Ok(())
}
