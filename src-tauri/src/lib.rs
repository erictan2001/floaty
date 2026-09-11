use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

mod plugins;
use plugins::PluginInfo;

// ---------- logging ----------

fn log_line(app: &AppHandle, msg: &str) {
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

fn persist(app: &AppHandle) {
    // Serialize under the lock, write after releasing it: holding the store
    // mutex across file IO serialized every save/list/create and stalled the
    // settings window while widgets persisted positions in the background.
    let json = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().expect("store lock");
        let list: Vec<&WidgetRecord> = guard.widgets.values().collect();
        serde_json::to_string_pretty(&list).unwrap_or_else(|_| "[]".to_string())
    };
    fs::write(store_file(app), json).ok();
}

fn load_all(app: &AppHandle) -> Vec<WidgetRecord> {
    let path = store_file(app);
    let Ok(bytes) = fs::read(path) else {
        return vec![];
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
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
    #[serde(default = "default_floatiness")]
    floatiness: f64,
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
}

fn default_stay_on_desktop() -> bool {
    true
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
fn default_floatiness() -> f64 {
    1.0
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

fn load_settings(app: &AppHandle) -> FloatSettings {
    let parsed: Option<FloatSettings> =
        fs::read(settings_file(app)).ok().and_then(|b| serde_json::from_slice(&b).ok());
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
            ..s
        },
        None => FloatSettings {
            pet_speed: default_pet_speed(),
            gravity: default_gravity(),
            bounce: default_bounce(),
            floatiness: default_floatiness(),
            single_click: default_single_click(),
            double_click: default_double_click(),
            live2d_root: String::new(),
            files_root: String::new(),
            disabled: Vec::new(),
            stay_on_desktop: default_stay_on_desktop(),
        },
    }
}

#[tauri::command]
fn floaty_get_settings(app: AppHandle) -> FloatSettings {
    load_settings(&app)
}

#[tauri::command]
fn floaty_set_settings(settings: FloatSettings, app: AppHandle) -> FloatSettings {
    let s = FloatSettings {
        pet_speed: settings.pet_speed.clamp(0.0, 3.0),
        gravity: settings.gravity.clamp(0.0, 8000.0),
        bounce: settings.bounce.clamp(0.0, 0.95),
        floatiness: settings.floatiness.clamp(0.0, 2.0),
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
    };
    if let Ok(json) = serde_json::to_string_pretty(&s) {
        fs::write(settings_file(&app), json).ok();
    }
    #[cfg(windows)]
    {
        desktop_pin::set_stay_on_desktop_global(s.stay_on_desktop);
        for (label, win) in app.webview_windows() {
            if label.starts_with("widget-") {
                if let Ok(hwnd) = win.hwnd() {
                    desktop_pin::apply_desktop_pin(hwnd.0 as isize, s.stay_on_desktop);
                }
            }
        }
    }
    app.emit("floaty-settings-changed", &s).ok();
    log_line(&app, "settings updated");
    s
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
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_TOOLWINDOW: isize = 0x00000080;
    const WS_EX_APPWINDOW: isize = 0x00040000;

    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_FRAMECHANGED: u32 = 0x0020;
    const SWP_HIDEWINDOW: u32 = 0x0080;

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
    extern "system" {
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

    unsafe extern "system" fn widget_subclass_proc(
        hwnd: isize,
        msg: u32,
        wparam: usize,
        lparam: isize,
        _id_subclass: usize,
        _ref_data: usize,
    ) -> isize {
        // Enforce WS_EX_TOOLWINDOW is preserved so taskbar never shows the window
        if msg == WM_WINDOWPOSCHANGING {
            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            if (ex_style & WS_EX_TOOLWINDOW) == 0 || (ex_style & WS_EX_APPWINDOW) != 0 {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex_style | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW);
            }
        }

        if STAY_ON_DESKTOP_ACTIVE.load(Ordering::Relaxed) {
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
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }

    pub fn apply_desktop_pin(hwnd: isize, stay_on_desktop: bool) {
        unsafe {
            remove_from_taskbar(hwnd);

            // Register subclass (idempotent if already registered)
            SetWindowSubclass(hwnd, widget_subclass_proc, SUBCLASS_ID, 0);

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
}

// ---------- windows ----------

fn widget_label(id: &str) -> String {
    format!("widget-{id}")
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
    let (w, h) = plugins::widget_size(&rec.kind, &rec.data);
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
        .resizable(plugins::is_resizable(&rec.kind))
        .skip_taskbar(true)
        // desktop layer for everything by default; per-widget pin-on-top
        // lives in the webview (right-click menu) instead
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

fn create_record_with(
    app: &AppHandle,
    kind: &str,
    data: serde_json::Value,
    at: Option<(i32, i32)>,
) -> Result<WidgetRecord, String> {
    if !plugins::is_valid_kind(kind) {
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
    // Async window creation: never block the command thread on the main
    // thread (see spawn_widget_async). The record is already stored, so the
    // caller gets its response immediately.
    spawn_widget_async(app, &rec);
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

/// Check if an icon data url is low resolution (legacy 32x32 extraction or missing)
fn is_low_res_icon(s: &str) -> bool {
    if s.is_empty() || s == "none" {
        return true;
    }
    s.starts_with("data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACAAAAAg")
        || s.contains("AAAAACAAAAAg")
        || s.contains("AAAACAAAAAg")
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

static ICON_CACHE: std::sync::LazyLock<Mutex<HashMap<String, String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

fn resolve_icons_batch(paths: &[String]) -> HashMap<String, String> {
    let mut results = HashMap::new();
    let mut needed = Vec::new();

    if let Ok(guard) = ICON_CACHE.lock() {
        for p in paths {
            if let Some(cached) = guard.get(p) {
                if cached != "none" && !is_low_res_icon(cached) {
                    results.insert(p.clone(), cached.clone());
                    continue;
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
            if let Ok(mut guard) = ICON_CACHE.lock() {
                guard.insert(p.clone(), icon_data);
            }
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
Add-Type -AssemblyName System.Drawing;

function Get-IconB64($filePath) {
    if ([string]::IsNullOrEmpty($filePath) -or -not [System.IO.File]::Exists($filePath)) { return $null }

    if ($filePath.EndsWith('.ico', [StringComparison]::OrdinalIgnoreCase)) {
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
$input | ForEach-Object {
    $p = $_.Trim()
    if ([string]::IsNullOrEmpty($p) -or -not [System.IO.File]::Exists($p)) { return }
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
        [Console]::Out.WriteLine($p + '|' + $b64)
    } else {
        [Console]::Out.WriteLine($p + '|none')
    }
}
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
                    let mut cache_guard = ICON_CACHE.lock().ok();
                    for line in raw.lines() {
                        let trimmed = line.trim();
                        if let Some((p, b64)) = trimmed.split_once('|') {
                            let val = if b64 == "none" {
                                "none".to_string()
                            } else {
                                format!("data:image/png;base64,{b64}")
                            };
                            results.insert(p.to_string(), val.clone());
                            if let Some(ref mut cg) = cache_guard {
                                cg.insert(p.to_string(), val);
                            }
                        }
                    }
                }
            }
        }
    }

    results
}

fn resolve_icon_data_url(lnk_path: &str) -> Option<String> {
    if let Ok(guard) = ICON_CACHE.lock() {
        if let Some(cached) = guard.get(lnk_path) {
            return if cached == "none" { None } else { Some(cached.clone()) };
        }
    }
    let map = resolve_icons_batch(&[lnk_path.to_string()]);
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
                if !s.is_empty() && !is_low_res_icon(s) {
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
            .filter(|r| r.kind == "app")
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
            if let Some(obj) = r.data.as_object_mut() {
                obj.insert(
                    "icon".to_string(),
                    serde_json::Value::String(data_url.clone()),
                );
            }
        }
    }
    persist(&app);
    Ok(data_url)
}

/// Background task that automatically detects any legacy 32x32 icons in saved app launchers
/// or folders and upgrades them to crisp native high-res icons (up to 256x256).
async fn upgrade_low_res_icons(app: &AppHandle) {
    let to_upgrade: Vec<(String, String)> = {
        let state = app.state::<AppState>();
        let Ok(guard) = state.0.lock() else { return };
        guard
            .widgets
            .iter()
            .filter(|(_, r)| r.kind == "app")
            .filter_map(|(id, r)| {
                let target = r.data.get("target")?.as_str()?.to_string();
                let icon = r.data.get("icon").and_then(|v| v.as_str()).unwrap_or("");
                if is_low_res_icon(icon) && !target.trim().is_empty() {
                    Some((id.clone(), target))
                } else {
                    None
                }
            })
            .collect()
    };

    let folders_to_check: Vec<(String, Vec<(usize, String)>)> = {
        let state = app.state::<AppState>();
        let Ok(guard) = state.0.lock() else { return };
        guard
            .widgets
            .iter()
            .filter(|(_, r)| r.kind == "folder")
            .filter_map(|(id, r)| {
                let items = folder_items(r);
                let needed: Vec<(usize, String)> = items
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, it)| {
                        if !it.is_dir && is_low_res_icon(&it.icon) && !it.target.trim().is_empty() {
                            Some((idx, it.target.clone()))
                        } else {
                            None
                        }
                    })
                    .collect();
                if needed.is_empty() {
                    None
                } else {
                    Some((id.clone(), needed))
                }
            })
            .collect()
    };

    if to_upgrade.is_empty() && folders_to_check.is_empty() {
        return;
    }

    let mut all_paths: Vec<String> = to_upgrade.iter().map(|(_, t)| t.clone()).collect();
    for (_, needed) in &folders_to_check {
        for (_, t) in needed {
            all_paths.push(t.clone());
        }
    }
    all_paths.sort();
    all_paths.dedup();

    log_line(app, &format!("upgrade_low_res_icons: batch resolving {} icons", all_paths.len()));

    let icon_map = tauri::async_runtime::spawn_blocking(move || {
        resolve_icons_batch(&all_paths)
    }).await.unwrap_or_default();

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
                for (idx, target) in needed {
                    if let Some(hi_res) = icon_map.get(target) {
                        if hi_res != "none" && *idx < items.len() && items[*idx].icon != *hi_res {
                            items[*idx].icon = hi_res.clone();
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
            .filter(|it| !it.is_dir && is_low_res_icon(&it.icon) && !it.target.trim().is_empty())
            .map(|it| it.target)
            .collect()
    };

    if needed.is_empty() {
        return Ok(());
    }

    let icon_map = tauri::async_runtime::spawn_blocking(move || {
        resolve_icons_batch(&needed)
    })
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
        fs::write(settings_file(app), json).ok();
    }
    app.emit("floaty-plugins-changed", &plugins::all_plugin_info(&s.disabled)).ok();
}

#[tauri::command]
fn floaty_plugins(app: AppHandle) -> Vec<PluginInfo> {
    plugins::all_plugin_info(&load_settings(&app).disabled)
}

#[tauri::command]
fn floaty_set_plugin_enabled(id: String, enabled: bool, app: AppHandle) {
    log_line(&app, &format!("plugin {id} enabled={enabled}"));
    set_plugin_enabled(&app, &id, enabled);
    // close (disable) or respawn (enable) this kind's windows
    let ids: Vec<String> = app
        .state::<AppState>()
        .0
        .lock()
        .map(|g| g.widgets.values().filter(|r| r.kind == id).map(|r| r.id.clone()).collect())
        .unwrap_or_default();
    if enabled {
        for wid in ids {
            let rec: Option<WidgetRecord> = app
                .state::<AppState>()
                .0
                .lock()
                .ok()
                .and_then(|g| g.widgets.get(&wid).cloned());
            if let Some(r) = rec {
                spawn_widget_async(&app, &r);
            }
        }
    } else {
        for wid in &ids {
            close_widget_async(&app, wid);
        }
    }
}

#[tauri::command]
fn floaty_add_launcher(name: String, path: String, app: AppHandle) -> Result<WidgetRecord, String> {
    if path.trim().is_empty() {
        return Err("empty path".into());
    }
    let data = serde_json::json!({ "name": name, "target": path });
    // spawn near the top so it falls with gravity on arrival
    create_record_with(&app, "app", data, Some((200, 40)))
}

// ---------- folders ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    if rec.kind == "app" {
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
            .filter(|r| r.kind == "app")
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
fn floaty_dropped(id: String, app: AppHandle) -> Option<String> {
    // live physical rect of the dropped icon; compare in physical px throughout
    let me = app.get_webview_window(&widget_label(&id))?;
    let (mp, ms) = (me.outer_position().ok()?, me.inner_size().ok()?);
    let cx = mp.x + ms.width as i32 / 2;
    let cy = mp.y + ms.height as i32 / 2;

    // candidate targets (snapshot under the lock, probe windows after release)
    let cands: Vec<(String, String)> = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().ok()?;
        guard
            .widgets
            .iter()
            .filter(|(oid, r)| *oid != &id && (r.kind == "app" || r.kind == "folder"))
            .map(|(oid, r)| (oid.clone(), r.kind.clone()))
            .collect()
    };
    let mut hit: Option<(String, String)> = None;
    for (oid, kind) in cands {
        if let Some(w) = app.get_webview_window(&widget_label(&oid)) {
            if let (Ok(p), Ok(s)) = (w.outer_position(), w.inner_size()) {
                let x = p.x - 12;
                let y = p.y - 12;
                let ww = s.width as i32 + 24;
                let hh = s.height as i32 + 24;
                if cx >= x && cx < x + ww && cy >= y && cy < y + hh {
                    hit = Some((oid, kind));
                    break;
                }
            }
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

    if target_kind == "folder" {
        {
            let state = app.state::<AppState>();
            let mut guard = state.0.lock().ok()?;
            let rec = guard.widgets.get_mut(&target_id)?;
            let folder_path = rec.data.get("path").and_then(|v| v.as_str()).map(|s| s.to_string());

            // If the folder is backed by a directory on disk, move the item into that directory on disk
            if let Some(ref fpath) = folder_path {
                let dest_dir = std::path::Path::new(fpath);
                let src = std::path::PathBuf::from(&dragged.target);
                if dest_dir.is_dir() && src.exists() {
                    dragged.is_dir = src.is_dir();
                    if let Some(file_name) = src.file_name().map(|f| f.to_os_string()) {
                        let dest_path = unique_dest_path(dest_dir, &file_name);
                        if dest_path != src {
                            if let Ok(_) = std::fs::rename(&src, &dest_path) {
                                log_line(&app, &format!("moved into folder on disk: {} -> {}", src.display(), dest_path.display()));
                                dragged.target = dest_path.to_string_lossy().to_string();
                                dragged.name = dest_path.file_name().unwrap_or(&file_name).to_string_lossy().to_string();
                            }
                        }
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

    // target is an app icon: fold both into a new folder at its spot
    let titem = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().ok()?;
        let trec = guard.widgets.get(&target_id)?;
        widget_as_folder_item(trec)?
    };
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
        let dest_dir: Option<std::path::PathBuf> = if let Some(ref fp) = folder_path {
            let p = std::path::Path::new(fp);
            p.parent().map(|parent| parent.to_path_buf())
        } else if let Some(parent) = src_path.parent().and_then(|p| p.parent()) {
            Some(parent.to_path_buf())
        } else {
            let s = load_settings(&app);
            if !s.files_root.is_empty() {
                Some(std::path::PathBuf::from(s.files_root))
            } else {
                None
            }
        };

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

    // Treat as new floatie: directory -> folder floatie, file -> app launcher floatie
    let rec = if item.is_dir || std::path::Path::new(&item.target).is_dir() {
        let sub_items = scan_folder_items(std::path::Path::new(&item.target));
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
        create_record_with(&app, "app", data, Some((x, y)))?
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
                    let items = scan_folder_items(&dest);
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
}

#[tauri::command]
fn floaty_sync_files(root: Option<String>, app: AppHandle) -> Result<FilesSyncResult, String> {
    let target_root = match root {
        Some(r) if !r.trim().is_empty() => {
            let mut s = load_settings(&app);
            s.files_root = r.trim().to_string();
            if let Ok(json) = serde_json::to_string_pretty(&s) {
                std::fs::write(settings_file(&app), json).ok();
            }
            s.files_root
        }
        _ => load_settings(&app).files_root,
    };

    if target_root.trim().is_empty() {
        return Ok(FilesSyncResult { files: 0, dirs: 0, total: 0 });
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
            } else if w.kind == "app" {
                if let Some(p) = w.data.get("target").and_then(|v| v.as_str()) {
                    map.insert(p.to_string(), (id.clone(), "app".to_string()));
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

    if let Ok(entries) = std::fs::read_dir(&base_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();
            if file_name.starts_with('.')
                || file_name.eq_ignore_ascii_case("desktop.ini")
                || file_name.eq_ignore_ascii_case("thumbs.db")
            {
                continue;
            }
            let path_str = path.to_string_lossy().to_string();
            if path.is_dir() {
                let mut items = scan_folder_items(&path);
                if let Some((fid, kind)) = existing_map.get(&path_str) {
                    if kind == "folder" {
                        if let Ok(guard) = app.state::<AppState>().0.lock() {
                            if let Some(w) = guard.widgets.get(fid) {
                                let old_items = folder_items(w);
                                for item in &mut items {
                                    if let Some(old) = old_items.iter().find(|o| o.target == item.target) {
                                        if !old.icon.is_empty() && old.icon != "none" && !is_low_res_icon(&old.icon) {
                                            item.icon = old.icon.clone();
                                        }
                                    }
                                }
                            }
                        }
                        updated_folders.push((fid.clone(), items));
                    }
                } else {
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
                dirs_count += 1;
            } else {
                if !existing_map.contains_key(&path_str) {
                    let (px, py) = next_pos(&mut occupied_positions);
                    next_id += 1;
                    let id = format!("app-{}", next_id);
                    let mut data = serde_json::json!({
                        "name": file_name,
                        "target": path_str,
                        "icon": "",
                    });
                    data["pinned"] = serde_json::Value::Bool(true);
                    new_records.push(WidgetRecord {
                        id,
                        kind: "app".to_string(),
                        x: px,
                        y: py,
                        data,
                    });
                }
                files_count += 1;
            }
        }
    }

    {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        guard.next = next_id;
        for (fid, items) in &updated_folders {
            if let Some(w) = guard.widgets.get_mut(fid) {
                set_folder_items(w, items);
            }
        }
        for rec in &new_records {
            guard.widgets.insert(rec.id.clone(), rec.clone());
        }
    }
    persist(&app);

    for (fid, _) in updated_folders {
        app.emit("floaty-folder-changed", &fid).ok();
    }

    if !new_records.is_empty() {
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

    let bg_app = app.clone();
    tauri::async_runtime::spawn(async move {
        upgrade_low_res_icons(&bg_app).await;
    });

    log_line(&app, &format!("synced files root '{}': {} files, {} dirs ({} new floaties)", target_root, files_count, dirs_count, new_records.len()));
    Ok(FilesSyncResult {
        files: files_count,
        dirs: dirs_count,
        total: files_count + dirs_count,
    })
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
        .filter(|r| r.kind == "app")
        .map(|r| {
            let (w, h) = plugins::widget_size(&r.kind, &r.data);
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

#[tauri::command]
fn floaty_list(app: AppHandle) -> Vec<WidgetRecord> {
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
fn floaty_save(mut record: WidgetRecord, app: AppHandle) {
    let state = app.state::<AppState>();
    if let Ok(mut guard) = state.0.lock() {
        if guard.dead.contains(&record.id) {
            return; // removed meanwhile (remove / folder-merge)
        }
        // Protect high-resolution icons from being overwritten by stale/low-res incoming data
        if let Some(existing) = guard.widgets.get(&record.id) {
            if let Some(existing_icon) = existing.data.get("icon").and_then(|v| v.as_str()) {
                if !is_low_res_icon(existing_icon) {
                    let incoming_icon = record.data.get("icon").and_then(|v| v.as_str()).unwrap_or("");
                    if is_low_res_icon(incoming_icon) {
                        if let Some(obj) = record.data.as_object_mut() {
                            obj.insert("icon".to_string(), serde_json::Value::String(existing_icon.to_string()));
                        }
                    }
                }
            }
            if record.kind == "folder" {
                let existing_items = folder_items(existing);
                let mut incoming_items = folder_items(&record);
                for in_it in incoming_items.iter_mut() {
                    if let Some(ex) = existing_items.iter().find(|e| e.target == in_it.target) {
                        if !is_low_res_icon(&ex.icon) && is_low_res_icon(&in_it.icon) {
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

#[tauri::command]
fn floaty_remove(id: String, app: AppHandle) {
    log_line(&app, &format!("remove {id}"));
    let state: State<'_, AppState> = app.state::<AppState>();
    if let Ok(mut guard) = state.0.lock() {
        guard.widgets.remove(&id);
        guard.dead.insert(id.clone());
    }
    persist(&app);
    // Close off the command thread: like creation, window ops must not block
    // command handling on the main-thread dispatch.
    let handle = app.clone();
    std::thread::spawn(move || {
        let h2 = handle.clone();
        let id2 = id.clone();
        if let Err(e) = handle.run_on_main_thread(move || {
            if let Some(w) = h2.get_webview_window(&widget_label(&id2)) {
                if let Err(e) = w.close() {
                    log_line(&h2, &format!("close window FAILED for {id2}: {e}"));
                }
            }
        }) {
            log_line(&handle, &format!("main-thread dispatch FAILED for {id}: {e}"));
        }
    });
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
    app.exit(0);
}

#[tauri::command]
fn floaty_log(msg: String, app: AppHandle) {
    log_line(&app, &format!("webview: {msg}"));
}

// ---------- app ----------
// (probe build)

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState(Mutex::new(StoreData::default())))
        .setup(|app| {
            let handle = app.handle().clone();
            log_line(&handle, "=== floaty starting ===");
            log_line(&handle, &format!("backend build {}", env!("FLOATY_BUILD_MARK")));

            #[cfg(windows)]
            {
                let stay = load_settings(&handle).stay_on_desktop;
                desktop_pin::set_stay_on_desktop_global(stay);
            }

            // restore persisted widgets into state
            let saved = load_all(&handle);
            {
                let state = handle.state::<AppState>();
                let mut guard = state.0.lock().expect("store lock");
                let mut max_n: u64 = 0;
                for rec in saved {
                    if let Some(n) = rec
                        .id
                        .rsplit('-')
                        .next()
                        .and_then(|s| s.parse::<u64>().ok())
                    {
                        max_n = max_n.max(n);
                    }
                    guard.widgets.insert(rec.id.clone(), rec);
                }
                guard.next = max_n;
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
            } else {
                let disabled_kinds = load_settings(&handle).disabled;
                for rec in &ids {
                    if disabled_kinds.iter().any(|d| d == &rec.kind) {
                        continue;
                    }
                    if let Err(e) = spawn_widget(&handle, rec) {
                        log_line(&handle, &format!("restore spawn FAILED {}: {e}", rec.id));
                    }
                }
            }
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
            floaty_create,
            floaty_save,
            floaty_remove,
            floaty_show_settings,
            floaty_quit,
            floaty_plugins,
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
            floaty_log
        ])
        .run(tauri::generate_context!())
        .expect("error while running floaty");
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
