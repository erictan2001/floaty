use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

mod audio;
mod fs_watch;
mod plugins;
mod shell_ops;
mod sysmon;
use plugins::PluginInfo;

// ---------- logging ----------

/// `[hh:mm:ss.mmm]`, so a recovery that took ten seconds says so in the log.
fn timestamp() -> String {
    #[cfg(windows)]
    {
        use windows::Win32::System::SystemInformation::GetLocalTime;
        let t = unsafe { GetLocalTime() };
        format!(
            "[{:02}:{:02}:{:02}.{:03}]",
            t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
        )
    }
    #[cfg(not(windows))]
    {
        String::new()
    }
}

/// Log lines now come from several threads (power callbacks, the pump watchdog);
/// without this the appends interleave.
static LOG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn log_line(app: &AppHandle, msg: &str) {
    let _serialised = LOG_LOCK.lock();
    let msg = &format!("{} {msg}", timestamp());
    eprintln!("[floaty] {msg}");
    let Ok(dir) = app.path().app_data_dir().map(|d| {
        fs::create_dir_all(&d).ok();
        d
    }) else {
        return;
    };
    let path = dir.join("floaty.log");
    // cap log at ~100KB
    if fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > 100_000 {
        fs::write(&path, "").ok();
    }
    use std::fmt::Write as _;
    let mut line = String::new();
    let _ = writeln!(line, "{msg}");
    use std::io::Write as _;
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()))
        .ok();
}

// ---------- store ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WidgetRecord {
    id: String,
    kind: String,
    /// logical px (frontend converts via scaleFactor; builder takes logical)
    x: i32,
    y: i32,
    data: serde_json::Value,
}

#[derive(Default)]
struct StoreData {
    widgets: HashMap<String, WidgetRecord>,
    next: u64,
    /// ids removed at runtime (remove / folder-merge): late saves from dying
    /// windows must not resurrect them
    dead: HashSet<String>,
}

struct AppState(Mutex<StoreData>);

fn store_file(app: &AppHandle) -> std::path::PathBuf {
    let dir = app
        .path()
        .app_data_dir()
        .expect("app data dir should resolve");
    fs::create_dir_all(&dir).ok();
    dir.join("floaty-store.json")
}

/// Write a text file atomically, keeping the previous contents as `<name>.bak`.
///
/// The store used to be written in place: being killed mid-write left a
/// truncated JSON file, which then loaded as "no widgets at all" — that is how
/// the desktop sometimes came back empty.
fn write_text_atomic(path: &std::path::Path, text: &str) {
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, text).is_err() {
        return;
    }
    if fs::rename(&tmp, path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

/// Seconds between backup rotations while the app is running.
const BACKUP_EVERY_SECS: u64 = 60;
static LAST_BACKUP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Refresh `<name>.bak` from the current file. Rotated on a timer (and once at
/// startup) rather than on every save: a session that came up on an empty store
/// used to write its freshly re-synced contents straight over the only good
/// copy, which turned a recoverable crash into a permanent loss.
fn refresh_backup(path: &std::path::Path, force: bool) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if !force {
        let last = LAST_BACKUP.load(std::sync::atomic::Ordering::Relaxed);
        if last != 0 && now.saturating_sub(last) < BACKUP_EVERY_SECS {
            return;
        }
    }
    LAST_BACKUP.store(now, std::sync::atomic::Ordering::Relaxed);
    if path.exists() {
        let _ = fs::copy(path, path.with_extension("bak"));
    }
}

/// Set while a write is outstanding; the writer clears it before writing.
static STORE_DIRTY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Mark the store dirty and let the background writer do the work.
///
/// `floaty_save` used to serialise and write the whole store itself — ~3MB of
/// JSON with every icon base64-inlined. A single remount (every widget reports
/// the position the layout just gave it) is sixty of those, and on the main
/// thread that is a measured **18.8 seconds** of the app not answering, which is
/// the "Not Responding" the user sees after a wake-up or a restart. Coalescing
/// turns the sixty writes into one.
fn persist(app: &AppHandle) {
    STORE_DIRTY.store(true, std::sync::atomic::Ordering::SeqCst);
    static WRITER: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    WRITER.get_or_init(|| {
        std::thread::spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_millis(300));
            if !STORE_DIRTY.swap(false, std::sync::atomic::Ordering::SeqCst) {
                continue;
            }
            if let Some(app) = shared_app() {
                write_store_now(&app);
            }
        });
    });
    let _ = app;
}

/// Write the store immediately, on this thread. Used by the background writer and
/// by the exit path, which cannot wait for a timer.
fn write_store_now(app: &AppHandle) {
    // Serialize under the lock, write after releasing it: holding the store
    // mutex across file IO serialized every save/list/create and stalled the
    // settings window while widgets persisted positions in the background.
    let json = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().expect("store lock");
        let list: Vec<&WidgetRecord> = guard.widgets.values().collect();
        serde_json::to_string_pretty(&list).unwrap_or_else(|_| "[]".to_string())
    };
    let store = store_file(app);
    let started = std::time::Instant::now();
    let bytes = json.len();
    write_text_atomic(&store, &json);
    // never let an empty desktop become the backup
    if json.len() > 4 {
        refresh_backup(&store, false);
    }
    let ms = started.elapsed().as_millis();
    if ms > 120 {
        log_line(
            app,
            &format!(
                "store: wrote {:.1}MB in {}ms (this is the main thread; it used to happen                  once per widget, which is what made the app stop answering)",
                bytes as f64 / (1024.0 * 1024.0),
                ms
            ),
        );
    }
}

/// Parse a JSON file, falling back to its `.bak` copy when the file is missing
/// **or its JSON is damaged**. Returns the value and whether the backup was
/// needed — a truncated write must not be mistaken for "nothing to load".
fn read_json_with_backup<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
) -> Option<(T, bool)> {
    if let Ok(text) = fs::read_to_string(path) {
        if !text.trim().is_empty() {
            if let Ok(value) = serde_json::from_str::<T>(&text) {
                return Some((value, false));
            }
        }
    }
    let bak = path.with_extension("bak");
    if let Ok(text) = fs::read_to_string(&bak) {
        if let Ok(value) = serde_json::from_str::<T>(&text) {
            return Some((value, true));
        }
    }
    None
}

/// Keep the damaged file instead of throwing it away, so a bad write is still
/// diagnosable after the fact.
fn quarantine(path: &std::path::Path) {
    if path.exists() {
        let _ = fs::rename(path, path.with_extension("corrupt"));
    }
}

fn load_all(app: &AppHandle) -> Vec<WidgetRecord> {
    let path = store_file(app);
    match read_json_with_backup::<Vec<WidgetRecord>>(&path) {
        Some((list, from_backup)) => {
            if from_backup {
                log_line(
                    app,
                    &format!("store unreadable; recovered {} widgets from backup", list.len()),
                );
                quarantine(&path);
                if let Ok(json) = serde_json::to_string_pretty(&list) {
                    write_text_atomic(&path, &json);
                }
            }
            list
        }
        None => {
            if path.exists() {
                log_line(app, "store and backup both unreadable; desktop starts empty");
                quarantine(&path);
            }
            vec![]
        }
    }
}

// ---------- global floating settings ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FloatSettings {
    #[serde(default = "default_pet_speed")]
    pet_speed: f64,
    #[serde(default = "default_gravity")]
    gravity: f64,
    #[serde(default = "default_bounce")]
    bounce: f64,
    #[serde(default = "default_single_click")]
    single_click: String,
    #[serde(default = "default_double_click")]
    double_click: String,
    /// live2d model library root picked by the user
    #[serde(default)]
    live2d_root: String,
    /// root directory where files and folders should be floaties
    #[serde(default)]
    files_root: String,
    /// plugin kinds whose windows stay closed
    #[serde(default)]
    disabled: Vec<String>,
    /// keep widgets visible on desktop when Show Desktop (Win+D) is triggered
    #[serde(default = "default_stay_on_desktop")]
    stay_on_desktop: bool,
    /// launch floaty when the user signs in (a Run entry in the registry)
    #[serde(default = "default_start_on_boot")]
    start_on_boot: bool,
    /// percentage of floaties that participate in animation (0 to 100)
    #[serde(default = "default_animated_ratio")]
    animated_ratio: f64,
    /// animation mode: "wave", "sync", "gentle", "static"
    #[serde(default = "default_animation_mode")]
    animation_mode: String,
    /// how far a resting icon rises, in px — the same travel in every mode.
    /// (Was multiplied by a second `floatiness` slider, which is folded in once
    /// by `migrate_float_settings`.)
    #[serde(default = "default_float_amplitude")]
    float_amplitude: f64,
    /// seconds per bob, in every mode
    #[serde(default = "default_float_period")]
    float_period: f64,
    /// wave only: how much of the cycle separates one icon from the next, as a
    /// percentage of one cycle
    #[serde(default = "default_float_spread")]
    float_spread: f64,
    /// ask before removing a floatie (file, folder or widget)
    #[serde(default = "default_confirm_remove")]
    confirm_remove: bool,
    /// visualizer sensitivity multiplier
    #[serde(default = "default_viz_gain")]
    viz_gain: f64,
    /// visualizer redraw rate (frames per second, while audio plays)
    #[serde(default = "default_viz_fps")]
    viz_fps: f64,
    /// system monitor sample period (milliseconds)
    #[serde(default = "default_sysmon_interval")]
    sysmon_interval: f64,
    /// version of the icon resolution pipeline the stored icons came from.
    /// Bumping `ICON_PIPELINE` re-resolves every stored icon once, which is how
    /// icons produced by an older (worse) pipeline get replaced.
    #[serde(default = "default_icon_pipeline")]
    icon_pipeline: u32,
}

/// Bump when the icon resolver changes what it produces, so already-stored
/// icons are refreshed once. v2: shell item image + alpha-preserving PNG.
/// v3: pick the route whose artwork actually fills the frame.
const ICON_PIPELINE: u32 = 3;

fn default_icon_pipeline() -> u32 {
    // Old settings files predate the field; 0 means "resolve everything once".
    0
}

fn default_stay_on_desktop() -> bool {
    true
}

fn default_start_on_boot() -> bool {
    false
}

fn default_float_amplitude() -> f64 {
    6.0
}

fn default_float_period() -> f64 {
    10.0
}

fn default_float_spread() -> f64 {
    10.0
}

fn default_confirm_remove() -> bool {
    true
}

fn default_viz_gain() -> f64 {
    1.0
}

fn default_viz_fps() -> f64 {
    30.0
}

fn default_sysmon_interval() -> f64 {
    1000.0
}

fn default_animated_ratio() -> f64 {
    100.0
}

fn default_animation_mode() -> String {
    "wave".to_string()
}

fn default_pet_speed() -> f64 {
    1.0
}
fn default_gravity() -> f64 {
    2600.0
}
fn default_bounce() -> f64 {
    0.45
}
fn default_single_click() -> String {
    "drop".to_string()
}
fn default_double_click() -> String {
    "launch".to_string()
}

fn settings_file(app: &AppHandle) -> std::path::PathBuf {
    let dir = app
        .path()
        .app_data_dir()
        .expect("app data dir should resolve");
    fs::create_dir_all(&dir).ok();
    dir.join("floaty-settings.json")
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
fn migrate_float_settings(value: &mut serde_json::Value) {
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

fn load_settings(app: &AppHandle) -> FloatSettings {
    let path = settings_file(app);
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
            ..s
        },
        None => FloatSettings {
            pet_speed: default_pet_speed(),
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
            icon_pipeline: default_icon_pipeline(),
        },
    }
}

#[tauri::command]
async fn floaty_get_settings(app: AppHandle) -> FloatSettings {
    load_settings(&app)
}

#[tauri::command]
fn floaty_set_settings(settings: FloatSettings, app: AppHandle) -> FloatSettings {
    // icon_pipeline is the backend's own bookkeeping (it decides whether stored
    // icons need re-resolving); the settings UI does not know about it, so keep
    // whatever is already on disk instead of letting a save reset it to 0.
    let stored = load_settings(&app);
    let stored_pipeline = stored.icon_pipeline;
    let mut s = FloatSettings {
        pet_speed: settings.pet_speed.clamp(0.0, 3.0),
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
        icon_pipeline: stored_pipeline.max(settings.icon_pipeline),
    };
    // Start on boot is a registry entry, not a preference: write it now and, if
    // that fails, put the old value back rather than save a checkbox that claims
    // something Windows does not agree with.
    if s.start_on_boot != stored.start_on_boot && !set_start_on_boot(&app, s.start_on_boot) {
        s.start_on_boot = stored.start_on_boot;
    }
    if let Ok(json) = serde_json::to_string_pretty(&s) {
        write_text_atomic(&settings_file(&app), &json);
    }
    #[cfg(windows)]
    {
        if let Some(win) = app.get_webview_window("desktop-overlay") {
            if let Ok(hwnd) = win.hwnd() {
                desktop_pin::apply_overlay_desktop_pin("desktop-overlay", hwnd.0 as isize, s.stay_on_desktop);
            }
        }
        if let Some(win) = app.get_webview_window("top-overlay") {
            // the always-on-top layer is never parented into the desktop
            if let Ok(hwnd) = win.hwnd() {
                desktop_pin::apply_overlay_desktop_pin("top-overlay", hwnd.0 as isize, false);
            }
        }
        for (label, win) in app.webview_windows() {
            // the overlays were handled above, and they are pinned through
            // `apply_overlay_desktop_pin` (which is also what registers their
            // click-through region)
            if label == "desktop-overlay" || label == "top-overlay" {
                continue;
            }
            if let Ok(hwnd) = win.hwnd() {
                if is_desktop_layer_label(&label) {
                    desktop_pin::apply_desktop_pin(hwnd.0 as isize, s.stay_on_desktop);
                } else if desktop_pin::restore_ordinary_window(hwnd.0 as isize) {
                    // settings, the manager: ordinary windows. A settings save is
                    // frequent enough to repair one that an earlier pass pinned,
                    // and saying so is what makes the repair visible in the log.
                    log_line(
                        &app,
                        &format!("settings: {label} handed back to the shell as an ordinary window"),
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
    s
}

// ---------- start on boot ----------

/// Make Windows start floaty when the user signs in — or stop doing that.
///
/// The setting is only a record of the intent; the `Run` entry is what actually
/// starts the app, so the two have to be written together. Returns false when the
/// registry write failed, which is why callers put the old value back instead of
/// logging and moving on: a switch that says "on" while nothing happens at sign-in
/// is worse than one that visibly refuses to move.
///
/// Called on every save that changes the value, and once at launch, where it also
/// repairs an entry left pointing at an older copy of the executable.
fn set_start_on_boot(app: &AppHandle, on: bool) -> bool {
    use tauri_plugin_autostart::ManagerExt;

    let manager = app.autolaunch();
    // `is_enabled` reads the entry back, so this is a no-op when it already says
    // what we want (a launch, or a save that did not touch the setting).
    if manager.is_enabled().unwrap_or(false) == on {
        return true;
    }

    match if on { manager.enable() } else { manager.disable() } {
        Ok(()) => {
            log_line(
                app,
                if on {
                    "autostart: floaty will start when you sign in"
                } else {
                    "autostart: floaty will not start on its own"
                },
            );
            true
        }
        Err(e) => {
            let what = if on { "enable" } else { "disable" };
            log_line(app, &format!("autostart: FAILED to {what}: {e}"));
            false
        }
    }
}

// ---------- desktop window pinning (stay on desktop / Win+D) ----------

#[cfg(windows)]
mod desktop_pin {
    use std::sync::atomic::{AtomicBool, Ordering};

    pub static STAY_ON_DESKTOP_ACTIVE: AtomicBool = AtomicBool::new(true);

    pub fn set_stay_on_desktop_global(active: bool) {
        STAY_ON_DESKTOP_ACTIVE.store(active, Ordering::SeqCst);
    }

    const GWLP_HWNDPARENT: i32 = -8;
    const GWL_STYLE: i32 = -16;
    const GWL_EXSTYLE: i32 = -20;
    const WS_CAPTION: isize = 0x00C00000;
    const WS_THICKFRAME: isize = 0x00040000;
    const WS_BORDER: isize = 0x00800000;
    const WS_DLGFRAME: isize = 0x00400000;
    const WS_EX_TOOLWINDOW: isize = 0x00000080;
    const WS_EX_APPWINDOW: isize = 0x00040000;

    const WM_ERASEBKGND: u32 = 0x0014;
    const WM_NCCALCSIZE: u32 = 0x0083;
    const WM_NCPAINT: u32 = 0x0085;
    const WM_NCACTIVATE: u32 = 0x0086;

    const DWMWA_NCRENDERING_POLICY: u32 = 2;
    const DWMNCRP_DISABLED: u32 = 1;

    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_FRAMECHANGED: u32 = 0x0020;
    const SWP_HIDEWINDOW: u32 = 0x0080;
    const HWND_TOPMOST: isize = -1;

    const WM_SYSCOMMAND: u32 = 0x0112;
    const SC_MINIMIZE: usize = 0xF020;
    const WM_WINDOWPOSCHANGING: u32 = 0x0046;
    const WM_SIZE: u32 = 0x0005;
    const SIZE_MINIMIZED: usize = 1;
    const SW_RESTORE: i32 = 9;

    const SUBCLASS_ID: usize = 0x464C5459; // 'FLTY'

    #[repr(C)]
    struct WINDOWPOS {
        hwnd: isize,
        hwnd_insert_after: isize,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    }

    #[allow(non_snake_case)]
    #[repr(C)]
    struct ITaskbarListVtbl {
        pub QueryInterface: unsafe extern "system" fn(this: *mut std::ffi::c_void, riid: *const u8, ppv: *mut *mut std::ffi::c_void) -> i32,
        pub AddRef: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> u32,
        pub Release: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> u32,
        pub HrInit: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> i32,
        pub AddTab: unsafe extern "system" fn(this: *mut std::ffi::c_void, hwnd: isize) -> i32,
        pub DeleteTab: unsafe extern "system" fn(this: *mut std::ffi::c_void, hwnd: isize) -> i32,
        pub ActivateTab: unsafe extern "system" fn(this: *mut std::ffi::c_void, hwnd: isize) -> i32,
        pub SetActiveAlt: unsafe extern "system" fn(this: *mut std::ffi::c_void, hwnd: isize) -> i32,
    }

    #[allow(non_snake_case)]
    #[repr(C)]
    struct ITaskbarList {
        pub lpVtbl: *const ITaskbarListVtbl,
    }

    #[allow(non_snake_case)]
    #[link(name = "user32")]
    #[link(name = "comctl32")]
    #[link(name = "ole32")]
    #[link(name = "kernel32")]
    #[link(name = "gdi32")]
    #[link(name = "dwmapi")]
    extern "system" {
        pub fn DwmSetWindowAttribute(
            hwnd: isize,
            dwAttribute: u32,
            pvAttribute: *const std::ffi::c_void,
            cbAttribute: u32,
        ) -> i32;
        pub fn FindWindowW(lpClassName: *const u16, lpWindowName: *const u16) -> isize;
        pub fn FindWindowExW(
            hWndParent: isize,
            hWndChildAfter: isize,
            lpszClass: *const u16,
            lpszWindow: *const u16,
        ) -> isize;
        pub fn GetShellWindow() -> isize;
        pub fn GetWindowLongPtrW(hWnd: isize, nIndex: i32) -> isize;
        pub fn SetWindowLongPtrW(hWnd: isize, nIndex: i32, dwNewLong: isize) -> isize;
        pub fn SetWindowPos(
            hWnd: isize,
            hWndInsertAfter: isize,
            X: i32,
            Y: i32,
            cx: i32,
            cy: i32,
            uFlags: u32,
        ) -> isize;
        pub fn ShowWindow(hWnd: isize, nCmdShow: i32) -> i32;
        pub fn IsWindowVisible(hWnd: isize) -> i32;
        pub fn SetWindowSubclass(
            hWnd: isize,
            pfnSubclass: unsafe extern "system" fn(
                hWnd: isize,
                uMsg: u32,
                wParam: usize,
                lParam: isize,
                uIdSubclass: usize,
                dwRefData: usize,
            ) -> isize,
            uIdSubclass: usize,
            dwRefData: usize,
        ) -> i32;
        pub fn DefSubclassProc(hWnd: isize, uMsg: u32, wParam: usize, lParam: isize) -> isize;
        pub fn OpenInputDesktop(dwFlags: u32, fInherit: i32, dwDesiredAccess: u32) -> isize;
        pub fn SetThreadDesktop(hDesktop: isize) -> i32;
        pub fn CoCreateInstance(
            rclsid: *const u8,
            pUnkOuter: *mut std::ffi::c_void,
            dwClsContext: u32,
            riid: *const u8,
            ppv: *mut *mut std::ffi::c_void,
        ) -> i32;
        pub fn CreateRectRgn(x1: i32, y1: i32, x2: i32, y2: i32) -> isize;
        pub fn CombineRgn(hrgnDst: isize, hrgnSrc1: isize, hrgnSrc2: isize, iMode: i32) -> i32;
        pub fn DeleteObject(ho: isize) -> i32;
        pub fn SetWindowRgn(hWnd: isize, hRgn: isize, bRedraw: i32) -> i32;

        pub fn InvalidateRect(hWnd: isize, lpRect: *const std::ffi::c_void, bErase: i32) -> i32;
        pub fn RedrawWindow(
            hWnd: isize,
            lprcUpdate: *const std::ffi::c_void,
            hrgnUpdate: isize,
            flags: u32,
        ) -> i32;
    }

    const RDW_INVALIDATE: u32 = 0x0001;
    const RDW_ERASE: u32 = 0x0004;
    const RDW_ALLCHILDREN: u32 = 0x0080;
    const RDW_UPDATENOW: u32 = 0x0100;

    /// Make the window paint itself from scratch.
    ///
    /// A WebView2's first frame is white and Chromium only repaints the damage it
    /// knows about, so after a standby that white can survive in regions the page
    /// paints nothing into — it shows up as solid white bands through the
    /// translucent tiles sitting above it. Raw Win32, on purpose: this must not
    /// queue behind a wedged renderer.
    pub fn force_repaint(hwnd: isize) {
        if hwnd == 0 {
            return;
        }
        unsafe {
            InvalidateRect(hwnd, std::ptr::null(), 0);
            // No RDW_UPDATENOW on purpose: that waits for the paint to happen, and
            // the paint is what can be stuck. Invalidate and let it come back.
            RedrawWindow(hwnd, std::ptr::null(), 0, RDW_INVALIDATE | RDW_ALLCHILDREN);
        }
    }

    pub fn delete_taskbar_tab(hwnd: isize) {
        unsafe {
            // CLSID_TaskbarList {56FDF342-FD6D-11d0-958A-006097C9A090}
            let clsid: [u8; 16] = [0x42, 0xF3, 0xFD, 0x56, 0x6D, 0xFD, 0xd0, 0x11, 0x95, 0x8A, 0x00, 0x60, 0x97, 0xC9, 0xA0, 0x90];
            // IID_ITaskbarList {56FDF344-FD6D-11d0-958A-006097C9A090}
            let iid: [u8; 16] = [0x44, 0xF3, 0xFD, 0x56, 0x6D, 0xFD, 0xd0, 0x11, 0x95, 0x8A, 0x00, 0x60, 0x97, 0xC9, 0xA0, 0x90];
            let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            if CoCreateInstance(clsid.as_ptr(), std::ptr::null_mut(), 1, iid.as_ptr(), &mut ptr) == 0 && !ptr.is_null() {
                let tbl = ptr as *mut ITaskbarList;
                let vtbl = &*(*tbl).lpVtbl;
                let _ = (vtbl.HrInit)(ptr);
                let _ = (vtbl.DeleteTab)(ptr, hwnd);
                let _ = (vtbl.Release)(ptr);
            }
        }
    }

    pub fn remove_from_taskbar(hwnd: isize) {
        unsafe {
            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let new_style = (ex_style | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW;
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_style);
            SetWindowPos(
                hwnd,
                0,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
            delete_taskbar_tab(hwnd);
        }
    }

    /// The inverse of `delete_taskbar_tab`: put the window's taskbar button back.
    pub fn add_taskbar_tab(hwnd: isize) {
        unsafe {
            // CLSID_TaskbarList {56FDF342-FD6D-11d0-958A-006097C9A090}
            let clsid: [u8; 16] = [
                0x42, 0xF3, 0xFD, 0x56, 0x6D, 0xFD, 0xd0, 0x11, 0x95, 0x8A, 0x00, 0x60, 0x97, 0xC9,
                0xA0, 0x90,
            ];
            // IID_ITaskbarList {56FDF344-FD6D-11d0-958A-006097C9A090}
            let iid: [u8; 16] = [
                0x44, 0xF3, 0xFD, 0x56, 0x6D, 0xFD, 0xd0, 0x11, 0x95, 0x8A, 0x00, 0x60, 0x97, 0xC9,
                0xA0, 0x90,
            ];
            let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            if CoCreateInstance(
                clsid.as_ptr(),
                std::ptr::null_mut(),
                1,
                iid.as_ptr(),
                &mut ptr,
            ) == 0
                && !ptr.is_null()
            {
                let tbl = ptr as *mut ITaskbarList;
                let vtbl = &*(*tbl).lpVtbl;
                let _ = (vtbl.HrInit)(ptr);
                let _ = (vtbl.AddTab)(ptr, hwnd);
                let _ = (vtbl.Release)(ptr);
            }
        }
    }

    /// Hand a window back to the shell as an ordinary app window.
    ///
    /// `apply_desktop_pin` marks a window a tool window and deletes its taskbar
    /// tab, which is what the overlays and the one-window-per-widget floaties
    /// want, and exactly what breaks everything else the app opens: a tool
    /// window with no tab cannot be minimized and come back, never joins
    /// Alt-Tab, and — with the desktop shell as its owner — drags its own title
    /// bar around behind every other window. The settings window is an ordinary
    /// window, so any pass that walks *every* label has to give it back.
    ///
    /// Returns true when it actually changed something, so a caller can say so
    /// in the log instead of repairing silently.
    pub fn restore_ordinary_window(hwnd: isize) -> bool {
        if hwnd == 0 {
            return false;
        }
        unsafe {
            let mut changed = false;
            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let wanted = (ex_style & !WS_EX_TOOLWINDOW) | WS_EX_APPWINDOW;
            if wanted != ex_style {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted);
                SetWindowPos(
                    hwnd,
                    0,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
                changed = true;
            }
            // no owner: an owned window follows its owner's z-order and cannot be
            // activated on its own
            if GetWindowLongPtrW(hwnd, GWLP_HWNDPARENT) != 0 {
                SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, 0);
                changed = true;
            }
            // the tab comes back only for a window that is on screen, so the
            // hidden manager window never puts one up for nothing
            if changed && IsWindowVisible(hwnd) != 0 {
                add_taskbar_tab(hwnd);
            }
            changed
        }
    }

    pub fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn ensure_input_desktop() {
        unsafe {
            let dt = OpenInputDesktop(0, 0, 0x01FF);
            if dt != 0 {
                SetThreadDesktop(dt);
            }
        }
    }

    pub fn get_desktop_shell_hwnd() -> isize {
        ensure_input_desktop();
        let progman_cls = to_wide("Progman");
        let defview_cls = to_wide("SHELLDLL_DefView");
        let workerw_cls = to_wide("WorkerW");

        // 1. Try finding SHELLDLL_DefView directly under Progman (Win 11 24H2+, etc.)
        let progman = unsafe { FindWindowW(progman_cls.as_ptr(), std::ptr::null()) };
        if progman != 0 {
            let defview = unsafe { FindWindowExW(progman, 0, defview_cls.as_ptr(), std::ptr::null()) };
            if defview != 0 {
                return defview;
            }
        }

        // 2. Try finding SHELLDLL_DefView under top-level WorkerW windows (Win 10 / earlier Win 11)
        let mut workerw = unsafe { FindWindowExW(0, 0, workerw_cls.as_ptr(), std::ptr::null()) };
        while workerw != 0 {
            let defview = unsafe { FindWindowExW(workerw, 0, defview_cls.as_ptr(), std::ptr::null()) };
            if defview != 0 {
                return defview;
            }
            workerw = unsafe { FindWindowExW(0, workerw, workerw_cls.as_ptr(), std::ptr::null()) };
        }

        // 3. Fallback to GetShellWindow() or Progman
        let shell = unsafe { GetShellWindow() };
        if shell != 0 {
            return shell;
        }
        progman
    }

    /// `ref_data` for windows the desktop-layer policy applies to. Ordinary
    /// windows get `0` and the policy leaves them alone.
    const DESKTOP_LAYER_REF: usize = 1;

    unsafe extern "system" fn widget_subclass_proc(
        hwnd: isize,
        msg: u32,
        wparam: usize,
        lparam: isize,
        _id_subclass: usize,
        ref_data: usize,
    ) -> isize {
        // The desktop-layer policy lives in this one procedure, and it is only
        // meant for the windows that *are* the desktop. Applied to the settings
        // window it is the "settings is broken" bug: the first drag step stripped
        // its taskbar button (this runs on every WM_WINDOWPOSCHANGING, and a move
        // is a stream of them) and the minimize button did nothing.
        let desktop_layer = ref_data == DESKTOP_LAYER_REF;

        // Enforce WS_EX_TOOLWINDOW is preserved so taskbar never shows the window
        if desktop_layer && msg == WM_WINDOWPOSCHANGING {
            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            if (ex_style & WS_EX_TOOLWINDOW) == 0 || (ex_style & WS_EX_APPWINDOW) != 0 {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex_style | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW);
            }
        }

        // Sleep/resume lands here: the webview is never told, so the app has to
        // put itself back together. Two shapes: a resume code after a classic
        // sleep, or the display switching back on after Modern Standby (which
        // sends no resume at all).
        if msg == crate::WM_POWERBROADCAST {
            if wparam == crate::PBT_POWERSETTINGCHANGE && lparam != 0 {
                let setting = unsafe { &*(lparam as *const crate::POWERBROADCAST_SETTING) };
                if setting.PowerSetting == crate::GUID_CONSOLE_DISPLAY_STATE
                    && setting.DataLength >= 1
                    && setting.Data[0] == 1
                {
                    // Never do this work on the pumping thread: it touches the
                    // webview, the compositor and the disk, and a stall in any of
                    // them makes Windows mark the whole app "Not Responding".
                    std::thread::spawn(|| crate::recover_windows("display back on"));
                    return 1;
                }
            } else if wparam == crate::PBT_APMRESUMEAUTOMATIC
                || wparam == crate::PBT_APMRESUMESUSPEND
                || wparam == crate::PBT_APMRESUMECRITICAL
            {
                std::thread::spawn(|| crate::recover_windows("woke from sleep"));
                return 1;
            }
        }

        if desktop_layer && STAY_ON_DESKTOP_ACTIVE.load(Ordering::Relaxed) {
            // Block SC_MINIMIZE syscommand
            if msg == WM_SYSCOMMAND && (wparam & 0xFFF0) == SC_MINIMIZE {
                return 0;
            }
            // Strip SWP_HIDEWINDOW when Windows tries to hide windows on Win+D / Show Desktop
            if msg == WM_WINDOWPOSCHANGING && lparam != 0 {
                let wp = &mut *(lparam as *mut WINDOWPOS);
                if (wp.flags & SWP_HIDEWINDOW) != 0 {
                    wp.flags &= !SWP_HIDEWINDOW;
                }
            }
            // If window receives SIZE_MINIMIZED, restore it
            if msg == WM_SIZE && wparam == SIZE_MINIMIZED {
                ShowWindow(hwnd, SW_RESTORE);
                return 0;
            }
        }

        // Suppress non-client frame rendering & background erase. This prevents
        // Windows from drawing a default white title bar / caption strip along
        // the top edge of shaped window regions — but only for the windows that
        // have no frame to draw in the first place. Swallowing these on an
        // ordinary window is what made the settings window lose its title bar
        // the first time it was maximized: the frame is painted through
        // WM_NCPAINT, and nothing repaints it once the message is answered with
        // "nothing to do" (the initial frame is only visible because it was
        // painted at creation, before this procedure was armed).
        if desktop_layer {
            if msg == WM_NCCALCSIZE {
                return 0;
            }
            if msg == WM_NCPAINT {
                return 0;
            }
            if msg == WM_NCACTIVATE {
                return 1;
            }
            if msg == WM_ERASEBKGND {
                return 1;
            }
        }

        DefSubclassProc(hwnd, msg, wparam, lparam)
    }

    /// Install the window procedure that blocks minimize/show-desktop tricks and
    /// answers the sleep/resume messages. Idempotent.
    ///
    /// Timing matters: a window's procedure can still be replaced while the
    /// webview is being created, which drops an earlier subclass out of the
    /// chain — and then the display-state notification is delivered into
    /// nothing. Installing it again once the window is up (the first heartbeat)
    /// is what makes the wake-up recovery reliable.
    /// Arm the window procedure on a window. `desktop_layer` decides which
    /// policy the procedure enforces for it: `true` keeps the tool-window style
    /// on and swallows minimize (the overlays, the one-window-per-widget
    /// floaties), `false` leaves an ordinary window alone.
    pub fn install_subclass(hwnd: isize, desktop_layer: bool) {
        if hwnd != 0 {
            unsafe {
                SetWindowSubclass(
                    hwnd,
                    widget_subclass_proc,
                    SUBCLASS_ID,
                    if desktop_layer { DESKTOP_LAYER_REF } else { 0 },
                );
            }
        }
    }

    pub fn apply_desktop_pin(hwnd: isize, stay_on_desktop: bool) {
        unsafe {
            remove_from_taskbar(hwnd);
            // one window takes the display-state notification for the process
            crate::watch_display_state(hwnd);

            // Register subclass (idempotent if already registered)
            install_subclass(hwnd, true);

            if stay_on_desktop {
                let desktop_hwnd = get_desktop_shell_hwnd();
                if desktop_hwnd != 0 {
                    SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, desktop_hwnd);
                }
            } else {
                SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, 0);
            }
            SetWindowPos(
                hwnd,
                0,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct HitRect {
        pub id: String,
        pub x: i32,
        pub y: i32,
        pub w: i32,
        pub h: i32,
    }

    const RGN_OR: i32 = 2;
    const WS_EX_TRANSPARENT: isize = 0x00000020;

    /// The overlay windows and their click-through regions, keyed by window
    /// label: `desktop-overlay` is the desktop layer, `top-overlay` holds the
    /// widgets pinned above other windows. Each window keeps its own region — a
    /// region applied to the wrong window is either a window that swallows every
    /// click on the screen or one that cannot be clicked at all.
    static OVERLAY_HWNDS: std::sync::RwLock<Vec<(String, isize)>> = std::sync::RwLock::new(Vec::new());
    static OVERLAY_HIT_RECTS: std::sync::RwLock<Vec<(String, Vec<HitRect>)>> =
        std::sync::RwLock::new(Vec::new());
    static OVERLAY_IS_DRAGGING: std::sync::RwLock<Vec<String>> = std::sync::RwLock::new(Vec::new());

    fn remember_hwnd(label: &str, hwnd: isize) {
        if let Ok(mut guard) = OVERLAY_HWNDS.write() {
            match guard.iter_mut().find(|(l, _)| l == label) {
                Some(slot) => slot.1 = hwnd,
                None => guard.push((label.to_string(), hwnd)),
            }
        }
    }

    fn hwnd_for(label: &str) -> isize {
        OVERLAY_HWNDS
            .read()
            .ok()
            .and_then(|g| g.iter().find(|(l, _)| l == label).map(|(_, h)| *h))
            .unwrap_or(0)
    }

    fn rects_for(label: &str) -> Vec<HitRect> {
        OVERLAY_HIT_RECTS
            .read()
            .ok()
            .and_then(|g| g.iter().find(|(l, _)| l == label).map(|(_, r)| r.clone()))
            .unwrap_or_default()
    }

    fn is_dragging(label: &str) -> bool {
        OVERLAY_IS_DRAGGING
            .read()
            .map(|g| g.iter().any(|l| l == label))
            .unwrap_or(false)
    }

    pub fn apply_hit_regions(hwnd: isize, rects: &[HitRect]) {
        if hwnd == 0 {
            return;
        }
        unsafe {
            let valid: Vec<&HitRect> = rects.iter().filter(|r| r.w > 0 && r.h > 0).collect();
            if valid.is_empty() {
                // An empty region excludes the entire window from receiving clicks
                let empty = CreateRectRgn(0, 0, 0, 0);
                SetWindowRgn(hwnd, empty, 0);
                return;
            }

            let first = valid[0];
            let combined = CreateRectRgn(first.x, first.y, first.x + first.w, first.y + first.h);
            for r in &valid[1..] {
                let item = CreateRectRgn(r.x, r.y, r.x + r.w, r.y + r.h);
                CombineRgn(combined, combined, item, RGN_OR);
                DeleteObject(item);
            }

            // SetWindowRgn transfers ownership of `combined` to the operating system.
            // bRedraw must be 1: the strip newly added to the region still holds
            // stale (transparent) pixels, and Chromium only repaints damage it
            // knows about — a widget whose pixels were clipped away before the
            // region included them would stay invisible until something else
            // damaged it. Client redraws are safe here: the subclass suppresses
            // WM_NCCALCSIZE / WM_NCPAINT and swallows WM_ERASEBKGND.
            SetWindowRgn(hwnd, combined, 1);
            InvalidateRect(hwnd, std::ptr::null(), 0);
        }
    }

    pub fn set_hit_rects(label: String, rects: Vec<HitRect>) {
        let changed = if let Ok(mut guard) = OVERLAY_HIT_RECTS.write() {
            match guard.iter_mut().find(|(l, _)| *l == label) {
                Some(slot) => {
                    if slot.1 == rects {
                        false
                    } else {
                        slot.1 = rects.clone();
                        true
                    }
                }
                None => {
                    guard.push((label.clone(), rects.clone()));
                    true
                }
            }
        } else {
            false
        };
        let hwnd = hwnd_for(&label);
        if changed && !is_dragging(&label) && hwnd != 0 {
            apply_hit_regions(hwnd, &rects);
        }
    }

    pub fn set_dragging(label: String, dragging: bool) {
        if let Ok(mut guard) = OVERLAY_IS_DRAGGING.write() {
            guard.retain(|l| *l != label);
            if dragging {
                guard.push(label.clone());
            }
        }
        let hwnd = hwnd_for(&label);
        if hwnd != 0 {
            if dragging {
                // Clear the window region during dragging so the entire desktop can receive drag events
                unsafe {
                    SetWindowRgn(hwnd, 0, 0);
                }
            } else {
                apply_hit_regions(hwnd, &rects_for(&label));
            }
        }
    }

    pub fn cleanup_hook() {}

    /// Put a window in front of *everything*, other topmost windows included.
    ///
    /// `WS_EX_TOPMOST` is not exclusive: every topmost window keeps the flag, and
    /// the band is ordered by whoever raised (or was activated) last. A remote
    /// desktop window that puts itself on top therefore covers a window that is
    /// also topmost — measured: RustDesk's session window one slot above the
    /// pinned layer. Re-asserting the position is what "always on top" means in
    /// practice, and it is what desktop "keep on top" utilities do.
    ///
    /// `SWP_NOACTIVATE` is the point of the flags: the z-order changes, the
    /// focus does not — the user keeps typing where they were typing.
    pub fn raise_above_everything(hwnd: isize) {
        if hwnd == 0 {
            return;
        }
        unsafe {
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// Style an overlay window and remember it under its label, so the regions
    /// its page sends land on the right window.
    pub fn apply_overlay_desktop_pin(label: &str, hwnd: isize, stay_on_desktop: bool) {
        apply_desktop_pin(hwnd, stay_on_desktop);
        remember_hwnd(label, hwnd);
        unsafe {
            // Remove WS_EX_TRANSPARENT so the shaped regions receive clicks normally
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex & !WS_EX_TRANSPARENT);

            // Strip caption / thickframe / border styles to eliminate non-client frame
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(WS_CAPTION | WS_THICKFRAME | WS_BORDER | WS_DLGFRAME));

            // Disable DWM non-client rendering policy (no caption, frame, or drop shadows)
            let policy: u32 = DWMNCRP_DISABLED;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_NCRENDERING_POLICY,
                &policy as *const u32 as *const std::ffi::c_void,
                std::mem::size_of::<u32>() as u32,
            );

            SetWindowPos(
                hwnd,
                0,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
        if let Ok(guard) = OVERLAY_HIT_RECTS.read() {
            if let Some((_, rects)) = guard.iter().find(|(l, _)| l == label) {
                apply_hit_regions(hwnd, rects);
            }
        }
    }
}

// ---------- sleep / resume ----------

/// Windows tells every top-level window when the machine goes down and comes
/// back. Nothing else re-creates what a sleep broke.
const WM_POWERBROADCAST: u32 = 0x0218;
const PBT_APMRESUMESUSPEND: usize = 0x0007;
const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;
const PBT_APMRESUMECRITICAL: usize = 0x0006;
const PBT_POWERSETTINGCHANGE: usize = 0x8013;

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
static DISPLAY_STATE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(2);
/// when the display-on callback last ran, in ms since the process started
static DISPLAY_ON_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[repr(C)]
struct DeviceNotifySubscribeParameters {
    callback: Option<
        unsafe extern "system" fn(
            context: *const core::ffi::c_void,
            kind: u32,
            setting: *const core::ffi::c_void,
        ) -> u32,
    >,
    context: *const core::ffi::c_void,
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
fn start_pump_watchdog() {
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

static SHARED_APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();
static RESUME_CLOCK: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
static LAST_RESUME_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The app handle, for the window procedures that only get an HWND.
fn shared_app() -> Option<AppHandle> {
    SHARED_APP.get().cloned()
}

/// Put the pinned layer back in front of everything else.
#[cfg(windows)]
fn raise_top_layer(app: &AppHandle) {
    let Some(w) = app.get_webview_window(TOP_LAYER) else { return };
    if let Ok(hwnd) = w.hwnd() {
        desktop_pin::raise_above_everything(hwnd.0 as isize);
    }
}

#[cfg(not(windows))]
fn raise_top_layer(_app: &AppHandle) {}

/// Keep the pinned layer above other *topmost* windows.
///
/// Being topmost is not enough on its own: Windows keeps every topmost window in
/// one band and orders that band by whoever raised last, so a remote-desktop
/// window that puts itself on top covers the pinned widgets (measured: RustDesk's
/// session window directly above `top-overlay`). Two assertions a second is what
/// "stay on top" costs — a `SetWindowPos` that does not move, resize or activate
/// anything — and the work only happens while the layer exists, which is only
/// while some widget is pinned.
fn start_top_layer_watchdog() {
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
fn recover_windows(reason: &str) {
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
        .filter(|(label, _)| {
            label == DESKTOP_LAYER
                || label == TOP_LAYER
                || label == "settings"
                || label.starts_with("widget-")
        })
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
fn elapsed_ms() -> u64 {
    RESUME_CLOCK.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

/// One-pixel resize and back, straight to the window: no webview API involved.
fn nudge_bounds(hwnd: isize) {
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
fn last_beat(label: &str) -> (u64, String) {
    heartbeats()
        .lock()
        .ok()
        .and_then(|m| m.get(label).cloned())
        .unwrap_or((0, String::new()))
}

/// Last report from each window: when it came in, and what the page thought its
/// own visibility was.
static HEARTBEATS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, (u64, String)>>,
> = std::sync::OnceLock::new();

fn heartbeats() -> &'static std::sync::Mutex<std::collections::HashMap<String, (u64, String)>> {
    HEARTBEATS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// The windows report in every few seconds so the wake-up recovery can tell a
/// live page from one that is wedged.
/// Per-window record of what we did about a stale "hidden" verdict.
static HEARTBEAT_REPAIRS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, (u64, u32)>>,
> = std::sync::OnceLock::new();

fn heartbeat_repairs() -> &'static std::sync::Mutex<std::collections::HashMap<String, (u64, u32)>> {
    HEARTBEAT_REPAIRS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[tauri::command]
fn floaty_heartbeat(label: String, visibility: Option<String>, app: AppHandle) {
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
fn recreate_window(app: &AppHandle, label: &str) {
    if label == DESKTOP_LAYER || label == TOP_LAYER {
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
            let again = if label == TOP_LAYER {
                // only if something is still pinned: nothing pinned means the
                // layer is meant to be gone
                sync_top_overlay(app);
                Ok(())
            } else {
                spawn_overlay_window(app).map(|_| ())
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
fn record_for_window(app: &AppHandle, label: &str) -> Option<WidgetRecord> {
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
fn wants_on_top(rec: &WidgetRecord) -> bool {
    rec.data.get("on_top").and_then(|v| v.as_bool()).unwrap_or(false)
}

/// The desktop layer's window, and the layer a pinned widget is drawn in.
const DESKTOP_LAYER: &str = "desktop-overlay";
const TOP_LAYER: &str = "top-overlay";

/// Whether the overlay mode is running (as opposed to one window per widget).
fn is_overlay_running(app: &AppHandle) -> bool {
    app.get_webview_window(DESKTOP_LAYER).is_some()
}

// ---------- windows ----------

fn widget_label(id: &str) -> String {
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
fn is_desktop_layer_label(label: &str) -> bool {
    label == "desktop-overlay" || label == "top-overlay" || label.starts_with("widget-")
}

/// Extensions the Windows shell launches directly. Anything else that is not a
/// directory is a *file*: it floats with a document icon and opens with its
/// default app instead of being spawned like an executable.
const LAUNCHABLE_EXTS: &[&str] = &[
    "exe", "lnk", "url", "bat", "cmd", "com", "scr", "msi", "appref-ms", "ps1", "psm1", "vbs",
    "vbe", "js", "jse", "wsf", "wsh", "hta", "cpl", "msc", "reg", "jar", "ahk",
];

fn is_launchable_target(path: &std::path::Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => LAUNCHABLE_EXTS.iter().any(|k| ext.eq_ignore_ascii_case(k)),
        None => false,
    }
}

/// Widget kind for a filesystem entry: folder, launchable app, or loose file.
fn kind_for_path(path: &std::path::Path, is_dir: bool) -> &'static str {
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
fn is_path_kind(kind: &str) -> bool {
    plugins::is_path_kind(kind)
}

/// Where the overlay belongs on the primary monitor: logical px, so it matches
/// the coordinates the widgets and the store use.
fn overlay_rect(app: &AppHandle) -> (f64, f64, f64, f64) {
    if let Ok(Some(mon)) = app.primary_monitor() {
        let s = mon.scale_factor();
        let size = mon.size();
        let pos = mon.position();
        (
            (pos.x as f64) / s,
            (pos.y as f64) / s,
            (size.width as f64) / s,
            (size.height as f64) / s,
        )
    } else {
        (0.0, 0.0, 1920.0, 1080.0)
    }
}

/// Follow the primary monitor: the resolution or the DPI can change while the
/// machine is asleep (a dock, a projector, a different scaling), which leaves the
/// overlay the wrong size and the widgets outside it looking stuck.
fn fit_overlay_to_monitor(app: &AppHandle) {
    // both layers are the size of the monitor: the desktop layer and the layer
    // holding the widgets pinned above other windows
    for label in [DESKTOP_LAYER, TOP_LAYER] {
        fit_layer_to_monitor(app, label);
    }
}

fn fit_layer_to_monitor(app: &AppHandle, label: &str) {
    let Some(w) = app.get_webview_window(label) else { return };
    let (x, y, ww, hh) = overlay_rect(app);
    if ww < 100.0 || hh < 100.0 {
        return;
    }
    use tauri::{LogicalPosition, LogicalSize};
    if let Ok(size) = w.inner_size() {
        let scale = w.scale_factor().unwrap_or(1.0);
        let cur_w = size.width as f64 / scale;
        let cur_h = size.height as f64 / scale;
        if (cur_w - ww).abs() < 2.0 && (cur_h - hh).abs() < 2.0 {
            return;
        }
    }
    log_line(app, &format!("{label}: refitting to {x},{y} {ww}x{hh}"));
    let _ = w.set_position(LogicalPosition::new(x, y));
    let _ = w.set_size(LogicalSize::new(ww, hh));
}

fn spawn_overlay_window(app: &AppHandle) -> tauri::Result<()> {
    if app.get_webview_window("desktop-overlay").is_some() {
        return Ok(());
    }

    let (x, y, w, h) = overlay_rect(app);

    log_line(app, &format!("spawn desktop-overlay at {x},{y} size {w}x{h}"));
    let win = WebviewWindowBuilder::new(
        app,
        "desktop-overlay",
        WebviewUrl::App("index.html#/overlay".into()),
    )
    .title("Floaty Desktop")
    .inner_size(w, h)
    .position(x, y)
    .transparent(true)
    .decorations(false)
    .shadow(false)
    .resizable(false)
    .skip_taskbar(true)
    .always_on_top(false)
    .build()?;

    #[cfg(windows)]
    {
        let stay = load_settings(app).stay_on_desktop;
        if let Ok(hwnd) = win.hwnd() {
            desktop_pin::apply_overlay_desktop_pin("desktop-overlay", hwnd.0 as isize, stay);
        }
    }

    Ok(())
}

/// The layer for the widgets that were pinned above other windows.
///
/// A pinned widget cannot be drawn in the desktop overlay (the whole window is
/// in the desktop layer, so the flag there lifts every widget at once), and it
/// cannot have a window of its own either: an icon is 92x112, and its right-click
/// menu is taller than that — a menu drawn in a window that size is clipped to
/// the window. So pinned widgets get a second, full-screen overlay that *is*
/// always on top, built while at least one widget is pinned and taken down again
/// when none is.
fn spawn_top_overlay(app: &AppHandle) -> tauri::Result<()> {
    if app.get_webview_window("top-overlay").is_some() {
        return Ok(());
    }
    let (x, y, w, h) = overlay_rect(app);
    log_line(app, &format!("spawn top-overlay at {x},{y} size {w}x{h}"));
    let win = WebviewWindowBuilder::new(
        app,
        "top-overlay",
        WebviewUrl::App("index.html#/overlay".into()),
    )
    .title("Floaty On Top")
    .inner_size(w, h)
    .position(x, y)
    .transparent(true)
    .decorations(false)
    .shadow(false)
    .resizable(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .focused(false)
    .build()?;

    #[cfg(windows)]
    {
        if let Ok(hwnd) = win.hwnd() {
            desktop_pin::apply_overlay_desktop_pin("top-overlay", hwnd.0 as isize, false);
        }
    }
    // and straight to the front: the band it has to win is the topmost one
    raise_top_layer(app);
    Ok(())
}

fn spawn_top_overlay_async(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        let h2 = handle.clone();
        if let Err(e) = handle.run_on_main_thread(move || {
            if let Err(e) = spawn_top_overlay(&h2) {
                log_line(&h2, &format!("spawn top-overlay FAILED: {e}"));
            }
        }) {
            log_line(&handle, &format!("main-thread dispatch FAILED for top-overlay: {e}"));
        }
    });
}

/// Bring the always-on-top layer in line with the records: it only exists while
/// something is pinned, so an idle desktop is not paying for a whole second
/// full-screen webview.
fn sync_top_overlay(app: &AppHandle) {
    let pinned: Vec<String> = app
        .state::<AppState>()
        .0
        .lock()
        .map(|g| {
            g.widgets
                .values()
                .filter(|r| wants_on_top(r))
                .map(|r| r.id.clone())
                .collect()
        })
        .unwrap_or_default();

    if pinned.is_empty() {
        if let Some(w) = app.get_webview_window("top-overlay") {
            log_line(app, "no widget is pinned: taking the top layer down");
            let _ = w.close();
        }
        return;
    }
    if app.get_webview_window("top-overlay").is_none() {
        log_line(app, &format!("{} pinned widget(s): building the top layer", pinned.len()));
        spawn_top_overlay_async(app);
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
fn floaty_set_on_top(id: String, on_top: bool, app: AppHandle) -> Result<(), String> {
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
async fn floaty_update_hit_rects(label: String, rects: Vec<desktop_pin::HitRect>) {
    desktop_pin::set_hit_rects(label, rects);
}

#[tauri::command]
fn floaty_set_overlay_dragging(label: String, dragging: bool) {
    desktop_pin::set_dragging(label, dragging);
}

/// Create a widget window without blocking the calling (command) thread.
/// Window creation must run on the main thread; blocking a command thread on
/// that dispatch wedged the settings UI whenever the main loop was slow to
/// pump it, so hand it to a throwaway thread and return immediately.
fn spawn_widget_async(app: &AppHandle, rec: &WidgetRecord) {
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

fn spawn_widget(app: &AppHandle, rec: &WidgetRecord) -> tauri::Result<()> {
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
fn show_widget(app: &AppHandle, rec: &WidgetRecord) {
    if is_overlay_running(app) {
        app.emit("floaty-widget-added", rec).ok();
    } else {
        spawn_widget_async(app, rec);
    }
}

/// Counterpart of `show_widget`: hide it again without leaving anything behind.
fn hide_widget(app: &AppHandle, id: &str) {
    if is_overlay_running(app) {
        app.emit("floaty-widget-removed", id).ok();
    } else {
        close_widget_async(app, id);
    }
}

fn create_record_with(
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

fn create_record(app: &AppHandle, kind: &str) -> Result<WidgetRecord, String> {
    let data = plugins::default_data(kind);
    create_record_with(app, kind, data, None)
}

fn show_settings(app: &AppHandle) -> tauri::Result<()> {
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

// ---------- app discovery + launch ----------

#[derive(Debug, Clone, Serialize)]
struct DiscoveredApp {
    name: String,
    path: String,
}

fn collect_lnk(dir: &str, out: &mut Vec<DiscoveredApp>, seen: &mut HashSet<String>, depth: u8) {
    if depth > 2 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if depth < 2 {
                collect_lnk(&p.to_string_lossy(), out, seen, depth + 1);
            }
            continue;
        }
        let is_lnk = p
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case("lnk"))
            .unwrap_or(false);
        if !is_lnk {
            continue;
        }
        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let lower = stem.to_lowercase();
        if stem.is_empty() || lower.contains("uninstall") || lower.contains("unins0") {
            continue;
        }
        if seen.insert(lower) {
            out.push(DiscoveredApp {
                name: stem,
                path: p.to_string_lossy().to_string(),
            });
        }
    }
}

/// Blocking filesystem enumeration, shared by the async command (via
/// spawn_blocking, so a slow/cloud-backed Start Menu walk can never stall the
/// command pipeline and hang the settings window) and the setup demo harness.
fn scan_apps_blocking() -> Vec<DiscoveredApp> {
    let mut dirs: Vec<String> = vec![];
    if let Ok(v) = std::env::var("APPDATA") {
        dirs.push(format!("{v}\\Microsoft\\Windows\\Start Menu\\Programs"));
    }
    if let Ok(v) = std::env::var("PROGRAMDATA") {
        dirs.push(format!("{v}\\Microsoft\\Windows\\Start Menu\\Programs"));
    }
    if let Ok(v) = std::env::var("USERPROFILE") {
        dirs.push(format!("{v}\\Desktop"));
    }
    dirs.push("C:\\Users\\Public\\Desktop".to_string());
    let mut out: Vec<DiscoveredApp> = vec![];
    let mut seen: HashSet<String> = HashSet::new();
    for d in &dirs {
        collect_lnk(d, &mut out, &mut seen, 0);
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.truncate(250);
    out
}

#[tauri::command]
async fn floaty_scan_apps(app: AppHandle) -> Vec<DiscoveredApp> {
    let out = tauri::async_runtime::spawn_blocking(scan_apps_blocking)
        .await
        .unwrap_or_default();
    log_line(&app, &format!("scan found {} apps", out.len()));
    out
}

/// Minimal base64 decoder for the first bytes of a data url (no deps needed).
fn base64_decode_prefix(text: &str, max_bytes: usize) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let take = (max_bytes.div_ceil(3) * 4).max(4);
    let chars: Vec<u8> = text.bytes().take(take).collect();
    let mut out = Vec::with_capacity(max_bytes);
    for chunk in chars.chunks(4) {
        if chunk.len() < 4 {
            break;
        }
        let mut v = [0u8; 4];
        for (i, &c) in chunk.iter().enumerate() {
            v[i] = if c == b'=' { 0 } else { val(c)? };
        }
        out.push((v[0] << 2) | (v[1] >> 4));
        let pad = chunk.iter().filter(|&&c| c == b'=').count();
        if pad < 2 {
            out.push((v[1] << 4) | (v[2] >> 2));
        }
        if pad < 1 {
            out.push((v[2] << 6) | v[3]);
        }
        if out.len() >= max_bytes {
            break;
        }
    }
    out.truncate(max_bytes);
    Some(out)
}

/// Pixel size of a stored icon data url. Reads the PNG header instead of
/// pattern-matching base64 (the old substring test both missed icons and
/// flagged crisp ones, which made the upgrade pass churn).
fn icon_pixel_size(s: &str) -> Option<(u32, u32)> {
    let body = s.strip_prefix("data:image/png;base64,").unwrap_or(s);
    let head = base64_decode_prefix(body, 24)?;
    if head.len() < 24 || &head[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes([head[16], head[17], head[18], head[19]]);
    let h = u32::from_be_bytes([head[20], head[21], head[22], head[23]]);
    Some((w, h))
}

/// Nothing usable stored: empty, the literal "none", or an icon we cannot read.
/// This — not "small" — is what forces a re-resolution, so a widget never
/// re-runs the (expensive) resolver just because its icon is 32px.
fn icon_is_missing(s: &str) -> bool {
    if s.is_empty() || s == "none" {
        return true;
    }
    icon_pixel_size(s).is_none()
}

/// Smaller than the shell's jumbo size, so the background upgrade pass may try
/// for a crisper one. Never a reason to throw a stored icon away.
fn is_low_res_icon(s: &str) -> bool {
    if s.is_empty() || s == "none" {
        return true;
    }
    match icon_pixel_size(s) {
        Some((w, h)) => w < 64 || h < 64,
        None => true,
    }
}

/// Standalone base64 encoder with zero external dependencies
fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        result.push(CHARS[(b0 >> 2) as usize] as char);
        result.push(CHARS[(((b0 & 3) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[(((b1 & 0xF) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(b2 & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

/// Parse ICO binary format directly in Rust and extract the largest embedded PNG frame if available
fn try_extract_png_from_ico_bytes(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 22 {
        return None;
    }
    let id_type = u16::from_le_bytes([bytes[2], bytes[3]]);
    if id_type != 1 {
        return None;
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    if count == 0 || bytes.len() < 6 + count * 16 {
        return None;
    }

    let mut best_width = 0u32;
    let mut best_data: Option<&[u8]> = None;

    for i in 0..count {
        let entry = 6 + i * 16;
        let raw_w = bytes[entry] as u32;
        let w = if raw_w == 0 { 256 } else { raw_w };
        let bytes_in_res = u32::from_le_bytes([
            bytes[entry + 8],
            bytes[entry + 9],
            bytes[entry + 10],
            bytes[entry + 11],
        ]) as usize;
        let image_offset = u32::from_le_bytes([
            bytes[entry + 12],
            bytes[entry + 13],
            bytes[entry + 14],
            bytes[entry + 15],
        ]) as usize;

        if image_offset + bytes_in_res <= bytes.len() && bytes_in_res >= 8 {
            let img = &bytes[image_offset..image_offset + bytes_in_res];
            // PNG magic signature: 0x89 'P' 'N' 'G' 0x0D 0x0A 0x1A 0x0A
            if img.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
                if w > best_width {
                    best_width = w;
                    best_data = Some(img);
                }
            }
        }
    }

    best_data.map(|d| d.to_vec())
}

/// Instantly extract a PNG frame from an ICO file on disk in Rust (sub-millisecond)
fn try_extract_fast_ico(ico_path: &str) -> Option<String> {
    let p = std::path::Path::new(ico_path);
    if !p.is_file() {
        return None;
    }
    let bytes = std::fs::read(p).ok()?;
    let png = try_extract_png_from_ico_bytes(&bytes)?;
    Some(format!("data:image/png;base64,{}", base64_encode(&png)))
}

/// Instantly extract Steam game and internet shortcut (.url) icons in Rust (sub-millisecond)
fn try_extract_fast_url(url_path: &str) -> Option<String> {
    let p = std::path::Path::new(url_path);
    if !p.is_file() {
        return None;
    }
    let content = std::fs::read_to_string(p).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("IconFile=") {
            let ico_path = rest.trim();
            if ico_path.ends_with(".ico") || ico_path.ends_with(".ICO") {
                if let Some(data_url) = try_extract_fast_ico(ico_path) {
                    return Some(data_url);
                }
            }
        }
    }
    None
}

/// A resolved icon plus a fingerprint of the file it came from, so a replaced
/// or re-downloaded file refreshes instead of serving the old icon forever
/// (that was the "some tiles use the previous cache" complaint).
#[derive(Clone)]
struct CachedIcon {
    data: String,
    stamp: Option<(u64, u64)>,
}

fn path_stamp(path: &str) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some((mtime, meta.len()))
}

static ICON_CACHE: std::sync::LazyLock<Mutex<HashMap<String, CachedIcon>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

fn cache_icon(path: &str, data: &str) {
    if let Ok(mut guard) = ICON_CACHE.lock() {
        guard.insert(
            path.to_string(),
            CachedIcon {
                data: data.to_string(),
                stamp: path_stamp(path),
            },
        );
    }
}

/// Resolve icons for `paths`. Cached entries are reused while the file is
/// unchanged; `force` skips the cache so the upgrade pass can really try for a
/// crisper icon instead of being handed back the 32px one it wants to replace.
fn resolve_icons_batch(paths: &[String], force: bool) -> HashMap<String, String> {
    let mut results = HashMap::new();
    let mut needed = Vec::new();

    if let Ok(guard) = ICON_CACHE.lock() {
        for p in paths {
            if !force {
                if let Some(cached) = guard.get(p) {
                    if cached.stamp == path_stamp(p) {
                        results.insert(p.clone(), cached.data.clone());
                        continue;
                    }
                }
            }
            needed.push(p.clone());
        }
    } else {
        needed.extend_from_slice(paths);
    }

    if needed.is_empty() {
        return results;
    }

    // 1. Rust-native fast path for .url and .ico (executes in 0ms without spawning processes)
    let mut still_needed = Vec::new();
    for p in &needed {
        let fast_res = if p.ends_with(".url") || p.ends_with(".URL") {
            try_extract_fast_url(p)
        } else if p.ends_with(".ico") || p.ends_with(".ICO") {
            try_extract_fast_ico(p)
        } else {
            None
        };

        if let Some(icon_data) = fast_res {
            results.insert(p.clone(), icon_data.clone());
            cache_icon(p, &icon_data);
        } else {
            still_needed.push(p.clone());
        }
    }

    if still_needed.is_empty() {
        return results;
    }

    #[cfg(windows)]
    {
        use std::io::Write as _;
        use std::os::windows::process::CommandExt;
        const NO_WINDOW: u32 = 0x08000000;

        let script = r#"
# Explicit UTF-8 on both pipes: the console codepage mangles any path with
# non-ASCII characters, which silently skipped those icons entirely.
$reader = New-Object System.IO.StreamReader([Console]::OpenStandardInput(), (New-Object Text.UTF8Encoding($false)))
$writer = New-Object System.IO.StreamWriter([Console]::OpenStandardOutput(), (New-Object Text.UTF8Encoding($false)))
$writer.AutoFlush = $true
Add-Type -AssemblyName System.Drawing;
# PresentationCore/WindowsBase load WPF, which is what converts the shell's
# HBITMAP to PNG with its alpha channel intact.
Add-Type -AssemblyName PresentationCore, WindowsBase;

$IconFetchSrc = @'
using System;
using System.Runtime.InteropServices;
using System.Drawing;
using System.Drawing.Imaging;
using System.IO;
public static class IconFetch {
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern uint PrivateExtractIcons(string szFileName, int nIconIndex, int cxIcon, int cyIcon, IntPtr[] phicon, uint[] piconid, uint nIcons, uint flags);
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr SHGetFileInfo(string pszPath, uint dwFileAttributes, ref SHFILEINFO psfi, uint cbFileInfo, uint uFlags);
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct SHFILEINFO {
        public IntPtr hIcon;
        public int iIcon;
        public uint dwAttributes;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] public string szDisplayName;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 80)] public string szTypeName;
    }
    [DllImport("user32.dll")]
    public static extern bool DestroyIcon(IntPtr hIcon);
    [DllImport("gdi32.dll")]
    public static extern bool DeleteObject(IntPtr hObject);

    // The shell's own item image: the exact bitmap Explorer draws for a path,
    // and the only route that returns more than 32px for file types whose
    // registered icon is a small resource (pdf / txt / zip / md / folders).
    [ComImport, Guid("bcc18b79-ba16-442f-80c4-8a59c30c463b"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IShellItemImageFactory {
        void GetImage(SIZE size, int flags, out IntPtr phbm);
    }
    [StructLayout(LayoutKind.Sequential)]
    public struct SIZE { public int cx; public int cy; }
    [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
    public static extern void SHCreateItemFromParsingName(string path, IntPtr pbc, ref Guid riid, [MarshalAs(UnmanagedType.Interface)] out object ppv);

    public static IntPtr GetShellImageHandle(string path, int px) {
        Guid iid = new Guid("bcc18b79-ba16-442f-80c4-8a59c30c463b");
        object o;
        SHCreateItemFromParsingName(path, IntPtr.Zero, ref iid, out o);
        var factory = (IShellItemImageFactory)o;
        IntPtr hbm;
        factory.GetImage(new SIZE { cx = px, cy = px }, 4, out hbm); // 4 = SIIGBF_ICONONLY
        return hbm;
    }

    // How much of the frame the artwork actually covers: 0-100, the larger of the
    // width/height span. Some apps ship a 256px frame whose art is drawn tiny in
    // the middle; the shell hands that frame back as-is, so a tile built from it
    // looks like a minimised icon next to the others.
    public static int ArtSpanPercent(string pngB64) {
        try {
            byte[] bytes = Convert.FromBase64String(pngB64);
            using (var ms = new MemoryStream(bytes, false))
            using (var bmp = new Bitmap(ms)) {
                int w = bmp.Width, h = bmp.Height;
                var data = bmp.LockBits(new Rectangle(0, 0, w, h),
                    ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
                try {
                    int stride = data.Stride;
                    byte[] buf = new byte[stride * h];
                    Marshal.Copy(data.Scan0, buf, 0, buf.Length);
                    int minX = w, minY = h, maxX = -1, maxY = -1;
                    for (int y = 0; y < h; y++) {
                        int row = y * stride;
                        for (int x = 0; x < w; x++) {
                            if (buf[row + x * 4 + 3] > 96) {
                                if (x < minX) minX = x;
                                if (x > maxX) maxX = x;
                                if (y < minY) minY = y;
                                if (y > maxY) maxY = y;
                            }
                        }
                    }
                    if (maxX < 0) return 0;
                    int pctW = (maxX - minX + 1) * 100 / w;
                    int pctH = (maxY - minY + 1) * 100 / h;
                    return pctW > pctH ? pctW : pctH;
                } finally {
                    bmp.UnlockBits(data);
                }
            }
        } catch { return -1; }
    }
}
'@

Add-Type -TypeDefinition $IconFetchSrc -ReferencedAssemblies System.Drawing;

# Bitmap -> PNG base64 for a shell icon handle, releasing the handle afterwards.
function Get-HiconB64([IntPtr]$hIcon) {
    if ($hIcon -eq [IntPtr]::Zero) { return $null }
    $res = $null
    try {
        $ico = [System.Drawing.Icon]::FromHandle($hIcon)
        $bmp = $ico.ToBitmap()
        $ms = New-Object IO.MemoryStream
        $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
        $res = [Convert]::ToBase64String($ms.ToArray())
        $ms.Dispose(); $bmp.Dispose()
    } catch { $res = $null }
    [void][IconFetch]::DestroyIcon($hIcon)
    return $res
}

# Real 256px icon straight out of an exe/dll/ico. ExtractAssociatedIcon only ever
# returns 32x32, which is what made app icons look soft.
function Get-BigIconB64($spec) {
    $path = $spec
    $index = 0
    if ($spec -match '^(.*),(\d+)$') { $path = $matches[1]; [void][int]::TryParse($matches[2], [ref]$index) }
    if (-not (Test-Path -LiteralPath $path)) { return $null }
    try {
        $handles = New-Object IntPtr[] 1
        $ids = New-Object uint32[] 1
        $n = [IconFetch]::PrivateExtractIcons($path, $index, 256, 256, $handles, $ids, 1, 0)
        if ($n -ge 1 -and $handles[0] -ne [IntPtr]::Zero) { return Get-HiconB64 $handles[0] }
    } catch {}
    return $null
}

# Icon bitmap straight out of the shell's item image factory. This is what
# Explorer itself draws, and the only way to get 256px for types whose
# registered icon is a 32px resource (.pdf/.txt/.zip/.md/.py/.json, folders).
# The HBITMAP -> PNG step goes through WPF on purpose: System.Drawing's
# Bitmap.FromHbitmap throws the alpha channel away, which turned every shell
# icon into a black square on the desktop.
function Get-ShellImageB64($path) {
    if ([string]::IsNullOrEmpty($path) -or -not (Test-Path -LiteralPath $path)) { return $null }
    $ms = $null
    try {
        $hbm = [IconFetch]::GetShellImageHandle($path, 256)
        if ($hbm -eq [IntPtr]::Zero) { return $null }
        try {
            $src = [System.Windows.Interop.Imaging]::CreateBitmapSourceFromHBitmap($hbm, [IntPtr]::Zero, [System.Windows.Int32Rect]::Empty, [System.Windows.Media.Imaging.BitmapSizeOptions]::FromEmptyOptions())
            $enc = New-Object System.Windows.Media.Imaging.PngBitmapEncoder
            $enc.Frames.Add([System.Windows.Media.Imaging.BitmapFrame]::Create($src))
            $ms = New-Object IO.MemoryStream
            $enc.Save($ms)
            return [Convert]::ToBase64String($ms.ToArray())
        } finally {
            [void][IconFetch]::DeleteObject($hbm)
        }
    } catch { return $null }
    finally { if ($ms) { $ms.Dispose() } }
}

# Directories have no file to extract from, so ask the shell for their glyph.
function Get-DirIconB64($path) {
    $shellImg = Get-ShellImageB64 $path
    if (-not [string]::IsNullOrEmpty($shellImg)) { return $shellImg }
    try {
        $info = New-Object 'IconFetch+SHFILEINFO'
        $size = [Runtime.InteropServices.Marshal]::SizeOf($info)
        $flags = 0x100
        $r = [IconFetch]::SHGetFileInfo($path, 0x10, [ref]$info, [uint32]$size, $flags)
        if ($r -ne [IntPtr]::Zero -and $info.hIcon -ne [IntPtr]::Zero) { return Get-HiconB64 $info.hIcon }
    } catch {}
    return $null
}

function Get-IconB64($filePath) {
    if ([string]::IsNullOrEmpty($filePath) -or -not (Test-Path -LiteralPath $filePath)) { return $null }
    if (Test-Path -LiteralPath $filePath -PathType Container) { return Get-DirIconB64 $filePath }

    if ($filePath.EndsWith('.ico', [StringComparison]::OrdinalIgnoreCase)) {
        # A .ico holds several frames, usually including a 256px one in BMP
        # format. PrivateExtractIcons picks the biggest; the PNG-only frame scan
        # below returned 32-48px for these (Sprite/Steam/App icons).
        $bigIco = Get-BigIconB64 $filePath
        if (-not [string]::IsNullOrEmpty($bigIco)) { return $bigIco }
        try {
            $bytes = [System.IO.File]::ReadAllBytes($filePath)
            if ($bytes.Length -ge 22) {
                $cnt = [BitConverter]::ToUInt16($bytes, 4)
                $bestIdx = -1; $bestW = 0
                for ($i = 0; $i -lt $cnt; $i++) {
                    $off = 6 + $i * 16
                    $w = [int]$bytes[$off]
                    if ($w -eq 0) { $w = 256 }
                    $imgOff = [BitConverter]::ToUInt32($bytes, $off + 12)
                    if ($imgOff + 8 -le $bytes.Length -and $bytes[$imgOff] -eq 0x89 -and $bytes[$imgOff+1] -eq 0x50) {
                        if ($w -gt $bestW) { $bestW = $w; $bestIdx = $i }
                    }
                }
                if ($bestIdx -ge 0) {
                    $off = 6 + $bestIdx * 16
                    $imgOff = [BitConverter]::ToUInt32($bytes, $off + 12)
                    $imgLen = [BitConverter]::ToUInt32($bytes, $off + 8)
                    if ($imgOff + $imgLen -le $bytes.Length) {
                        return [Convert]::ToBase64String($bytes, $imgOff, $imgLen)
                    }
                }
            }
        } catch {}

        try {
            $ico = New-Object System.Drawing.Icon($filePath, 256, 256)
            $bmp = $ico.ToBitmap()
            $ms = New-Object IO.MemoryStream
            $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
            $res = [Convert]::ToBase64String($ms.ToArray())
            $ms.Dispose(); $bmp.Dispose(); $ico.Dispose()
            return $res
        } catch {}

        try {
            $bmp = [System.Drawing.Bitmap]::FromFile($filePath)
            $ms = New-Object IO.MemoryStream
            $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
            $res = [Convert]::ToBase64String($ms.ToArray())
            $ms.Dispose(); $bmp.Dispose()
            return $res
        } catch {}
    }

    # Two routes disagree for some apps. The shell returns the app's real 256px
    # frame, which a few ship with the artwork drawn tiny in the middle (Sandboxie's
    # SandMan.exe), while PrivateExtractIcons upscales a full-bleed frame. So try
    # the route that usually wins for this kind, and take the other one as well
    # when the first does not fill the frame: a small glyph in a big square reads
    # as a minimised tile next to the other icons.
    $order = if ($filePath -match '\.(exe|dll|ocx|scr|cpl|msi|sys|com)$') { @('big', 'shell') } else { @('shell', 'big') }
    $best = $null
    $bestSpan = -1
    foreach ($route in $order) {
        $cand = if ($route -eq 'big') { Get-BigIconB64 $filePath } else { Get-ShellImageB64 $filePath }
        if ([string]::IsNullOrEmpty($cand)) { continue }
        $span = [IconFetch]::ArtSpanPercent($cand)
        if ($span -ge 60) { return $cand }
        if ($span -gt $bestSpan) { $bestSpan = $span; $best = $cand }
    }
    if (-not [string]::IsNullOrEmpty($best)) { return $best }

    # no icon inside the file itself: ask the shell which icon the file *type*
    # uses and pull that one at 256px. Explorer's per-user choice comes first:
    # the machine-wide ProgID often has no DefaultIcon (that is why .pdf fell
    # back to a 32x32 icon even though the shell shows a crisp one).
    try {
        $ext = [System.IO.Path]::GetExtension($filePath)
        if (-not [string]::IsNullOrEmpty($ext)) {
            $candidates = New-Object System.Collections.ArrayList
            $choice = (Get-ItemProperty -LiteralPath ('Registry::HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\' + $ext + '\UserChoice') -ErrorAction SilentlyContinue).ProgId
            if (-not [string]::IsNullOrEmpty($choice)) { [void]$candidates.Add($choice) }
            $progDefault = (Get-ItemProperty -LiteralPath ('Registry::HKEY_CLASSES_ROOT\' + $ext) -ErrorAction SilentlyContinue).'(default)'
            if (-not [string]::IsNullOrEmpty($progDefault)) { [void]$candidates.Add($progDefault) }
            foreach ($cand in $candidates) {
                $specs = New-Object System.Collections.ArrayList
                $defIcon = (Get-ItemProperty -LiteralPath ('Registry::HKEY_CLASSES_ROOT\' + $cand + '\DefaultIcon') -ErrorAction SilentlyContinue).'(default)'
                if (-not [string]::IsNullOrEmpty($defIcon) -and $defIcon -notmatch '^@\{') { [void]$specs.Add($defIcon) }
                if ($cand -match '^Applications\(.+\.exe)$') { [void]$specs.Add($matches[1]) }
                elseif ($cand -match '\.exe$') { [void]$specs.Add($cand) }
                foreach ($spec in $specs) {
                    $parts = $spec -split ','
                    $iconPath = [System.Environment]::ExpandEnvironmentVariables($parts[0].Trim('"', ' '))
                    $iconIdx = 0
                    if ($parts.Count -gt 1) { [void][int]::TryParse($parts[1].Trim(), [ref]$iconIdx) }
                    if (Test-Path -LiteralPath $iconPath) {
                        $typed = Get-BigIconB64($iconPath + ',' + $iconIdx)
                        if ([string]::IsNullOrEmpty($typed)) { $typed = Get-BigIconB64 $iconPath }
                        if (-not [string]::IsNullOrEmpty($typed)) { return $typed }
                    }
                }
            }
        }
    } catch {}

    try {
        $ico = [System.Drawing.Icon]::ExtractAssociatedIcon($filePath)
        if ($null -ne $ico) {
            $ms = New-Object IO.MemoryStream
            $bmp = $ico.ToBitmap()
            $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
            $res = [Convert]::ToBase64String($ms.ToArray())
            $ms.Dispose(); $bmp.Dispose(); $ico.Dispose()
            return $res
        }
    } catch {}

    return $null
}

$sh = $null
while ($true) {
    $p = $reader.ReadLine()
    if ($null -eq $p) { break }
    $p = $p.Trim()
    if ([string]::IsNullOrEmpty($p) -or -not (Test-Path -LiteralPath $p)) { continue }
    $b64 = $null

    if ($p.EndsWith('.url', [StringComparison]::OrdinalIgnoreCase)) {
        try {
            $txt = [System.IO.File]::ReadAllText($p)
            if ($txt -match 'IconFile=([^\r\n]+)') {
                $icoPath = $matches[1].Trim()
                $b64 = Get-IconB64 $icoPath
            }
        } catch {}
    }

    if ([string]::IsNullOrEmpty($b64) -and $p.EndsWith('.lnk', [StringComparison]::OrdinalIgnoreCase)) {
        try {
            if ($null -eq $sh) { $sh = New-Object -ComObject WScript.Shell }
            $sc = $sh.CreateShortcut($p)
            if (-not [string]::IsNullOrEmpty($sc.IconLocation)) {
                $loc = ($sc.IconLocation -split ',')[0].Trim()
                if (-not [string]::IsNullOrEmpty($loc) -and [System.IO.File]::Exists($loc)) {
                    $b64 = Get-IconB64 $loc
                }
            }
            if ([string]::IsNullOrEmpty($b64) -and -not [string]::IsNullOrEmpty($sc.TargetPath)) {
                if ([System.IO.File]::Exists($sc.TargetPath)) {
                    $b64 = Get-IconB64 $sc.TargetPath
                }
            }
        } catch {}
    }

    if ([string]::IsNullOrEmpty($b64)) {
        $b64 = Get-IconB64 $p
    }

    if (-not [string]::IsNullOrEmpty($b64)) {
        $writer.WriteLine($p + '|' + $b64)
    } else {
        $writer.WriteLine($p + '|none')
    }
}
$writer.Flush()
"#;

        if let Ok(mut child) = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .creation_flags(NO_WINDOW)
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                for p in &still_needed {
                    let _ = writeln!(stdin, "{}", p);
                }
            }
            if let Ok(output) = child.wait_with_output() {
                if output.status.success() {
                    let raw = String::from_utf8_lossy(&output.stdout);
                    for line in raw.lines() {
                        let trimmed = line.trim();
                        if let Some((p, b64)) = trimmed.split_once('|') {
                            let val = if b64 == "none" {
                                "none".to_string()
                            } else {
                                format!("data:image/png;base64,{b64}")
                            };
                            results.insert(p.to_string(), val.clone());
                            cache_icon(p, &val);
                        }
                    }
                }
            }
        }
    }

    results
}

fn resolve_icon_data_url(lnk_path: &str) -> Option<String> {
    // resolve_icons_batch consults ICON_CACHE (with its file stamp) for us.
    let map = resolve_icons_batch(&[lnk_path.to_string()], false);
    let res = map.get(lnk_path)?;
    if res == "none" {
        None
    } else {
        Some(res.clone())
    }
}

#[tauri::command]
async fn floaty_icon(id: String, app: AppHandle) -> Result<String, String> {
    // serve cached icon only if it's already high resolution
    {
        let state = app.state::<AppState>();
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        if let Some(r) = guard.widgets.get(&id) {
            if let Some(s) = r.data.get("icon").and_then(|v| v.as_str()) {
                // Serve whatever we already have: only a *missing* icon is worth
                // another PowerShell round-trip (size-based "low-res" is the
                // background upgrade pass's business, not every mount's).
                if !icon_is_missing(s) {
                    return Ok(s.to_string());
                }
            }
        }
    }
    let target = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        guard
            .widgets
            .get(&id)
            .filter(|r| is_path_kind(&r.kind))
            .and_then(|r| {
                r.data
                    .get("target")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .ok_or("launcher not found")?
    };
    if target.trim().is_empty() {
        return Err("launcher has no target".into());
    }
    let target_clone = target.clone();
    let data_url = tauri::async_runtime::spawn_blocking(move || resolve_icon_data_url(&target_clone))
        .await
        .map_err(|e| e.to_string())?
        .unwrap_or_else(|| "none".to_string());
    {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        if let Some(r) = guard.widgets.get_mut(&id) {
            // a re-resolution must never downgrade an icon that is already good:
            // remounts used to overwrite crisp 256px icons with 32px ones
            let existing = r
                .data
                .get("icon")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let keep_existing = !existing.is_empty()
                && existing != "none"
                && !is_low_res_icon(&existing)
                && is_low_res_icon(&data_url);
            if !keep_existing {
                if let Some(obj) = r.data.as_object_mut() {
                    obj.insert(
                        "icon".to_string(),
                        serde_json::Value::String(data_url.clone()),
                    );
                }
            }
        }
    }
    persist(&app);
    Ok(data_url)
}

/// Background task that automatically detects any legacy 32x32 icons in saved app launchers
/// or folders and upgrades them to crisp native high-res icons (up to 256x256).
async fn upgrade_low_res_icons(app: &AppHandle) {
    // A pipeline bump invalidates every stored icon once. Without this, icons an
    // older resolver produced (black-background squares, 32px blanks) look
    // "good enough" to the size test and would never be replaced.
    let migrate = load_settings(app).icon_pipeline < ICON_PIPELINE;

    let to_upgrade: Vec<(String, String)> = {
        let state = app.state::<AppState>();
        let Ok(guard) = state.0.lock() else { return };
        guard
            .widgets
            .iter()
            .filter(|(_, r)| is_path_kind(&r.kind))
            .filter_map(|(id, r)| {
                let target = r.data.get("target")?.as_str()?.to_string();
                let icon = r.data.get("icon").and_then(|v| v.as_str()).unwrap_or("");
                if (migrate || is_low_res_icon(icon)) && !target.trim().is_empty() {
                    Some((id.clone(), target))
                } else {
                    None
                }
            })
            .collect()
    };

    let folders_to_check: Vec<(String, Vec<String>)> = {
        let state = app.state::<AppState>();
        let Ok(guard) = state.0.lock() else { return };
        guard
            .widgets
            .iter()
            .filter(|(_, r)| r.kind == "folder")
            .filter_map(|(id, r)| {
                let needed: Vec<String> = folder_items(r)
                    .into_iter()
                    .filter(|it| {
                        // directories included: the shell has a jumbo folder glyph,
                        // and skipping them left every subfolder blank
                        (migrate || is_low_res_icon(&it.icon)) && !it.target.trim().is_empty()
                    })
                    .map(|it| it.target)
                    .collect();
                if needed.is_empty() {
                    None
                } else {
                    Some((id.clone(), needed))
                }
            })
            .collect()
    };

    let stamp_pipeline = || {
        let mut settings = load_settings(app);
        settings.icon_pipeline = ICON_PIPELINE;
        if let Ok(json) = serde_json::to_string_pretty(&settings) {
            write_text_atomic(&settings_file(app), &json);
        }
        log_line(app, &format!("icon pipeline: stored icons re-resolved to v{ICON_PIPELINE}"));
    };

    if to_upgrade.is_empty() && folders_to_check.is_empty() {
        if migrate {
            stamp_pipeline();
        }
        return;
    }

    let mut all_paths: Vec<String> = to_upgrade.iter().map(|(_, t)| t.clone()).collect();
    for (_, needed) in &folders_to_check {
        all_paths.extend(needed.iter().cloned());
    }
    all_paths.sort();
    all_paths.dedup();

    log_line(app, &format!("upgrade_low_res_icons: batch resolving {} icons", all_paths.len()));

    let icon_map =
        tauri::async_runtime::spawn_blocking(move || resolve_icons_batch(&all_paths, true))
            .await
            .unwrap_or_default();

    let mut changed = false;
    {
        let state = app.state::<AppState>();
        let Ok(mut guard) = state.0.lock() else { return };
        for (id, target) in &to_upgrade {
            if let Some(hi_res) = icon_map.get(target) {
                if hi_res != "none" {
                    if let Some(r) = guard.widgets.get_mut(id) {
                        let cur = r.data.get("icon").and_then(|v| v.as_str()).unwrap_or("");
                        if cur != hi_res {
                            if let Some(obj) = r.data.as_object_mut() {
                                obj.insert("icon".to_string(), serde_json::Value::String(hi_res.clone()));
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
        for (folder_id, needed) in &folders_to_check {
            if let Some(r) = guard.widgets.get_mut(folder_id) {
                let mut items = folder_items(r);
                let mut folder_changed = false;
                for target in needed {
                    // matched by target, not by index: the resolve can take a
                    // second, and the user can add or remove items meanwhile
                    let Some(hi_res) = icon_map.get(target) else { continue };
                    if hi_res == "none" {
                        continue;
                    }
                    for it in items.iter_mut() {
                        if it.target == *target && it.icon != *hi_res {
                            it.icon = hi_res.clone();
                            folder_changed = true;
                            changed = true;
                        }
                    }
                }
                if folder_changed {
                    set_folder_items(r, &items);
                }
            }
        }
    }

    if changed {
        persist(app);
        for (id, _) in &to_upgrade {
            app.emit("floaty-icon-refreshed", id).ok();
        }
        for (folder_id, _) in &folders_to_check {
            app.emit("floaty-folder-changed", folder_id).ok();
        }
        log_line(app, "upgrade_low_res_icons: batch upgrade complete");
    }

    if migrate {
        stamp_pipeline();
    }
}

#[tauri::command]
async fn floaty_resolve_folder_icons(folder_id: String, app: AppHandle) -> Result<(), String> {
    let needed: Vec<String> = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        let rec = guard.widgets.get(&folder_id).ok_or("folder not found")?;
        let items = folder_items(rec);
        items
            .into_iter()
            // directories included: skipping them left every subfolder inside a
            // folder widget blank (only the startup pass ever filled those in)
            .filter(|it| icon_is_missing(&it.icon) && !it.target.trim().is_empty())
            .map(|it| it.target)
            .collect()
    };

    if needed.is_empty() {
        return Ok(());
    }

    let icon_map =
        tauri::async_runtime::spawn_blocking(move || resolve_icons_batch(&needed, false))
            .await
            .map_err(|e| e.to_string())?;

    let changed = {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        if let Some(rec) = guard.widgets.get_mut(&folder_id) {
            let mut items = folder_items(rec);
            let mut modified = false;
            for it in &mut items {
                if let Some(icon) = icon_map.get(&it.target) {
                    if icon != "none" && it.icon != *icon {
                        it.icon = icon.clone();
                        modified = true;
                    }
                }
            }
            if modified {
                set_folder_items(rec, &items);
                true
            } else {
                false
            }
        } else {
            false
        }
    };

    if changed {
        persist(&app);
        app.emit("floaty-folder-changed", &folder_id).ok();
    }

    Ok(())
}

// ---------- plugins ----------

fn set_plugin_enabled(app: &AppHandle, id: &str, enabled: bool) {
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
fn floaty_plugins(app: AppHandle) -> Vec<PluginInfo> {
    plugins::manifest(&load_settings(&app).disabled)
}

/// Where user plugins live: one folder each, `plugin.json` + its module.
fn plugins_dir(app: &AppHandle) -> std::path::PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join(plugins::PLUGINS_DIR_NAME)
}

/// The plugins folder (created if missing) so the settings window can offer
/// "install by dropping a folder in here".
#[tauri::command]
fn floaty_plugins_dir(app: AppHandle) -> Result<String, String> {
    let dir = plugins_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.to_string_lossy().to_string())
}

/// Show that folder in File Explorer.
#[tauri::command]
fn floaty_open_plugins_dir(app: AppHandle) -> Result<(), String> {
    let dir = plugins_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    shell_ops::reveal(&dir.to_string_lossy())
}

/// Re-scan the plugins folder without restarting: what authors do after editing
/// a manifest. Returns the rejections so the settings window can show them.
#[tauri::command]
fn floaty_rescan_plugins(app: AppHandle) -> Result<Vec<String>, String> {
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
    // Both windows cache the plugin list and the loaded modules, so pick the new
    // ones up by reloading them: a plugin author should not need a restart.
    for label in ["desktop-overlay", "settings"] {
        if let Some(w) = app.get_webview_window(label) {
            let _ = w.eval("location.reload()");
        }
    }
    app.emit("floaty-plugins-changed", &plugins::manifest(&load_settings(&app).disabled))
        .ok();
    Ok(rejected)
}

/// Show (enable) or hide (disable) every widget of one kind, in whichever mode
/// is running. Spawning windows here while the overlay is up was what put a
/// second copy of every file icon on the desktop.
fn apply_plugin_visibility(app: &AppHandle, kind: &str, enabled: bool) {
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
fn floaty_set_plugin_enabled(id: String, enabled: bool, app: AppHandle) {
    log_line(&app, &format!("plugin {id} enabled={enabled}"));
    set_plugin_enabled(&app, &id, enabled);
    apply_plugin_visibility(&app, &id, enabled);
}

#[tauri::command]
fn floaty_add_launcher(name: String, path: String, app: AppHandle) -> Result<WidgetRecord, String> {
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

// ---------- folders ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FolderItem {
    name: String,
    target: String,
    #[serde(default)]
    icon: String,
    #[serde(default)]
    is_dir: bool,
}

fn folder_items(rec: &WidgetRecord) -> Vec<FolderItem> {
    rec.data
        .get("items")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

fn set_folder_items(rec: &mut WidgetRecord, items: &[FolderItem]) {
    if let Some(obj) = rec.data.as_object_mut() {
        obj.insert(
            "items".to_string(),
            serde_json::to_value(items).unwrap_or(serde_json::Value::Null),
        );
    }
}

fn widget_as_folder_item(rec: &WidgetRecord) -> Option<FolderItem> {
    if is_path_kind(&rec.kind) {
        Some(FolderItem {
            name: rec
                .data
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("app")
                .to_string(),
            target: rec
                .data
                .get("target")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            icon: rec
                .data
                .get("icon")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            is_dir: false,
        })
    } else if rec.kind == "folder" {
        let path = rec.data.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if path.is_empty() {
            return None;
        }
        Some(FolderItem {
            name: rec
                .data
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Folder")
                .to_string(),
            target: path.to_string(),
            icon: String::new(),
            is_dir: true,
        })
    } else {
        None
    }
}

fn unique_dest_path(dest_dir: &std::path::Path, file_name: &std::ffi::OsStr) -> std::path::PathBuf {
    let original = dest_dir.join(file_name);
    if !original.exists() {
        return original;
    }
    let p_name = std::path::Path::new(file_name);
    let stem = p_name.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = p_name.extension().and_then(|s| s.to_str());
    for i in 1..1000 {
        let candidate_name = match ext {
            Some(e) => format!("{} ({}).{}", stem, i, e),
            None => format!("{} ({})", stem, i),
        };
        let candidate = dest_dir.join(candidate_name);
        if !candidate.exists() {
            return candidate;
        }
    }
    original
}

// ---------- putting dragged items on disk ----------

/// The name Windows itself gives a folder made by hand: the shell takes the
/// first free one, so a second folder is "New folder (2)".
const NEW_FOLDER_BASE: &str = "New folder";

/// The next free "New folder (n)" in `dir`, without creating it.
fn new_folder_path(dir: &std::path::Path) -> std::path::PathBuf {
    let first = dir.join(NEW_FOLDER_BASE);
    if !first.exists() {
        return first;
    }
    for i in 2..1000 {
        let candidate = dir.join(format!("{NEW_FOLDER_BASE} ({i})"));
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

/// Create the next free "New folder (n)". The create is what reserves the name:
/// a name Explorer (or a second drag) takes in between is retried rather than
/// written over.
fn create_new_folder(dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    for _ in 0..64 {
        let candidate = new_folder_path(dir);
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("could not create a new folder".into())
}

/// The pointed root: the directory the floaties mirror. `None` when nothing is
/// pointed at (or the pointer went stale), which is the callers' cue to keep
/// their no-directory behaviour instead of inventing a location.
fn files_root_dir(app: &AppHandle) -> Option<std::path::PathBuf> {
    let root = load_settings(app).files_root;
    let p = std::path::PathBuf::from(root.trim());
    if p.is_dir() {
        Some(p)
    } else {
        None
    }
}

/// A dragged item may only be *moved* into a folder when the root owns it.
/// Anything from outside — an app icon pointing into Program Files or at a Start
/// Menu shortcut — is carried in as a shortcut file instead: a plain move there
/// would take the program out of its install directory.
fn can_move_into_folder(root: Option<&std::path::Path>, src: &std::path::Path) -> bool {
    match root {
        Some(r) => src.starts_with(r),
        None => false,
    }
}

/// A usable file name for the shortcut built from an item's label.
fn shortcut_file_name(name: &str) -> String {
    let stem: String = name
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        })
        .collect();
    let stem = stem.trim().trim_end_matches('.').trim();
    let stem = if stem.is_empty() { "Shortcut" } else { stem };
    if stem.to_ascii_lowercase().ends_with(".lnk") {
        stem.to_string()
    } else {
        format!("{stem}.lnk")
    }
}

/// Whether a path already *is* a shortcut, in which case the file can simply be
/// copied: that keeps its arguments, working directory and icon exactly.
fn is_lnk_path(p: &std::path::Path) -> bool {
    p.extension().map(|e| e.eq_ignore_ascii_case("lnk")).unwrap_or(false)
}

/// Write a Windows shortcut with the shell's own COM interface — the only
/// supported way to author a .lnk; hand-rolled byte layouts come back broken.
fn create_lnk(lnk: &std::path::Path, target: &std::path::Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const NO_WINDOW: u32 = 0x08000000;
        // single-quoted PowerShell literals; an inner quote doubles
        let q = |s: String| s.replace('\'', "''");
        let script = format!(
            "$sh = New-Object -ComObject WScript.Shell; $sc = $sh.CreateShortcut('{}'); $sc.TargetPath = '{}'; $sc.Save()",
            q(lnk.to_string_lossy().to_string()),
            q(target.to_string_lossy().to_string()),
        );
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(NO_WINDOW)
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        if !lnk.exists() {
            return Err("the shortcut was not created".into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (lnk, target);
        Err("shortcuts are windows-only".into())
    }
}

/// Put a dragged item inside `dest_dir` on disk: the file itself when the root
/// owns it, otherwise a shortcut file. Returns the item rewritten to its new
/// home plus the line to log, or the reason it stayed put.
///
/// No app handle: the disk effect is the whole contract, and this is what the
/// tests drive.
fn place_item_in_dir(
    dest_dir: &std::path::Path,
    item: &FolderItem,
    root: Option<&std::path::Path>,
) -> Result<(FolderItem, String), String> {
    let src = std::path::PathBuf::from(&item.target);
    if !src.exists() {
        return Err(format!("{} is gone; left alone", src.display()));
    }
    let mut out = item.clone();

    if can_move_into_folder(root, &src) {
        let file_name = src
            .file_name()
            .ok_or_else(|| format!("{} has no file name", src.display()))?
            .to_os_string();
        let dest = unique_dest_path(dest_dir, &file_name);
        if dest != src {
            fs::rename(&src, &dest).map_err(|e| format!("move {} FAILED: {e}", src.display()))?;
        }
        out.target = dest.to_string_lossy().to_string();
        out.name = dest
            .file_name()
            .unwrap_or(&file_name)
            .to_string_lossy()
            .to_string();
        out.is_dir = dest.is_dir();
        return Ok((
            out,
            format!("moved into folder: {} -> {}", src.display(), dest.display()),
        ));
    }

    // outside the root: the original stays where it is, a shortcut moves in
    let file_name = if is_lnk_path(&src) {
        src.file_name()
            .ok_or_else(|| format!("{} has no file name", src.display()))?
            .to_os_string()
    } else {
        std::ffi::OsString::from(shortcut_file_name(&item.name))
    };
    let dest = unique_dest_path(dest_dir, &file_name);
    let made: Result<(), String> = if is_lnk_path(&src) {
        fs::copy(&src, &dest).map(|_| ()).map_err(|e| e.to_string())
    } else {
        create_lnk(&dest, &src)
    };
    made.map_err(|e| format!("shortcut for {} FAILED: {e}", src.display()))?;
    out.target = dest.to_string_lossy().to_string();
    out.name = dest
        .file_name()
        .unwrap_or(&file_name)
        .to_string_lossy()
        .to_string();
    out.is_dir = false;
    Ok((
        out,
        format!("shortcut in folder: {} -> {}", src.display(), dest.display()),
    ))
}

/// Write the app's shortcut into the root: a copy when the app is already
/// floated through one (that keeps its arguments, working directory and icon),
/// otherwise a fresh .lnk pointing at the program. Returns the launcher's new
/// name and path plus the line to log.
fn shortcut_into_root(
    root: &std::path::Path,
    name: &str,
    path: &str,
) -> Result<(String, String, String), String> {
    let src = std::path::Path::new(path);
    let file_name = src
        .file_name()
        .ok_or_else(|| format!("{path} has no file name"))?;
    let file_name = if is_lnk_path(src) {
        file_name.to_os_string()
    } else {
        std::ffi::OsString::from(shortcut_file_name(name))
    };
    let dest = unique_dest_path(root, &file_name);
    let made: Result<(), String> = if is_lnk_path(src) {
        fs::copy(src, &dest).map(|_| ()).map_err(|e| e.to_string())
    } else {
        create_lnk(&dest, src)
    };
    made.map_err(|e| format!("app shortcut for {} FAILED: {e}", src.display()))?;
    let nm = dest
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string());
    let log = format!("app shortcut in root: {} -> {}", src.display(), dest.display());
    Ok((nm, dest.to_string_lossy().to_string(), log))
}

/// Materialise an app launcher as a shortcut file in the pointed root, so app
/// icons are files there like everything else (and grouping can move them).
/// Falls back to pointing straight at the app when there is no root.
fn app_into_root(app: &AppHandle, name: &str, path: &str) -> (String, String) {
    let unchanged = || (name.to_string(), path.to_string());
    let Some(root) = files_root_dir(app) else {
        return unchanged();
    };
    let src = std::path::Path::new(path);
    if src.starts_with(&root) || !src.exists() {
        return unchanged();
    }
    match shortcut_into_root(&root, name, path) {
        Ok((nm, dest, log)) => {
            log_line(app, &log);
            (nm, dest)
        }
        Err(msg) => {
            log_line(app, &msg);
            unchanged()
        }
    }
}

/// Keep the icons already resolved for the same paths. A rescan returns items
/// with empty icons, and dropping the stored ones (say, because they are only
/// 32px) is what made folder tiles flap between an icon and a blank tile on
/// every launch: most file types and every directory resolve to 32px.
fn carry_icons_by_target(old: &[FolderItem], items: &mut [FolderItem]) {
    for item in items.iter_mut() {
        if let Some(prev) = old.iter().find(|o| o.target == item.target) {
            if !prev.icon.is_empty() && prev.icon != "none" {
                item.icon = prev.icon.clone();
            }
        }
    }
}

/// Same, matched by name: for a rescan after the folder itself was renamed on
/// disk, where every target path moved but the items are the same files.
fn carry_icons_by_name(old: &[FolderItem], items: &mut [FolderItem]) {
    for item in items.iter_mut() {
        if let Some(prev) = old.iter().find(|o| o.name == item.name) {
            if !prev.icon.is_empty() && prev.icon != "none" {
                item.icon = prev.icon.clone();
            }
        }
    }
}

fn scan_folder_items(dir_path: &std::path::Path) -> Vec<FolderItem> {
    if !dir_path.is_dir() {
        return Vec::new();
    }
    let mut items = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();
            if file_name.starts_with('.')
                || file_name.eq_ignore_ascii_case("desktop.ini")
                || file_name.eq_ignore_ascii_case("thumbs.db")
            {
                continue;
            }
            let is_dir = path.is_dir();
            let target = path.to_string_lossy().to_string();
            let icon = String::new();
            items.push(FolderItem {
                name: file_name,
                target,
                icon,
                is_dir,
            });
        }
    }
    items.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            b.is_dir.cmp(&a.is_dir)
        } else {
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        }
    });
    items
}

/// Close a widget window off the command thread (same reason as creation).
fn close_widget_async(app: &AppHandle, id: &str) {
    if app.get_webview_window("desktop-overlay").is_some() {
        // the overlay draws it: tell the overlay to drop it
        app.emit("floaty-widget-removed", id).ok();
    }
    // A widget can have a window of its own *in overlay mode too* — a pinned one
    // does — and that window has to come down for real: leaving it up left a
    // second copy of the widget on the desktop (and the overlay was asked to
    // drop a widget it was not drawing).
    if app.get_webview_window(&widget_label(id)).is_none() {
        return;
    }
    let handle = app.clone();
    let owned = id.to_string();
    std::thread::spawn(move || {
        let h2 = handle.clone();
        let id2 = owned.clone();
        if let Err(e) = handle.run_on_main_thread(move || {
            if let Some(w) = h2.get_webview_window(&widget_label(&id2)) {
                if let Err(e) = w.close() {
                    log_line(&h2, &format!("close window FAILED for {id2}: {e}"));
                }
            }
        }) {
            log_line(&handle, &format!("main-thread dispatch FAILED for {owned}: {e}"));
        }
    });
}

fn launch_target(app: &AppHandle, target: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW avoids a console flash; `start` resolves exe/lnk/urls
        const NO_WINDOW: u32 = 0x08000000;
        const DETACHED: u32 = 0x00000008;
        std::process::Command::new("cmd")
            .args(["/C", "start", "", target])
            .creation_flags(NO_WINDOW | DETACHED)
            .spawn()
            .map_err(|e| {
                log_line(app, &format!("launch FAILED: {e}"));
                e.to_string()
            })?;
    }
    Ok(())
}

#[tauri::command]
fn floaty_launch(id: String, app: AppHandle) -> Result<(), String> {
    let target = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        guard
            .widgets
            .get(&id)
            .filter(|r| is_path_kind(&r.kind))
            .and_then(|r| {
                r.data
                    .get("target")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .ok_or("launcher not found")?
    };
    if target.trim().is_empty() {
        return Err("launcher has no target".into());
    }
    log_line(&app, &format!("launch {id} -> {target}"));
    launch_target(&app, &target)
}

#[tauri::command]
fn floaty_launch_target(target: String, app: AppHandle) -> Result<(), String> {
    if target.trim().is_empty() {
        return Err("empty target".into());
    }
    log_line(&app, "launch target from folder");
    launch_target(&app, &target)
}

/// Called (fire-and-forget) when an app icon is dropped after a manual drag.
/// If the drop point lands on another icon/folder window, merge them.
/// Returns the folder id when a merge happened.
#[tauri::command]
fn floaty_dropped(id: String, x: Option<i32>, y: Option<i32>, app: AppHandle) -> Option<String> {
    // live rect of the dropped icon
    let (cx, cy) = if let (Some(px), Some(py)) = (x, y) {
        let (w, h) = {
            let state = app.state::<AppState>();
            let guard = state.0.lock().ok()?;
            let rec = guard.widgets.get(&id)?;
            plugins::size(&rec.kind, &rec.data)
        };
        (px + w as i32 / 2, py + h as i32 / 2)
    } else if let Some(me) = app.get_webview_window(&widget_label(&id)) {
        let (mp, ms) = (me.outer_position().ok()?, me.inner_size().ok()?);
        (mp.x + ms.width as i32 / 2, mp.y + ms.height as i32 / 2)
    } else {
        return None;
    };

    // candidate targets (snapshot under the lock, probe windows after release)
    let cands: Vec<(String, String)> = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().ok()?;
        guard
            .widgets
            .iter()
            .filter(|(oid, r)| *oid != &id && plugins::is_desktop_item(&r.kind))
            .map(|(oid, r)| (oid.clone(), r.kind.clone()))
            .collect()
    };
    let is_overlay = app.get_webview_window("desktop-overlay").is_some();
    let mut hit: Option<(String, String)> = None;
    for (oid, kind) in cands {
        let (hx, hy, ww, hh) = if is_overlay {
            let state = app.state::<AppState>();
            let guard = state.0.lock().ok()?;
            if let Some(r) = guard.widgets.get(&oid) {
                let (w, h) = plugins::size(&r.kind, &r.data);
                (r.x - 12, r.y - 12, w as i32 + 24, h as i32 + 24)
            } else {
                continue;
            }
        } else if let Some(w) = app.get_webview_window(&widget_label(&oid)) {
            if let (Ok(p), Ok(s)) = (w.outer_position(), w.inner_size()) {
                (p.x - 12, p.y - 12, s.width as i32 + 24, s.height as i32 + 24)
            } else {
                continue;
            }
        } else {
            continue;
        };
        if cx >= hx && cx < hx + ww && cy >= hy && cy < hy + hh {
            hit = Some((oid, kind));
            break;
        }
    }
    let (target_id, target_kind) = hit?;

    // snapshot the dragged item, then remove it (tombstone blocks its late
    // saves from resurrecting it)
    let mut dragged = {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().ok()?;
        let rec = guard.widgets.get(&id)?;
        let item = widget_as_folder_item(rec)?;
        if item.target.trim().is_empty() {
            return None;
        }
        guard.widgets.remove(&id);
        guard.dead.insert(id.clone());
        item
    };
    persist(&app);
    close_widget_async(&app, &id);

    // the pointed root, read once — outside the widget lock (it comes from disk)
    let root = files_root_dir(&app);

    if target_kind == "folder" {
        {
            let state = app.state::<AppState>();
            let mut guard = state.0.lock().ok()?;
            let rec = guard.widgets.get_mut(&target_id)?;
            let folder_path = rec.data.get("path").and_then(|v| v.as_str()).map(|s| s.to_string());

            // A folder backed by a directory on disk takes the item into that
            // directory: the file itself when the root owns it, a shortcut file
            // for anything from outside (an app in Program Files, say — moving
            // that one would take the program out of its install directory).
            if let Some(ref fpath) = folder_path {
                let dest_dir = std::path::Path::new(fpath);
                if dest_dir.is_dir() {
                    // bound to a local first: the borrow of `dragged` must end
                    // before it is rebound
                    let placed = match place_item_in_dir(dest_dir, &dragged, root.as_deref()) {
                        Ok((item, log)) => {
                            log_line(&app, &log);
                            Some(item)
                        }
                        Err(log) => {
                            log_line(&app, &log);
                            None
                        }
                    };
                    if let Some(placed) = placed {
                        dragged = placed;
                    }
                }
            }

            let mut items = folder_items(rec);
            if !items.iter().any(|it| it.target == dragged.target) {
                if dragged.icon.is_empty() && !dragged.is_dir {
                    dragged.icon = resolve_icon_data_url(&dragged.target).unwrap_or_default();
                }
                items.push(dragged);
                set_folder_items(rec, &items);
            }
        }
        persist(&app);
        app.emit("floaty-folder-changed", &target_id).ok();
        log_line(&app, &format!("merged {id} into folder {target_id}"));
        return Some(target_id);
    }

    // target is an app icon: the two land in a *new folder in the pointed root*,
    // named the way Windows names one. The dragged files move into it; an app
    // icon, whose program lives outside the root, is carried in as a shortcut
    // file instead of being moved out of its install directory.
    let titem = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().ok()?;
        let trec = guard.widgets.get(&target_id)?;
        widget_as_folder_item(trec)?
    };

    if let Some(dir) = root.as_deref().and_then(|r| match create_new_folder(r) {
        Ok(d) => Some(d),
        Err(e) => {
            log_line(&app, &format!("new folder FAILED in {}: {e}", r.display()));
            None
        }
    }) {
        let mut placed: Vec<FolderItem> = Vec::new();
        for it in [&titem, &dragged] {
            match place_item_in_dir(&dir, it, root.as_deref()) {
                Ok((item, log)) => {
                    log_line(&app, &log);
                    placed.push(item);
                }
                Err(log) => log_line(&app, &log),
            }
        }
        if !placed.is_empty() {
            let (tx, ty) = {
                let state = app.state::<AppState>();
                let mut guard = state.0.lock().ok()?;
                let trec = guard.widgets.remove(&target_id)?;
                guard.dead.insert(target_id.clone());
                (trec.x, trec.y)
            };
            persist(&app);
            close_widget_async(&app, &target_id);

            // rescan the directory: items are files again (the shortcuts are new
            // on disk), so carry over the icons already resolved for them
            let mut items = scan_folder_items(&dir);
            carry_icons_by_name(&placed, &mut items);
            let name = dir
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "Folder".to_string());
            let mut data = serde_json::json!({
                "name": name,
                "path": dir.to_string_lossy(),
                "items": items,
            });
            data["pinned"] = serde_json::Value::Bool(true);
            return match create_record_with(&app, "folder", data, Some((tx, ty))) {
                Ok(rec) => {
                    log_line(
                        &app,
                        &format!(
                            "grouped {id} + {target_id} into {} ({})",
                            rec.id,
                            dir.display()
                        ),
                    );
                    Some(rec.id)
                }
                Err(e) => {
                    log_line(&app, &format!("folder create FAILED: {e}"));
                    None
                }
            };
        }
        // nothing landed: don't leave an empty folder behind
        fs::remove_dir(&dir).ok();
    }

    // no pointed root (or the folder could not be made): fold both into an
    // in-app folder at the target's spot, as before
    let mut items = vec![titem, dragged];
    for it in items.iter_mut() {
        if it.icon.is_empty() && !it.is_dir {
            it.icon = resolve_icon_data_url(&it.target).unwrap_or_default();
        }
    }
    let (tx, ty) = {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().ok()?;
        let trec = guard.widgets.remove(&target_id)?;
        guard.dead.insert(target_id.clone());
        (trec.x, trec.y)
    };
    persist(&app);
    close_widget_async(&app, &target_id);
    let data = serde_json::json!({ "name": "Folder", "items": items });
    match create_record_with(&app, "folder", data, Some((tx, ty))) {
        Ok(rec) => {
            log_line(&app, &format!("grouped {id} + {target_id} into {}", rec.id));
            Some(rec.id)
        }
        Err(e) => {
            log_line(&app, &format!("folder create FAILED: {e}"));
            None
        }
    }
}

/// Where an item goes when it is pulled out of a folder.
///
/// Two shapes. A folder backed by a directory on disk holds its items *inside*
/// that directory, so an item comes out beside it (`<folder>/..`). A folder made
/// in-app by grouping has no directory at all — its record carries no `path` and
/// its items were never moved anywhere — so they stay exactly where they are,
/// which is what returning the item's own directory means to the caller (it skips
/// the move when the destination is not different).
///
/// That second case used to step two levels up from the item; for a desktop item
/// that is the parent of the *Desktop*, so ungrouping a folder of desktop icons
/// moved them out of the Desktop (two of the user's landed in the OneDrive root).
fn ungroup_dest_dir(
    folder_path: Option<&str>,
    src: &std::path::Path,
) -> Option<std::path::PathBuf> {
    match folder_path {
        Some(fp) => std::path::Path::new(fp).parent().map(|p| p.to_path_buf()),
        None => src.parent().map(|p| p.to_path_buf()),
    }
}

/// Pull one file/folder out of a folder: remove the item, move it out on disk
/// to the parent directory (relative path change), and float it as its own widget.
#[tauri::command]
fn floaty_ungroup(folder_id: String, index: usize, x: i32, y: i32, app: AppHandle) -> Result<WidgetRecord, String> {
    let (mut item, folder_path) = {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        let rec = guard.widgets.get_mut(&folder_id).ok_or("folder not found")?;
        if rec.kind != "folder" {
            return Err("not a folder".into());
        }
        let folder_path = rec.data.get("path").and_then(|v| v.as_str()).map(|s| s.to_string());
        let mut items = folder_items(rec);
        if index >= items.len() {
            return Err("bad index".into());
        }
        let item = items.remove(index);
        if item.target.trim().is_empty() {
            return Err("empty target".into());
        }
        set_folder_items(rec, &items);
        (item, folder_path)
    };
    persist(&app);
    app.emit("floaty-folder-changed", &folder_id).ok();

    // Move file/directory out on disk to the parent directory
    let src_path = std::path::PathBuf::from(&item.target);
    if src_path.exists() {
        let dest_dir = ungroup_dest_dir(folder_path.as_deref(), &src_path);

        if let Some(dest_dir_path) = dest_dir {
            if dest_dir_path.is_dir() && dest_dir_path != src_path.parent().unwrap_or(&src_path) {
                if let Some(file_name) = src_path.file_name() {
                    let dest_path = unique_dest_path(&dest_dir_path, file_name);
                    if dest_path != src_path {
                        if let Ok(_) = std::fs::rename(&src_path, &dest_path) {
                            log_line(&app, &format!("moved out of folder on disk: {} -> {}", src_path.display(), dest_path.display()));
                            item.target = dest_path.to_string_lossy().to_string();
                            item.name = dest_path.file_name().unwrap_or(file_name).to_string_lossy().to_string();
                            item.is_dir = dest_path.is_dir();
                        }
                    }
                }
            }
        }
    }

    // Treat as new floatie: directory -> folder floatie, launchable -> app
    // launcher floatie, anything else -> file floatie
    let target_path = std::path::Path::new(&item.target).to_path_buf();
    let is_dir = item.is_dir || target_path.is_dir();
    let rec = if is_dir {
        let sub_items = scan_folder_items(&target_path);
        let mut data = serde_json::json!({
            "name": item.name,
            "path": item.target,
            "items": sub_items,
        });
        data["pinned"] = serde_json::Value::Bool(true);
        create_record_with(&app, "folder", data, Some((x, y)))?
    } else {
        let mut data = serde_json::json!({
            "name": item.name,
            "target": item.target,
            "icon": item.icon,
        });
        data["pinned"] = serde_json::Value::Bool(true);
        create_record_with(&app, kind_for_path(&target_path, false), data, Some((x, y)))?
    };

    log_line(&app, &format!("ungrouped {} from {folder_id} as {} (kind: {})", item.name, rec.id, rec.kind));
    Ok(rec)
}

#[tauri::command]
fn floaty_rename_folder_dir(folder_id: String, new_name: String, app: AppHandle) -> Result<(), String> {
    let (old_path, new_path) = {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        let rec = guard.widgets.get_mut(&folder_id).ok_or("folder not found")?;
        let path = rec.data.get("path").and_then(|v| v.as_str()).map(|s| s.to_string());
        if let Some(old_p) = path {
            let p = std::path::Path::new(&old_p);
            if let Some(parent) = p.parent() {
                let dest = parent.join(&new_name);
                if p.exists() && dest != p {
                    std::fs::rename(p, &dest).map_err(|e| e.to_string())?;
                    let new_p_str = dest.to_string_lossy().to_string();
                    if let Some(obj) = rec.data.as_object_mut() {
                        obj.insert("path".to_string(), serde_json::Value::String(new_p_str.clone()));
                        obj.insert("name".to_string(), serde_json::Value::String(new_name));
                    }
                    // A rescan starts every item with no icon; the items are the
                    // same files under a new parent, so keep them by name (a
                    // rename used to blank every icon in the folder).
                    let old_items = folder_items(rec);
                    let mut items = scan_folder_items(&dest);
                    carry_icons_by_name(&old_items, &mut items);
                    set_folder_items(rec, &items);
                    (old_p, new_p_str)
                } else {
                    return Ok(());
                }
            } else {
                return Ok(());
            }
        } else {
            return Ok(());
        }
    };
    persist(&app);
    app.emit("floaty-folder-changed", &folder_id).ok();
    log_line(&app, &format!("renamed folder dir on disk: {old_path} -> {new_path}"));
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesSyncResult {
    pub files: usize,
    pub dirs: usize,
    pub total: usize,
    /// Whether this reconcile actually moved anything. The watcher fires it on
    /// any name change, and most of those turn out to be nothing.
    #[serde(default)]
    pub changed: bool,
}

#[tauri::command]
fn floaty_sync_files(root: Option<String>, app: AppHandle) -> Result<FilesSyncResult, String> {
    sync_root(root, app, false)
}

/// Mirror the pointed root: bring the desktop in step with what the folder
/// actually holds — add what is new, refresh what moved, and (the half that
/// makes this a mirror rather than an importer) take off the desktop what is no
/// longer there. `quiet` keeps the watcher's no-op passes out of the log.
fn sync_root(root: Option<String>, app: AppHandle, quiet: bool) -> Result<FilesSyncResult, String> {
    let started = std::time::Instant::now();
    let target_root = match root {
        Some(r) if !r.trim().is_empty() => {
            let mut s = load_settings(&app);
            s.files_root = r.trim().to_string();
            if let Ok(json) = serde_json::to_string_pretty(&s) {
                write_text_atomic(&settings_file(&app), &json);
            }
            s.files_root
        }
        _ => load_settings(&app).files_root,
    };

    if target_root.trim().is_empty() {
        // Nothing is pointed at: nothing to follow, mirror or watch.
        fs_watch::retarget(None, app.clone());
        return Ok(FilesSyncResult {
            files: 0,
            dirs: 0,
            total: 0,
            changed: false,
        });
    }

    let base_path = std::path::PathBuf::from(&target_root);
    if !base_path.is_dir() {
        return Err(format!("'{}' is not a valid directory", target_root));
    }

    // Auto-enable plugins if disabled
    let s = load_settings(&app);
    if s.disabled.iter().any(|d| d == "app") {
        set_plugin_enabled(&app, "app", true);
    }
    if s.disabled.iter().any(|d| d == "folder") {
        set_plugin_enabled(&app, "folder", true);
    }
    if s.disabled.iter().any(|d| d == "file") {
        set_plugin_enabled(&app, "file", true);
    }

    let mut files_count = 0;
    let mut dirs_count = 0;

    let (mut occupied_positions, existing_map, mut next_id) = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        let occupied: Vec<(i32, i32)> = guard.widgets.values().map(|w| (w.x, w.y)).collect();
        let mut map: HashMap<String, (String, String)> = HashMap::new();
        for (id, w) in &guard.widgets {
            if w.kind == "folder" {
                if let Some(p) = w.data.get("path").and_then(|v| v.as_str()) {
                    map.insert(p.to_string(), (id.clone(), "folder".to_string()));
                }
            } else if is_path_kind(&w.kind) {
                if let Some(p) = w.data.get("target").and_then(|v| v.as_str()) {
                    map.insert(p.to_string(), (id.clone(), w.kind.clone()));
                }
            }
        }
        (occupied, map, guard.next)
    };

    let next_pos = |occupied: &mut Vec<(i32, i32)>| -> (i32, i32) {
        let mut cur_x = 80;
        let mut cur_y = 80;
        loop {
            let collision = occupied.iter().any(|(x, y)| (*x - cur_x).abs() < 50 && (*y - cur_y).abs() < 50);
            if !collision {
                occupied.push((cur_x, cur_y));
                return (cur_x, cur_y);
            }
            cur_y += 115;
            if cur_y > 800 {
                cur_y = 80;
                cur_x += 105;
            }
        }
    };

    let mut updated_folders: Vec<(String, Vec<FolderItem>)> = Vec::new();
    let mut new_records: Vec<WidgetRecord> = Vec::new();
    // What the folder holds right now, lowercased: Windows compares paths
    // without case, so a record's stored path has to be matched the same way to
    // decide whether the thing it mirrors is still there.
    let mut on_disk: HashSet<String> = HashSet::new();
    // Only a listing that ran to the end can be read as "this is the folder":
    // a directory we could not open must never look like a directory that is not
    // there, or one permission problem would take every floatie off the desktop.
    let mut listing_complete = false;

    if let Ok(entries) = std::fs::read_dir(&base_path) {
        let mut unreadable = false;
        for entry in entries {
            let Ok(entry) = entry else {
                unreadable = true;
                continue;
            };
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();
            if file_name.starts_with('.')
                || file_name.eq_ignore_ascii_case("desktop.ini")
                || file_name.eq_ignore_ascii_case("thumbs.db")
            {
                continue;
            }
            let path_str = path.to_string_lossy().to_string();
            on_disk.insert(path_str.to_ascii_lowercase());
            if path.is_dir() {
                let mut items = scan_folder_items(&path);
                match existing_map.get(&path_str) {
                    Some((fid, kind)) if kind == "folder" => {
                        let mut refreshed = true;
                        if let Ok(guard) = app.state::<AppState>().0.lock() {
                            if let Some(w) = guard.widgets.get(fid) {
                                let old_items = folder_items(w);
                                carry_icons_by_target(&old_items, &mut items);
                                // an external rename moved every target at once:
                                // the items are the same files under new paths
                                carry_icons_by_name(&old_items, &mut items);
                                // An idle folder must not look changed: the write
                                // below is what the store persist hangs on.
                                refreshed = old_items != items;
                            }
                        }
                        if refreshed {
                            updated_folders.push((fid.clone(), items));
                        }
                    }
                    // already represented by a record of another kind: leave it
                    Some(_) => {}
                    None => {
                        let (px, py) = next_pos(&mut occupied_positions);
                        next_id += 1;
                        let id = format!("folder-{}", next_id);
                        let data = serde_json::json!({
                            "name": file_name,
                            "path": path_str,
                            "items": items,
                            "pinned": true,
                        });
                        new_records.push(WidgetRecord {
                            id,
                            kind: "folder".to_string(),
                            x: px,
                            y: py,
                            data,
                        });
                    }
                }
                dirs_count += 1;
            } else {
                if !existing_map.contains_key(&path_str) {
                    let (px, py) = next_pos(&mut occupied_positions);
                    next_id += 1;
                    // launchable entries are apps, everything else is a file
                    let kind = kind_for_path(&path, false);
                    let id = format!("{kind}-{}", next_id);
                    let mut data = serde_json::json!({
                        "name": file_name,
                        "target": path_str,
                        "icon": "",
                    });
                    data["pinned"] = serde_json::Value::Bool(true);
                    new_records.push(WidgetRecord {
                        id,
                        kind: kind.to_string(),
                        x: px,
                        y: py,
                        data,
                    });
                }
                files_count += 1;
            }
        }
        listing_complete = !unreadable;
    }

    // The other half of the mirror: a record for something that is no longer in
    // the folder comes off the desktop. Only records the folder owns are
    // eligible — a floatie pointing outside the root (an app floated from
    // Program Files, a file carried in from elsewhere) is not something this
    // folder's listing can answer for — and only when the listing itself is
    // trustworthy, because a half-read folder must not read as "gone".
    let mut vanished: Vec<String> = Vec::new();
    if !listing_complete {
        log_line(
            &app,
            &format!("synced files root '{}': listing incomplete — keeping every floatie it could not account for", target_root),
        );
    } else {
        let state = app.state::<AppState>();
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        for (id, w) in &guard.widgets {
            let Some(path) = record_path(w) else { continue };
            if !mirrors_root_entry(&target_root, &path) {
                continue;
            }
            if on_disk.contains(&path.to_ascii_lowercase()) {
                continue;
            }
            vanished.push(id.clone());
        }
    }

    let mut changed = false;
    {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        guard.next = next_id;
        for (fid, items) in &updated_folders {
            if let Some(w) = guard.widgets.get_mut(fid) {
                set_folder_items(w, items);
                changed = true;
            }
        }
        for rec in &new_records {
            guard.widgets.insert(rec.id.clone(), rec.clone());
            changed = true;
        }
        for id in &vanished {
            if guard.widgets.remove(id).is_some() {
                changed = true;
            }
        }
    }

    // A reconcile that found nothing must not rewrite the store: it is megabytes
    // with every icon inlined, and the watcher calls this on any name change —
    // including ones that turn out to be nothing, or a folder that is simply
    // idle while OneDrive touches its metadata.
    if changed {
        persist(&app);
    }

    for (fid, _) in &updated_folders {
        app.emit("floaty-folder-changed", fid).ok();
    }
    for id in &vanished {
        hide_widget(&app, id);
    }

    if !new_records.is_empty() {
        if app.get_webview_window("desktop-overlay").is_some() {
            for rec in &new_records {
                app.emit("floaty-widget-added", rec).ok();
            }
        } else {
            let app_handle = app.clone();
            let to_spawn = new_records.clone();
            std::thread::spawn(move || {
                for rec in to_spawn {
                    let h = app_handle.clone();
                    let r = rec.clone();
                    let _ = app_handle.run_on_main_thread(move || {
                        let _ = spawn_widget(&h, &r);
                    });
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
            });
        }
    }

    if changed {
        let bg_app = app.clone();
        tauri::async_runtime::spawn(async move {
            upgrade_low_res_icons(&bg_app).await;
        });
        // The settings window lists the same floaties; tell it they moved.
        app.emit("floaty-widgets-changed", ()).ok();
    }

    // From here on the folder is followed rather than read once: the watcher
    // re-runs this reconcile whenever it changes. `retarget` no-ops when this is
    // already the watched root, so every sync can call it.
    fs_watch::retarget(Some(target_root.clone()), app.clone());

    // A watcher-driven pass that found nothing is not worth a line; a manual
    // sync always reports, since someone is waiting to read it.
    if !quiet || changed {
        log_line(
            &app,
            &format!(
                "synced files root '{}': {} files, {} dirs ({} new, {} gone, {} folders refreshed) in {}ms",
                target_root,
                files_count,
                dirs_count,
                new_records.len(),
                vanished.len(),
                updated_folders.len(),
                started.elapsed().as_millis()
            ),
        );
    }
    Ok(FilesSyncResult {
        files: files_count,
        dirs: dirs_count,
        total: files_count + dirs_count,
        changed,
    })
}

/// The path a record mirrors, when its kind stands for something on disk.
fn record_path(rec: &WidgetRecord) -> Option<String> {
    let key = plugins::path_key(&rec.kind)?;
    rec.data
        .get(key.as_str())
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Whether a record mirrors one of the *root's own entries* — a file or folder
/// the scan would list.
///
/// Deliberately not "anywhere under the root": the reconcile only lists the
/// root's top level, so a floatie pointing at something deeper (a file ungrouped
/// out of a folder that lives inside the root) is not something that listing can
/// account for, and reading its absence as "gone" would take it off the desktop.
/// Windows compares paths without case, and a stored path can disagree with the
/// root about it (a shortcut written by Explorer, a root typed by hand), so the
/// comparison folds case.
fn mirrors_root_entry(root: &str, path: &str) -> bool {
    let root = root.trim_end_matches(['\\', '/']).to_ascii_lowercase();
    if root.is_empty() {
        return false;
    }
    let path = path.to_ascii_lowercase();
    match path.rfind(['\\', '/']) {
        Some(cut) => path[..cut] == root,
        None => false,
    }
}

/// Move floaties with their files.
///
/// An external rename arrives as a pair, and applying it here — rather than
/// letting the reconcile drop the old record and add a new one — is what keeps
/// the tile where the user put it, with its icon: the desktop follows the file
/// instead of shuffling around it.
fn apply_root_renames(app: &AppHandle, renames: &[(String, String)]) -> usize {
    if renames.is_empty() {
        return 0;
    }
    let mut moved: Vec<WidgetRecord> = Vec::new();
    {
        let state = app.state::<AppState>();
        let Ok(mut guard) = state.0.lock() else {
            return 0;
        };
        for (old, new) in renames {
            let from = old.to_ascii_lowercase();
            let hit = guard
                .widgets
                .iter()
                .find(|(_, w)| record_path(w).is_some_and(|p| p.to_ascii_lowercase() == from))
                .map(|(id, w)| (id.clone(), plugins::path_key(&w.kind).unwrap_or_default()));
            let Some((id, key)) = hit else { continue };
            let label = std::path::Path::new(new)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let Some(w) = guard.widgets.get_mut(&id) else {
                continue;
            };
            if let Some(obj) = w.data.as_object_mut() {
                obj.insert(key, serde_json::Value::String(new.clone()));
                // the label follows the file it points at
                if !label.is_empty() && obj.contains_key("name") {
                    obj.insert("name".to_string(), serde_json::Value::String(label));
                }
            }
            moved.push(w.clone());
        }
    }
    if moved.is_empty() {
        return 0;
    }
    persist(app);
    for rec in &moved {
        // the event `floaty_rename` sends: the overlay remounts that slot
        app.emit("floaty-widget-updated", rec).ok();
    }
    log_line(
        app,
        &format!("watch: {} floatie(s) followed a rename", moved.len()),
    );
    moved.len()
}

/// The pointed root changed on disk: bring the desktop back in step.
///
/// Runs on the watcher thread, so it does the cheap thing and lets the reconcile
/// decide what actually changed: move the floaties whose files were renamed,
/// then re-run the mirror — which is idempotent, so a redundant call costs one
/// `read_dir` and nothing else.
fn root_changed(app: &AppHandle, renames: &[(String, String)]) {
    apply_root_renames(app, renames);
    // The reconcile reports for itself when it moved something, and
    // `apply_root_renames` reports the renames; what needs saying here is only
    // when the mirror could not run at all (a root gone read-only, a drive out).
    if let Err(err) = sync_root(None, app.clone(), true) {
        log_line(app, &format!("watch: mirror failed: {err}"));
    }
}

// ---------- live2d model library ----------

#[derive(Debug, Clone, Serialize)]
struct Live2dModelEntry {
    name: String,
    path: String,
}

struct ScoredModelEntry {
    name: String,
    path: String,
    score: i32,
    parent: String,
}

fn is_generic_model_stem(stem: &str) -> bool {
    let s = stem.to_ascii_lowercase();
    matches!(s.as_str(), "model" | "index" | "model3" | "character" | "main")
}

fn extract_model_stem(file_name: &str) -> Option<&str> {
    let fl = file_name.to_ascii_lowercase();
    if fl.ends_with(".model.json") {
        Some(&file_name[..file_name.len() - ".model.json".len()])
    } else if fl.ends_with(".model3.json") {
        Some(&file_name[..file_name.len() - ".model3.json".len()])
    } else {
        None
    }
}

/// Blocking walk for *.model.json / *.model3.json up to 3 levels deep.
/// Deduplicates duplicate model files across directories and extracts clean display names.
fn scan_models_blocking(root: String) -> Vec<Live2dModelEntry> {
    let base = std::path::PathBuf::from(&root);
    if !base.is_dir() {
        return vec![];
    }
    let mut model_map: HashMap<String, ScoredModelEntry> = HashMap::new();
    let mut stack: Vec<(std::path::PathBuf, u8)> = vec![(base, 0)];

    while let Some((dir, depth)) = stack.pop() {
        if depth > 6 {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if depth < 6 {
                    let hidden = p
                        .file_name()
                        .and_then(|s| s.to_str())
                        .map(|s| s.starts_with('.'))
                        .unwrap_or(true);
                    if !hidden {
                        stack.push((p, depth + 1));
                    }
                }
                continue;
            }
            let Some(file_name) = p.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(stem) = extract_model_stem(file_name) else {
                continue;
            };

            let parent = p
                .parent()
                .and_then(|d| d.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            let is_generic = is_generic_model_stem(stem);
            let dedup_key = if is_generic {
                format!("{}/{}", parent.to_lowercase(), stem.to_lowercase())
            } else {
                stem.to_lowercase()
            };

            let mut score = 0;
            let p_lower = parent.to_lowercase();
            let s_lower = stem.to_lowercase();
            if p_lower == s_lower {
                score += 100;
            } else if p_lower.contains(&s_lower) || s_lower.contains(&p_lower) {
                score += 50;
            }

            if let Some(existing) = model_map.get(&dedup_key) {
                if existing.score >= score {
                    continue;
                }
            }

            // Extract display name, checking JSON name field if available
            let mut display_name = if is_generic {
                if parent.is_empty() {
                    stem.to_string()
                } else {
                    parent.clone()
                }
            } else {
                stem.to_string()
            };

            if let Ok(content) = fs::read_to_string(&p) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(jn) = val.get("name").and_then(|v| v.as_str()).map(|s| s.trim()) {
                        if !jn.is_empty()
                            && !jn.eq_ignore_ascii_case(stem)
                            && !is_generic_model_stem(jn)
                        {
                            if is_generic && !parent.is_empty() {
                                display_name = format!("{parent} ({jn})");
                            } else {
                                display_name = format!("{stem} ({jn})");
                            }
                        }
                    }
                }
            }

            model_map.insert(
                dedup_key,
                ScoredModelEntry {
                    name: display_name,
                    path: p.to_string_lossy().to_string(),
                    score,
                    parent,
                },
            );
        }
    }

    // Check for display name collisions and disambiguate with parent folder if needed
    let mut name_counts: HashMap<String, usize> = HashMap::new();
    for entry in model_map.values() {
        *name_counts.entry(entry.name.clone()).or_insert(0) += 1;
    }

    let mut out: Vec<Live2dModelEntry> = model_map
        .into_values()
        .map(|entry| {
            let final_name = if name_counts.get(&entry.name).copied().unwrap_or(0) > 1
                && !entry.parent.is_empty()
                && !entry.name.contains(&entry.parent)
            {
                format!("{} ({})", entry.name, entry.parent)
            } else {
                entry.name
            };
            Live2dModelEntry {
                name: final_name,
                path: entry.path,
            }
        })
        .collect();

    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.truncate(1000);
    out
}

#[tauri::command]
async fn floaty_scan_models(root: String, app: AppHandle) -> Vec<Live2dModelEntry> {
    let out = tauri::async_runtime::spawn_blocking(move || scan_models_blocking(root))
        .await
        .unwrap_or_default();
    log_line(&app, &format!("model scan found {} models", out.len()));
    out
}

#[tauri::command]
fn floaty_set_widget_model(id: String, model: String, app: AppHandle) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        let rec = guard.widgets.get_mut(&id).ok_or("widget not found")?;
        if rec.kind != "live2d" {
            return Err("not a live2d widget".into());
        }
        if let Some(obj) = rec.data.as_object_mut() {
            obj.insert("model".to_string(), serde_json::Value::String(model));
        }
    }
    persist(&app);
    app.emit("floaty-live2d-changed", &id).ok();
    log_line(&app, &format!("live2d model changed for {id}"));
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize)]
struct WindowCursorPos {
    rel_x: f64,
    rel_y: f64,
}

/// Returns the cursor position relative to the specified window's top-left corner
/// in logical pixels, tracking the cursor across the entire desktop.
#[tauri::command]
fn floaty_window_cursor_pos(label: String, app: AppHandle) -> Option<WindowCursorPos> {
    let w = app.get_webview_window(&label)?;
    let win_pos = w.outer_position().ok()?;
    let scale = w.scale_factor().unwrap_or(1.0);
    if scale <= 0.0 {
        return None;
    }

    #[cfg(windows)]
    {
        use std::mem::MaybeUninit;
        #[repr(C)]
        struct POINT {
            x: i32,
            y: i32,
        }
        extern "system" {
            fn GetCursorPos(lpPoint: *mut POINT) -> i32;
        }
        let mut pt = MaybeUninit::<POINT>::uninit();
        if unsafe { GetCursorPos(pt.as_mut_ptr()) } != 0 {
            let pt = unsafe { pt.assume_init() };
            return Some(WindowCursorPos {
                rel_x: (pt.x as f64 - win_pos.x as f64) / scale,
                rel_y: (pt.y as f64 - win_pos.y as f64) / scale,
            });
        }
    }

    let cur = app.cursor_position().ok()?;
    Some(WindowCursorPos {
        rel_x: (cur.x - win_pos.x as f64) / scale,
        rel_y: (cur.y - win_pos.y as f64) / scale,
    })
}

#[derive(Debug, Clone, Serialize)]
struct LayoutItem {
    id: String,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// Logical-px rects of all gravity widgets, for inter-icon separation.
#[tauri::command]
fn floaty_layout(app: AppHandle) -> Vec<LayoutItem> {
    let state = app.state::<AppState>();
    let guard = match state.0.lock() {
        Ok(g) => g,
        Err(_) => return vec![],
    };
    guard
        .widgets
        .values()
        // What the desktop is made of is what an icon can land on: every
        // desktop item, folders included. This is the one-window-per-widget
        // half of the same list the overlay builds from its own slots, and
        // `is_path_kind` here (which excludes folders, and is about a record
        // carrying its own path) left both folders and files out of the
        // physics, so icons dropped from above fell straight through them.
        .filter(|r| plugins::is_desktop_item(&r.kind))
        .map(|r| {
            let (w, h) = plugins::size(&r.kind, &r.data);
            LayoutItem {
                id: r.id.clone(),
                x: r.x,
                y: r.y,
                w: w as i32,
                h: h as i32,
            }
        })
        .collect()
}

// ---------- commands ----------

/// One widget record.
///
/// This exists because `loadRecord` used to call `floaty_list` and filter: every
/// widget's mount then pulled the whole store over IPC — 8.5MB with the icons
/// inlined — so a page with sixty widgets moved ~510MB and parsed it sixty times.
/// Measured: `floaty_list` at ~5s per call, reloads of 20-60s. One record is a
/// few KB.
#[tauri::command]
async fn floaty_get_record(id: String, app: AppHandle) -> Option<WidgetRecord> {
    let state = app.state::<AppState>();
    let guard = state.0.lock().ok()?;
    guard.widgets.get(&id).cloned()
}

#[tauri::command]
async fn floaty_list(app: AppHandle) -> Vec<WidgetRecord> {
    let state = app.state::<AppState>();
    state
        .0
        .lock()
        .map(|g| g.widgets.values().cloned().collect())
        .unwrap_or_default()
}

#[tauri::command]
fn floaty_create(kind: String, app: AppHandle) -> Result<WidgetRecord, String> {
    create_record(&app, &kind)
}

#[tauri::command]
async fn floaty_save(mut record: WidgetRecord, app: AppHandle) {
    let state = app.state::<AppState>();
    if let Ok(mut guard) = state.0.lock() {
        if guard.dead.contains(&record.id) {
            return; // removed meanwhile (remove / folder-merge)
        }
        // Protect a stored icon from being overwritten by stale incoming data:
        // never lose an icon, and never trade a crisp one for a blurry one.
        if let Some(existing) = guard.widgets.get(&record.id) {
            if let Some(existing_icon) = existing.data.get("icon").and_then(|v| v.as_str()) {
                let incoming_icon = record.data.get("icon").and_then(|v| v.as_str()).unwrap_or("");
                let would_lose = icon_is_missing(incoming_icon) && !icon_is_missing(existing_icon);
                let would_downgrade =
                    is_low_res_icon(incoming_icon) && !is_low_res_icon(existing_icon);
                if would_lose || would_downgrade {
                    if let Some(obj) = record.data.as_object_mut() {
                        obj.insert(
                            "icon".to_string(),
                            serde_json::Value::String(existing_icon.to_string()),
                        );
                    }
                }
            }
            if record.kind == "folder" {
                let existing_items = folder_items(existing);
                let mut incoming_items = folder_items(&record);
                for in_it in incoming_items.iter_mut() {
                    if let Some(ex) = existing_items.iter().find(|e| e.target == in_it.target) {
                        let would_lose = icon_is_missing(&in_it.icon) && !icon_is_missing(&ex.icon);
                        let would_downgrade =
                            is_low_res_icon(&in_it.icon) && !is_low_res_icon(&ex.icon);
                        if would_lose || would_downgrade {
                            in_it.icon = ex.icon.clone();
                        }
                    }
                }
                set_folder_items(&mut record, &incoming_items);
            }
        }
        guard.widgets.insert(record.id.clone(), record);
    }
    persist(&app);
}

/// Re-read these widgets from the store and rebuild them where they now are.
///
/// A floatie mounts once and then holds its own copy of the record: a plugin that
/// moves *other* floaties — the trail plugin lines them up along a drawn path —
/// can write their positions, but the ones on screen will not notice until they are
/// remounted. This is that remount, over the same event a rename sends. It is how
/// an icon that was still in the air when it was placed ends up standing where it
/// was put, instead of falling to the floor the next time it is mounted.
#[tauri::command]
fn floaty_refresh(ids: Vec<String>, app: AppHandle) {
    let records: Vec<WidgetRecord> = {
        let state = app.state::<AppState>();
        let guard = match state.0.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        ids.iter()
            .filter_map(|id| guard.widgets.get(id).cloned())
            .collect()
    };
    if records.is_empty() {
        return;
    }
    log_line(
        &app,
        &format!("refresh: remounted {} floatie(s)", records.len()),
    );
    for rec in &records {
        app.emit("floaty-widget-updated", rec).ok();
    }
}

#[tauri::command]
fn floaty_remove(id: String, app: AppHandle) {
    log_line(&app, &format!("remove {id}"));
    let state: State<'_, AppState> = app.state::<AppState>();
    if let Ok(mut guard) = state.0.lock() {
        guard.widgets.remove(&id);
        guard.dead.insert(id.clone());
    }
    persist(&app);
    close_widget_async(&app, &id);
}

// ---------- real file operations (desktop parity) ----------

/// The on-disk path a widget points at, with its kind. Folders keep theirs in
/// `path`, files and launchable items in `target`.
fn widget_path(app: &AppHandle, id: &str) -> Result<(String, String), String> {
    let state = app.state::<AppState>();
    let guard = state.0.lock().map_err(|e| e.to_string())?;
    let rec = guard.widgets.get(id).ok_or("widget not found")?;
    let key = plugins::path_key(&rec.kind);
    let path = key
        .and_then(|k| rec.data.get(k))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if path.trim().is_empty() {
        return Err(format!("this {} has no path", rec.kind));
    }
    Ok((rec.kind.clone(), path))
}

/// True when the path is one this app manages (a desktop item), i.e. deleting
/// the floatie may delete the file as well. Anything else is left alone —
/// removing the floatie for an installed program must not uninstall it.
fn is_desktop_item(app: &AppHandle, path: &str) -> bool {
    let p = std::path::Path::new(path);
    let root = load_settings(app).files_root;
    if !root.trim().is_empty() && p.starts_with(&root) {
        return true;
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        if p.starts_with(std::path::Path::new(&home).join("Desktop")) {
            return true;
        }
    }
    false
}

/// Delete a floatie the way the desktop would: a file or folder that lives on
/// the desktop goes to the Recycle Bin, so deleting the widget is a real
/// delete. Paths outside the desktop only lose their floatie.
#[tauri::command]
fn floaty_delete(id: String, app: AppHandle) -> Result<String, String> {
    let (kind, path) = widget_path(&app, &id)?;
    let managed = is_desktop_item(&app, &path)
        && (kind == "file" || kind == "folder" || kind == "app");
    let note = if managed {
        shell_ops::recycle(std::path::Path::new(&path))?;
        format!("moved '{}' to the recycle bin", path)
    } else {
        format!("removed the floatie; '{}' stays on disk", path)
    };
    let state: State<'_, AppState> = app.state::<AppState>();
    if let Ok(mut guard) = state.0.lock() {
        guard.widgets.remove(&id);
        guard.dead.insert(id.clone());
    }
    persist(&app);
    close_widget_async(&app, &id);
    app.emit("floaty-widget-removed", &id).ok();
    log_line(&app, &format!("delete {id}: {note}"));
    Ok(note)
}

/// The shell's own "Open with" picker.
#[tauri::command]
fn floaty_open_with(id: String, app: AppHandle) -> Result<(), String> {
    let (_, path) = widget_path(&app, &id)?;
    shell_ops::open_with(&path)
}

/// Show the item selected in File Explorer.
#[tauri::command]
fn floaty_reveal(id: String, app: AppHandle) -> Result<(), String> {
    let (_, path) = widget_path(&app, &id)?;
    shell_ops::reveal(&path)
}

/// The real Windows property sheet.
#[tauri::command]
fn floaty_properties(id: String, app: AppHandle) -> Result<(), String> {
    let (_, path) = widget_path(&app, &id)?;
    shell_ops::properties(&path)
}

/// Rename on disk, keeping the widget's label, target and folder path in step.
#[tauri::command]
fn floaty_rename(id: String, name: String, app: AppHandle) -> Result<WidgetRecord, String> {
    let (kind, path) = widget_path(&app, &id)?;
    let old = std::path::PathBuf::from(&path);
    let new_path = shell_ops::rename(&old, &name)?;
    let new_str = new_path.to_string_lossy().to_string();
    let new_name = new_path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| name.clone());
    let rec = {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        let r = guard.widgets.get_mut(&id).ok_or("widget not found")?;
        if let Some(obj) = r.data.as_object_mut() {
            obj.insert("name".to_string(), serde_json::Value::String(new_name));
            obj.insert("target".to_string(), serde_json::Value::String(new_str.clone()));
            if kind == "folder" {
                obj.insert("path".to_string(), serde_json::Value::String(new_str.clone()));
            }
        }
        r.clone()
    };
    persist(&app);
    app.emit("floaty-widget-updated", &rec).ok();
    log_line(&app, &format!("renamed {id}: {} -> {new_str}", old.display()));

    // A rename can change the icon (x.txt -> x.png): re-resolve in the background
    // and push it, so the tile never keeps the icon of its old name.
    let old_ext = old.extension().map(|e| e.to_string_lossy().to_lowercase());
    let new_ext = new_path.extension().map(|e| e.to_string_lossy().to_lowercase());
    if old_ext != new_ext {
        let handle = app.clone();
        let target = new_str.clone();
        let widget_id = id.clone();
        tauri::async_runtime::spawn(async move {
            let resolved = {
                let t = target.clone();
                tauri::async_runtime::spawn_blocking(move || resolve_icon_data_url(&t))
                    .await
                    .ok()
                    .flatten()
            };
            let Some(icon) = resolved else { return };
            {
                let state = handle.state::<AppState>();
                let Ok(mut guard) = state.0.lock() else { return };
                if let Some(r) = guard.widgets.get_mut(&widget_id) {
                    if let Some(obj) = r.data.as_object_mut() {
                        obj.insert("icon".to_string(), serde_json::Value::String(icon));
                    }
                }
            }
            persist(&handle);
            handle.emit("floaty-icon-refreshed", &widget_id).ok();
            log_line(&handle, &format!("icon re-resolved after renaming {widget_id}"));
        });
    }
    Ok(rec)
}

#[tauri::command]
fn floaty_show_settings(app: AppHandle) -> Result<(), String> {
    show_settings(&app).map_err(|e| {
        log_line(&app, &format!("show_settings FAILED: {e}"));
        e.to_string()
    })
}

#[tauri::command]
fn floaty_quit(app: AppHandle) {
    #[cfg(windows)]
    {
        desktop_pin::cleanup_hook();
    }
    app.exit(0);
}

#[tauri::command]
async fn floaty_log(msg: String, app: AppHandle) {
    log_line(&app, &format!("webview: {msg}"));
}

// ---------- audio visualizer ----------

/// Claim the system-audio capture (ref-counted, started by the first
/// visualizer widget). `fps` is the redraw rate the widget wants.
#[tauri::command]
fn floaty_audio_start(fps: Option<u32>, app: AppHandle) {
    audio::start(&app, fps);
}

/// Release one visualizer's claim on the capture thread.
#[tauri::command]
fn floaty_audio_stop() {
    audio::stop();
}

#[tauri::command]
fn floaty_audio_set_fps(fps: u32) {
    audio::set_fps(fps);
}

/// False when loopback capture could not start, so the visualizer can say so
/// instead of sitting there as a flat line.
#[tauri::command]
fn floaty_audio_status() -> bool {
    audio::is_running()
}

// ---------- system monitor ----------

/// Claim system sampling (ref-counted, started by the first sysmon widget).
#[tauri::command]
fn floaty_sysmon_start(interval_ms: Option<u32>, app: AppHandle) {
    sysmon::start(&app, interval_ms);
}

/// Release one sysmon widget's claim on the sampler.
#[tauri::command]
fn floaty_sysmon_stop() {
    sysmon::stop();
}

#[tauri::command]
fn floaty_sysmon_set_interval(ms: u32) {
    sysmon::set_interval(ms);
}

/// Number of samples the widget graph keeps — one source of truth.
#[tauri::command]
fn floaty_sysmon_history() -> usize {
    sysmon::HISTORY
}

/// False when the sampler could not start (e.g. no GPU performance counters),
/// which lets the widget say so instead of showing dashes forever.
#[tauri::command]
fn floaty_sysmon_status() -> bool {
    sysmon::is_running()
}

// ---------- app ----------
// (probe build 2)

pub fn run() {
    tauri::Builder::default()
        // First, before anything that touches the store: a second instance must
        // not get as far as loading `floaty-store.json`. The plugin holds a
        // mutex named after the app identifier and, when it is already held,
        // hands the new launch's arguments to this instance and exits — before
        // the app's own `setup` runs. What a second launch means, then, is "the
        // user wants Floaty": raise the settings window they were probably
        // reaching for, instead of two backends fighting over one store.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            log_line(
                app,
                &format!(
                    "second launch ignored ({} arg(s)): raising the settings window",
                    argv.len().saturating_sub(1)
                ),
            );
            // Deferred on purpose: this runs inside the first instance's window
            // procedure, with the second process blocked on it. Creating a
            // webview window from there would do the work with a foreign message
            // on the stack, so let the event loop pick it up instead.
            let handle = app.clone();
            std::thread::spawn(move || {
                let inner = handle.clone();
                let _ = handle.run_on_main_thread(move || {
                    if let Err(e) = show_settings(&inner) {
                        log_line(&inner, &format!("second launch: settings FAILED: {e}"));
                    }
                });
            });
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState(Mutex::new(StoreData::default())))
        .setup(|app| {
            let _ = SHARED_APP.set(app.handle().clone());
            // the hidden manager window outlives the overlay: it is the most
            // stable place to hang the display-state notification
            if let Some(m) = app.get_webview_window("manager") {
                if let Ok(hwnd) = m.hwnd() {
                    watch_display_state(hwnd.0 as isize);
                }
            }
            // The window-bound registration above depends on that window's
            // message procedure staying in the chain. This one is delivered to a
            // pool thread instead, so it survives any window rebuild — and it is
            // the only path that still runs while the main thread is blocked.
            let handle = app.handle().clone();
            watch_display_by_callback(&handle);
            start_pump_watchdog();
            start_top_layer_watchdog();
            log_line(&handle, "=== floaty starting ===");
            log_line(&handle, &format!("backend build {}", env!("FLOATY_BUILD_MARK")));

            #[cfg(windows)]
            {
                let stay = load_settings(&handle).stay_on_desktop;
                desktop_pin::set_stay_on_desktop_global(stay);
            }

            // restore persisted widgets into state
            let saved = load_all(&handle);
            // the store we just loaded is known good: pin it as the backup
            refresh_backup(&store_file(&handle), true);
            let mut reclassified = 0usize;
            {
                let state = handle.state::<AppState>();
                let mut guard = state.0.lock().expect("store lock");
                let mut max_n: u64 = 0;
                for mut rec in saved {
                    if let Some(n) = rec
                        .id
                        .rsplit('-')
                        .next()
                        .and_then(|s| s.parse::<u64>().ok())
                    {
                        max_n = max_n.max(n);
                    }
                    // Classification rule: launchable entries stay apps, loose
                    // files are file floaties (document icon + open-with). Also
                    // migrates records written before the rule existed.
                    if is_path_kind(&rec.kind) {
                        if let Some(t) = rec.data.get("target").and_then(|v| v.as_str()) {
                            let want = kind_for_path(std::path::Path::new(t), false);
                            if want != rec.kind.as_str() {
                                log_line(
                                    &handle,
                                    &format!("reclassify {} {} -> {}", rec.id, rec.kind, want),
                                );
                                rec.kind = want.to_string();
                                reclassified += 1;
                            }
                        }
                    }
                    guard.widgets.insert(rec.id.clone(), rec);
                }
                guard.next = max_n;
            }
            if reclassified > 0 {
                persist(&handle);
            }

            // Start on boot is a registry entry, so reconcile it with the setting
            // at launch: this is what repairs the entry when the executable has
            // moved (an update in place, a new build), which a toggle alone cannot
            // do.
            let boot = load_settings(&handle);
            if !set_start_on_boot(&handle, boot.start_on_boot) {
                log_line(&handle, "autostart: could not reconcile the startup entry");
            }

            // tray: settings + quit only (widgets are managed via settings)
            let settings =
                MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Floaty", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&settings, &quit])?;
            let icon = app
                .default_window_icon()
                .cloned()
                .expect("tray icon should exist (run `tauri icon` after adding assets/icon.png)");
            TrayIconBuilder::with_id("floaty-tray")
                .icon(icon)
                .tooltip("Floaty")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "settings" => {
                        if let Err(e) = show_settings(app) {
                            log_line(app, &format!("tray settings FAILED: {e}"));
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            log_line(&handle, "tray ready");

            // debug harness: auto-open settings to repro issues
            if std::env::var("FLOATY_OPEN_SETTINGS").is_ok() {
                log_line(&handle, "FLOATY_OPEN_SETTINGS is set");
                if let Err(e) = show_settings(&handle) {
                    log_line(&handle, &format!("show_settings FAILED: {e}"));
                }
            }

            // demo harness: float a few real apps from the top (gravity test)
            if std::env::var("FLOATY_DEMO").is_ok() {
                let apps = scan_apps_blocking();
                for (i, a) in apps.iter().take(3).enumerate() {
                    let data = serde_json::json!({ "name": a.name, "target": a.path });
                    let x = 180 + (i as i32 * 220);
                    if let Ok(rec) =
                        create_record_with(&handle, "app", data, Some((x, 30)))
                    {
                        log_line(&handle, &format!("demo floated {}", rec.id));
                    }
                }
                // also a clock + pet for the visual check
                create_record(&handle, "clock").ok();
                create_record(&handle, "pet").ok();
            }

            // spawn restored widgets, or a welcome note on first run
            let ids: Vec<WidgetRecord> = {
                let state = handle.state::<AppState>();
                state
                    .0
                    .lock()
                    .map(|g| g.widgets.values().cloned().collect())
                    .unwrap_or_default()
            };
            if ids.is_empty() {
                if let Ok(rec) = create_record(&handle, "note") {
                    let state = handle.state::<AppState>();
                    if let Ok(mut guard) = state.0.lock() {
                        if let Some(r) = guard.widgets.get_mut(&rec.id) {
                            r.data = serde_json::json!({
                                "text": "welcome to floaty!\n\n- drag me by the top bar\n- right-click the tray icon for more\n- i live on your desktop now"
                            });
                        }
                    }
                    persist(&handle);
                }
            }
            let (installed, rejected) = plugins::install_from(&plugins_dir(&handle));
            if !installed.is_empty() || !rejected.is_empty() {
                log_line(
                    &handle,
                    &format!(
                        "plugins: {} installed [{}]{}",
                        installed.len(),
                        installed.join(", "),
                        if rejected.is_empty() {
                            String::new()
                        } else {
                            format!(", {} rejected [{}]", rejected.len(), rejected.join("; "))
                        }
                    ),
                );
            }

            if let Err(e) = spawn_overlay_window(&handle) {
                log_line(&handle, &format!("spawn_overlay_window FAILED: {e}"));
            }
            // A pin survives a restart: whatever was pinned comes back in the top
            // layer, which means building that layer again.
            sync_top_overlay(&handle);
            let bg_app = handle.clone();
            tauri::async_runtime::spawn(async move {
                upgrade_low_res_icons(&bg_app).await;
                let s = load_settings(&bg_app);
                if !s.files_root.is_empty() {
                    let _ = floaty_sync_files(None, bg_app.clone());
                }
            });

            log_line(&handle, "=== floaty ready ===");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            floaty_list,
            floaty_get_record,
            floaty_create,
            floaty_save,
            floaty_refresh,
            floaty_remove,
            floaty_delete,
            floaty_open_with,
            floaty_reveal,
            floaty_properties,
            floaty_rename,
            floaty_show_settings,
            floaty_quit,
            floaty_plugins,
            floaty_plugins_dir,
            floaty_open_plugins_dir,
            floaty_rescan_plugins,
            floaty_set_plugin_enabled,
            floaty_get_settings,
            floaty_set_settings,
            floaty_scan_apps,
            floaty_add_launcher,
            floaty_icon,
            floaty_launch,
            floaty_launch_target,
            floaty_dropped,
            floaty_ungroup,
            floaty_scan_models,
            floaty_set_widget_model,
            floaty_layout,
            floaty_window_cursor_pos,
            floaty_sync_files,
            floaty_rename_folder_dir,
            floaty_resolve_folder_icons,
            floaty_update_hit_rects,
            floaty_set_overlay_dragging,
            floaty_set_on_top,
            floaty_log,
            floaty_heartbeat,
            floaty_audio_start,
            floaty_audio_stop,
            floaty_audio_set_fps,
            floaty_audio_status,
            floaty_sysmon_start,
            floaty_sysmon_stop,
            floaty_sysmon_set_interval,
            floaty_sysmon_history,
            floaty_sysmon_status
        ])
        .build(tauri::generate_context!())
        .expect("error while building floaty")
        .run(|app, event| {
            // last chance to flush: widgets save as they change, but a pending
            // move or edit should not be lost when the app is closed. Synchronous
            // on purpose — the coalescing writer is on a timer and would not get
            // another chance to run.
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                STORE_DIRTY.store(false, std::sync::atomic::Ordering::SeqCst);
                write_store_now(app);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A settings file written before `start_on_boot` existed has to keep loading:
    /// the new field defaults to off, and everything the file did say survives.
    #[test]
    fn an_old_settings_file_leaves_start_on_boot_off() {
        let old = r#"{ "gravity": 1200.0, "animated_ratio": 0.0, "stay_on_desktop": false }"#;
        let s: FloatSettings =
            serde_json::from_str(old).expect("an old settings file must still parse");
        assert!(!s.start_on_boot, "a file without the field means off");
        assert!(!s.stay_on_desktop, "what the file did say must survive");
        assert_eq!(s.gravity, 1200.0);
        // and it round-trips, so the new field is written out from now on
        let back: FloatSettings =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).expect("round trip");
        assert!(!back.start_on_boot);
    }

    /// The float rows used to overlap: `floatiness` multiplied
    /// `float_amplitude`, so neither number was the travel, and `float_spread`
    /// was a percentage of a stagger that never reached the tiles. An old file
    /// must come out of the migration with the look it had and one knob per
    /// idea — and exactly once, however many times the app is started.
    #[test]
    fn an_old_float_file_folds_its_multiplier_in_once() {
        let mut old: serde_json::Value = serde_json::from_str(
            r#"{ "floatiness": 2.0, "float_amplitude": 4.0, "float_period": 6.0,
                 "float_spread": 80.0, "animation_mode": "wave" }"#,
        )
        .expect("the shape of a real pre-change settings file");
        migrate_float_settings(&mut old);
        let s: FloatSettings = serde_json::from_value(old.clone()).expect("must still parse");
        assert_eq!(s.float_amplitude, 8.0, "2x of 4px is what the user was looking at");
        assert_eq!(s.float_spread, 12.0, "0-200 scale becomes percent of one cycle");
        assert_eq!(s.float_period, 6.0, "nothing else moves");
        assert!(
            !old.as_object().unwrap().contains_key("floatiness"),
            "the marker is dropped, so the fold cannot happen twice"
        );
        migrate_float_settings(&mut old);
        let again: FloatSettings = serde_json::from_value(old).expect("parses again");
        assert_eq!(again.float_amplitude, 8.0, "a second pass must not fold again");

        // a file that already speaks the new scale is left alone
        let mut new: serde_json::Value =
            serde_json::from_str(r#"{ "float_amplitude": 6.0, "float_spread": 10.0 }"#).unwrap();
        migrate_float_settings(&mut new);
        let n: FloatSettings = serde_json::from_value(new).unwrap();
        assert_eq!(n.float_amplitude, 6.0);
        assert_eq!(n.float_spread, 10.0);

        // and the results land on the sliders' own grids, so the handle and the
        // label of the row can never disagree about what they are showing
        let mut odd: serde_json::Value = serde_json::from_str(
            r#"{ "floatiness": 1.5, "float_amplitude": 4.5, "float_spread": 33.0 }"#,
        )
        .unwrap();
        migrate_float_settings(&mut odd);
        let o: FloatSettings = serde_json::from_value(odd).unwrap();
        assert_eq!(o.float_amplitude, 7.0, "6.75px rounded to the 0.5px step");
        assert_eq!(o.float_spread, 5.0, "4.95% rounded to a whole percent");
    }

    /// The desktop-layer window policy — no taskbar button, swallowed minimize,
    /// the tool-window style re-applied on every move — belongs to the windows
    /// that *are* the desktop. Applying it to the settings window is the bug
    /// this pins shut: its minimize button did nothing and the first drag step
    /// stripped its taskbar button.
    #[test]
    fn the_desktop_layer_is_the_overlays_and_the_floatie_windows() {
        assert!(is_desktop_layer_label("desktop-overlay"));
        assert!(is_desktop_layer_label("top-overlay"));
        assert!(is_desktop_layer_label("widget-app-1"));
        assert!(!is_desktop_layer_label("settings"));
        assert!(!is_desktop_layer_label("manager"));
    }

    /// This decides whether a floatie may be taken off the desktop, so it has to
    /// be strict: an entry *of the root itself*, compared without case.
    #[test]
    fn only_a_roots_own_entry_counts_as_mirrored() {
        let root = "C:\\Users\\eric\\Desktop";
        assert!(mirrors_root_entry(root, "C:\\Users\\eric\\Desktop\\a.txt"));
        assert!(mirrors_root_entry(root, "c:\\users\\eric\\desktop\\Sub"));
        assert!(mirrors_root_entry(
            "C:\\Users\\eric\\Desktop\\",
            "C:\\Users\\eric\\Desktop\\a.txt"
        ));
        // deeper than the root: the root's listing cannot account for it
        assert!(!mirrors_root_entry(
            root,
            "C:\\Users\\eric\\Desktop\\sub\\b.txt"
        ));
        // a sibling whose name merely starts with the root's
        assert!(!mirrors_root_entry(
            root,
            "C:\\Users\\eric\\Desktop2\\a.txt"
        ));
        assert!(!mirrors_root_entry(root, "D:\\elsewhere\\a.txt"));
        assert!(!mirrors_root_entry(root, "C:\\Users\\eric\\Desktop"));
        assert!(!mirrors_root_entry(root, "a.txt"));
        assert!(!mirrors_root_entry("", "C:\\a.txt"));
    }

    /// Only a kind that stands for something on disk answers, and only when the
    /// record actually points somewhere: a folder made by grouping has no
    /// directory, so the mirror must never treat it as one.
    #[test]
    fn record_path_only_answers_for_path_kinds() {
        let app = WidgetRecord {
            id: "app-1".into(),
            kind: "app".into(),
            x: 0,
            y: 0,
            data: serde_json::json!({ "name": "Chrome", "target": "C:\\x\\Chrome.lnk" }),
        };
        assert_eq!(record_path(&app).as_deref(), Some("C:\\x\\Chrome.lnk"));

        let folder = WidgetRecord {
            id: "folder-2".into(),
            kind: "folder".into(),
            x: 0,
            y: 0,
            data: serde_json::json!({ "name": "Projects", "path": "C:\\x\\Projects", "items": [] }),
        };
        assert_eq!(record_path(&folder).as_deref(), Some("C:\\x\\Projects"));

        let grouped = WidgetRecord {
            id: "folder-3".into(),
            kind: "folder".into(),
            x: 0,
            y: 0,
            data: serde_json::json!({ "name": "Group", "items": [] }),
        };
        assert_eq!(record_path(&grouped), None);

        let note = WidgetRecord {
            id: "note-4".into(),
            kind: "note".into(),
            x: 0,
            y: 0,
            data: serde_json::json!({ "text": "hi" }),
        };
        assert_eq!(record_path(&note), None);
    }

    #[test]
    fn test_kind_for_path() {
        use std::path::Path;
        // directories are folders
        assert_eq!(kind_for_path(Path::new("C:/x/Desktop/Stuff"), true), "folder");
        // launchable entries are apps
        for p in [
            "C:/Users/e/Desktop/Code.lnk",
            "C:/Program Files/App/app.EXE",
            "C:/Users/e/Desktop/site.url",
            "C:/tools/run.cmd",
        ] {
            assert_eq!(kind_for_path(Path::new(p), false), "app", "{p} should be an app");
        }
        // loose documents/images/archives are files
        for p in [
            "C:/Users/e/Desktop/notes.txt",
            "C:/Users/e/Desktop/report.pdf",
            "C:/Users/e/Desktop/photo.JPG",
            "C:/Users/e/Desktop/archive.zip",
            "C:/Users/e/Desktop/no-extension",
        ] {
            assert_eq!(kind_for_path(Path::new(p), false), "file", "{p} should be a file");
        }
        assert!(is_path_kind("app") && is_path_kind("file"));
        assert!(!is_path_kind("folder") && !is_path_kind("note"));
    }

    #[test]
    fn ungroup_keeps_an_item_where_it_is_when_the_folder_has_no_directory() {
        use std::path::Path;
        // an in-app folder (made by grouping): its items were never moved into a
        // directory, so the destination is the item's own directory and the
        // caller's "is it different?" test skips the move. The bug moved them two
        // levels up — out of the Desktop, into the parent of the Desktop.
        let src = Path::new(r"C:\Users\e\Desktop\CrystalDiskInfo.lnk");
        assert_eq!(
            ungroup_dest_dir(None, src).as_deref(),
            Some(Path::new(r"C:\Users\e\Desktop"))
        );
    }

    #[test]
    fn ungroup_moves_an_item_beside_a_folder_that_lives_on_disk() {
        use std::path::Path;
        let src = Path::new(r"C:\Users\e\Desktop\Stuff\thing.lnk");
        assert_eq!(
            ungroup_dest_dir(Some(r"C:\Users\e\Desktop\Stuff"), src).as_deref(),
            Some(Path::new(r"C:\Users\e\Desktop"))
        );
    }

    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "floaty_{tag}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// What the shell says a .lnk points at — the same COM object Explorer uses,
    /// so a shortcut that does not resolve here would not work for the user
    /// either.
    #[cfg(windows)]
    fn shortcut_target(lnk: &std::path::Path) -> std::path::PathBuf {
        use std::os::windows::process::CommandExt;
        const NO_WINDOW: u32 = 0x08000000;
        let script = format!(
            "(New-Object -ComObject WScript.Shell).CreateShortcut('{}').TargetPath",
            lnk.to_string_lossy().replace('\'', "''")
        );
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(NO_WINDOW)
            .output()
            .expect("powershell should run");
        let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert!(
            out.status.success() && !raw.is_empty(),
            "could not read {} back: {}",
            lnk.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        std::path::PathBuf::from(raw)
    }

    /// The shell hands a shortcut's target back the way it stored it, and on a volume
    /// with 8.3 aliases enabled that is the short name (the runner's temp directory
    /// comes back as RUNNER~1) while the test built its path from %TEMP% — so the two
    /// strings disagree while both name the same file, which is the whole point of the
    /// shortcut. Compare the files: canonicalize expands the alias to the long path.
    fn pinned(p: &std::path::Path) -> std::path::PathBuf {
        std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
    }

    fn folder_item(name: &str, target: &std::path::Path) -> FolderItem {
        FolderItem {
            name: name.to_string(),
            target: target.to_string_lossy().to_string(),
            icon: String::new(),
            is_dir: target.is_dir(),
        }
    }

    /// Dragging two icons together makes a folder named the way Explorer names
    /// one, counting only past the names that are taken.
    #[test]
    fn a_new_folder_is_named_the_way_windows_names_one() {
        let dir = scratch_dir("newfolder");
        assert_eq!(new_folder_path(&dir), dir.join("New folder"));

        let first = create_new_folder(&dir).unwrap();
        assert_eq!(first, dir.join("New folder"));
        assert_eq!(create_new_folder(&dir).unwrap(), dir.join("New folder (2)"));
        assert_eq!(create_new_folder(&dir).unwrap(), dir.join("New folder (3)"));

        // the first free name in the sequence wins, exactly like the shell: with
        // "New folder" deleted again, that is the one it hands out next
        std::fs::remove_dir(&first).unwrap();
        assert_eq!(create_new_folder(&dir).unwrap(), dir.join("New folder"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shortcut_names_are_usable_file_names() {
        assert_eq!(shortcut_file_name("Visual Studio Code"), "Visual Studio Code.lnk");
        // characters Windows refuses in a name are dropped, not escaped
        assert_eq!(shortcut_file_name("a/b\\c:d*e?f\"g<h>i|j"), "abcdefghij.lnk");
        // a label that is punctuation only still gets a usable name
        assert_eq!(shortcut_file_name("  ..  "), "Shortcut.lnk");
        // an app already named with its extension is not double-suffixed
        assert_eq!(shortcut_file_name("Steam.lnk"), "Steam.lnk");
    }

    /// Grouping may only carry the real file into the new folder when the root
    /// owns it; an app icon points at the program itself, and moving *that* would
    /// take the app out of its install directory.
    #[test]
    fn only_what_the_root_owns_is_moved_into_a_folder() {
        use std::path::Path;
        let root = Path::new(r"C:\Users\e\Desktop\floaty-root");
        assert!(can_move_into_folder(
            Some(root),
            Path::new(r"C:\Users\e\Desktop\floaty-root\notes.txt")
        ));
        assert!(!can_move_into_folder(
            Some(root),
            Path::new(r"C:\Program Files\App\app.exe")
        ));
        assert!(!can_move_into_folder(
            Some(root),
            Path::new(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\App.lnk")
        ));
        // nothing pointed at: nothing may be moved behind the user's back
        assert!(!can_move_into_folder(
            None,
            Path::new(r"C:\Users\e\Desktop\notes.txt")
        ));
    }

    /// The grouping itself, on disk: the dragged file is *in* the new folder
    /// afterwards, and its floatie points at the new path.
    #[test]
    fn grouping_moves_the_real_file_into_the_new_folder() {
        let root = scratch_dir("group_move");
        let src = root.join("notes.txt");
        std::fs::write(&src, "hello").unwrap();

        let dir = create_new_folder(&root).unwrap();
        let (item, log) =
            place_item_in_dir(&dir, &folder_item("notes.txt", &src), Some(&root)).unwrap();

        assert!(!src.exists(), "the original should have moved");
        assert_eq!(std::path::PathBuf::from(&item.target), dir.join("notes.txt"));
        assert!(log.starts_with("moved into folder"), "{log}");
        // the new folder scans back with the moved file in it
        let items = scan_folder_items(&dir);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "notes.txt");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// An app icon points at the program itself, and the program must stay put:
    /// the folder gets a shortcut file, which the shell resolves back to it.
    #[cfg(windows)]
    #[test]
    fn grouping_an_app_writes_a_shortcut_instead_of_moving_the_program() {
        let root = scratch_dir("group_app");
        let program_dir = scratch_dir("group_app_prog");
        let exe = program_dir.join("Some App.exe");
        std::fs::write(&exe, b"MZ").unwrap();

        let dir = create_new_folder(&root).unwrap();
        let (item, log) = place_item_in_dir(&dir, &folder_item("Some App", &exe), Some(&root)).unwrap();

        assert!(exe.exists(), "the program must not be moved");
        let made = std::path::PathBuf::from(&item.target);
        assert_eq!(made, dir.join("Some App.lnk"));
        assert_eq!(
            pinned(&shortcut_target(&made)),
            pinned(&exe),
            "the shortcut must point at the app"
        );
        assert!(log.starts_with("shortcut in folder"), "{log}");
        assert!(!item.is_dir && item.name == "Some App.lnk");

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&program_dir);
    }

    /// Floating an app from the picker leaves a shortcut in the pointed root, so
    /// the app icon is a file there like the file and folder ones — and a second
    /// float of the same app makes a second file instead of overwriting.
    #[cfg(windows)]
    #[test]
    fn floating_an_app_puts_its_shortcut_in_the_root() {
        let root = scratch_dir("app_root");
        let program_dir = scratch_dir("app_root_prog");
        let exe = program_dir.join("Thing.exe");
        std::fs::write(&exe, b"MZ").unwrap();

        let (name, path, _) = shortcut_into_root(&root, "Thing", &exe.to_string_lossy()).unwrap();
        assert_eq!(name, "Thing.lnk");
        let made = std::path::PathBuf::from(&path);
        assert_eq!(made, root.join("Thing.lnk"));
        assert_eq!(pinned(&shortcut_target(&made)), pinned(&exe));
        assert!(exe.exists(), "the program is only pointed at, never moved");

        let (_, second, _) = shortcut_into_root(&root, "Thing", &exe.to_string_lossy()).unwrap();
        // file collisions count from 1 here (the helper the folder moves share),
        // unlike Explorer's folder numbering that starts at 2
        assert_eq!(std::path::PathBuf::from(second), root.join("Thing (1).lnk"));

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&program_dir);
    }

    /// A floatie that already *is* a shortcut is copied, not rewritten, so its
    /// arguments, working directory and icon come along untouched.
    #[test]
    fn a_shortcut_source_is_copied_rather_than_rewritten() {
        let root = scratch_dir("shortcut_copy");
        let owned = scratch_dir("shortcut_copy_src");
        let lnk_src = owned.join("App.lnk");
        std::fs::write(&lnk_src, b"shell-authored shortcut bytes").unwrap();

        let dir = create_new_folder(&root).unwrap();
        let (item, log) = place_item_in_dir(&dir, &folder_item("App", &lnk_src), Some(&root)).unwrap();

        assert!(lnk_src.exists(), "the original shortcut stays where it was");
        let made = std::path::PathBuf::from(&item.target);
        assert_eq!(made, dir.join("App.lnk"));
        assert_eq!(std::fs::read(&made).unwrap(), b"shell-authored shortcut bytes");
        assert!(log.starts_with("shortcut in folder"), "{log}");

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&owned);
    }

    #[test]
    fn test_desktop_handle() {
        #[cfg(windows)]
        {
            desktop_pin::ensure_input_desktop();
            let h = desktop_pin::get_desktop_shell_hwnd();
            println!("Desktop shell HWND found: {h:#x}");
            assert_ne!(h, 0, "Desktop shell HWND should not be 0!");
        }
    }

    #[test]
    fn test_unique_dest_path() {
        let temp_dir = std::env::temp_dir().join(format!("floaty_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let file_name = std::ffi::OsStr::new("test_file.txt");
        let p1 = unique_dest_path(&temp_dir, file_name);
        assert_eq!(p1, temp_dir.join("test_file.txt"));

        // Create the file so it exists
        std::fs::write(&p1, "hello").unwrap();

        let p2 = unique_dest_path(&temp_dir, file_name);
        assert_eq!(p2, temp_dir.join("test_file (1).txt"));

        std::fs::write(&p2, "hello 2").unwrap();
        let p3 = unique_dest_path(&temp_dir, file_name);
        assert_eq!(p3, temp_dir.join("test_file (2).txt"));

        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_scan_folder_items() {
        let temp_dir = std::env::temp_dir().join(format!("floaty_test_scan_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let sub_dir = temp_dir.join("sub_folder");
        std::fs::create_dir_all(&sub_dir).unwrap();
        let file_a = temp_dir.join("a.txt");
        std::fs::write(&file_a, "content").unwrap();
        let file_hidden = temp_dir.join("desktop.ini");
        std::fs::write(&file_hidden, "hidden").unwrap();

        let items = scan_folder_items(&temp_dir);
        // desktop.ini should be ignored, so items should be sub_folder (dir first) and a.txt
        assert_eq!(items.len(), 2);
        assert!(items[0].is_dir);
        assert_eq!(items[0].name, "sub_folder");
        assert!(!items[1].is_dir);
        assert_eq!(items[1].name, "a.txt");

        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn icons_are_measured_from_the_png_header() {
        // 24 bytes: PNG signature + IHDR length/type + width/height, which is all
        // icon_pixel_size reads. Built by hand so the test needs no image file.
        fn png(w: u32, h: u32) -> String {
            let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
            bytes.extend_from_slice(&13u32.to_be_bytes());
            bytes.extend_from_slice(b"IHDR");
            bytes.extend_from_slice(&w.to_be_bytes());
            bytes.extend_from_slice(&h.to_be_bytes());
            format!("data:image/png;base64,{}", base64_encode(&bytes))
        }

        assert_eq!(icon_pixel_size(&png(32, 32)), Some((32, 32)));
        assert_eq!(icon_pixel_size(&png(256, 256)), Some((256, 256)));
        assert_eq!(icon_pixel_size("data:image/png;base64,not-base64!!"), None);

        // A 32px icon is low-res (an upgrade may do better) but it is NOT missing:
        // calling it missing is what re-ran the resolver on every mount.
        assert!(is_low_res_icon(&png(32, 32)));
        assert!(!icon_is_missing(&png(32, 32)));
        assert!(icon_is_missing(""));
        assert!(icon_is_missing("none"));
        assert!(icon_is_missing("data:image/png;base64,not-base64!!"));
        assert!(!is_low_res_icon(&png(256, 256)));
        assert!(!icon_is_missing(&png(256, 256)));
    }

    #[test]
    fn rescans_keep_the_icons_already_resolved() {
        let item = |name: &str, target: &str, icon: &str| FolderItem {
            name: name.to_string(),
            target: target.to_string(),
            icon: icon.to_string(),
            is_dir: false,
        };
        let icon32 = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACAAAAAg";
        let old = vec![
            item("a.txt", "C:\\d\\a.txt", icon32),
            item("b.txt", "C:\\d\\b.txt", ""),
        ];

        // sync: matched by target, and a 32px icon must survive the rescan
        let mut rescanned = vec![item("a.txt", "C:\\d\\a.txt", ""), item("b.txt", "C:\\d\\b.txt", "")];
        carry_icons_by_target(&old, &mut rescanned);
        assert_eq!(rescanned[0].icon, icon32);
        assert_eq!(rescanned[1].icon, "");

        // folder-dir rename: every target moved, so match by name instead
        let mut renamed = vec![item("a.txt", "C:\\d\\RENAMED\\a.txt", "")];
        carry_icons_by_name(&old, &mut renamed);
        assert_eq!(renamed[0].icon, icon32);
    }

    #[test]
    fn test_disk_move_relative() {
        let temp_dir = std::env::temp_dir().join(format!("floaty_test_move_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let folder_dir = temp_dir.join("MyFolder");
        std::fs::create_dir_all(&folder_dir).unwrap();

        // 1. Test moving file out to parent (root)
        let file_inside = folder_dir.join("doc.txt");
        std::fs::write(&file_inside, "data").unwrap();
        assert!(file_inside.exists());

        let dest_dir = folder_dir.parent().unwrap();
        let dest_file = unique_dest_path(dest_dir, file_inside.file_name().unwrap());
        assert_eq!(dest_file, temp_dir.join("doc.txt"));

        std::fs::rename(&file_inside, &dest_file).unwrap();
        assert!(!file_inside.exists());
        assert!(dest_file.exists());

        // 2. Test moving directory out to parent (root)
        let sub_inside = folder_dir.join("SubProject");
        std::fs::create_dir_all(&sub_inside).unwrap();
        std::fs::write(sub_inside.join("inner.txt"), "inner").unwrap();
        assert!(sub_inside.exists());

        let dest_sub = unique_dest_path(dest_dir, sub_inside.file_name().unwrap());
        assert_eq!(dest_sub, temp_dir.join("SubProject"));

        std::fs::rename(&sub_inside, &dest_sub).unwrap();
        assert!(!sub_inside.exists());
        assert!(dest_sub.is_dir());
        assert!(dest_sub.join("inner.txt").exists());

        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_fast_ico_and_url_extraction() {
        assert_eq!(base64_encode(b"hello world"), "aGVsbG8gd29ybGQ=");

        // Test with real Steam game url if present
        let test_url = r"C:\Users\erict\OneDrive\Desktop\games\Magical Princess.url";
        if std::path::Path::new(test_url).exists() {
            let res = try_extract_fast_url(test_url);
            assert!(res.is_some(), "try_extract_fast_url should successfully extract Steam icon");
            let data_url = res.unwrap();
            assert!(data_url.starts_with("data:image/png;base64,"), "should produce valid png data url");
            assert!(data_url.len() > 1000, "should produce high-res icon data");
        }
    }

    #[test]
    fn store_loads_from_backup_when_the_live_file_is_damaged() {
        let dir = std::env::temp_dir().join(format!("floaty-store-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("floaty-store.json");
        let bak = dir.join("floaty-store.bak");
        let good = r#"[{"id":"note-1","kind":"note","x":10,"y":20,"data":{}}]"#;

        // a truncated write must not be mistaken for "nothing to load"
        std::fs::write(&path, r#"[{"id":"note-1","kin"#).unwrap();
        std::fs::write(&bak, good).unwrap();
        let (recs, from_backup) = read_json_with_backup::<Vec<WidgetRecord>>(&path)
            .expect("a damaged live file with a good backup must still load");
        assert!(from_backup, "the backup should have been used");
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].id, "note-1");

        // a healthy live file wins
        std::fs::write(&path, good).unwrap();
        let (_, from_backup) = read_json_with_backup::<Vec<WidgetRecord>>(&path).unwrap();
        assert!(!from_backup);

        // both damaged: nothing to load
        std::fs::write(&path, "not json").unwrap();
        std::fs::write(&bak, "{").unwrap();
        assert!(read_json_with_backup::<Vec<WidgetRecord>>(&path).is_none());

        // atomic write leaves the file readable and no temp behind
        write_text_atomic(&path, good);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), good);
        assert!(!path.with_extension("tmp").exists());
        assert_eq!(serde_json::from_str::<Vec<WidgetRecord>>(&std::fs::read_to_string(&path).unwrap()).unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
