use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

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
    /// plugin kinds whose windows stay closed
    #[serde(default)]
    disabled: Vec<String>,
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
            disabled: Vec::new(),
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
        disabled: settings.disabled,
    };
    if let Ok(json) = serde_json::to_string_pretty(&s) {
        fs::write(settings_file(&app), json).ok();
    }
    app.emit("floaty-settings-changed", &s).ok();
    log_line(&app, "settings updated");
    s
}

// ---------- windows ----------

fn widget_size(kind: &str, data: &serde_json::Value) -> (f64, f64) {
    let base = match kind {
        "clock" => (250.0, 330.0),
        "pet" => (170.0, 170.0),
        "app" => (92.0, 112.0),
        "live2d" => (300.0, 400.0),
        "folder" => (92.0, 112.0),
        _ => (300.0, 330.0),
    };
    // notes + clocks are resizable; restore the user's size when stored
    if matches!(kind, "note" | "clock") {
        let (mw, mh) = if kind == "clock" {
            (200.0, 260.0)
        } else {
            (180.0, 140.0)
        };
        let w = data.get("w").and_then(|v| v.as_f64()).unwrap_or(base.0);
        let h = data.get("h").and_then(|v| v.as_f64()).unwrap_or(base.1);
        (w.clamp(mw, 1400.0), h.clamp(mh, 1400.0))
    } else {
        base
    }
}

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
    let (w, h) = widget_size(&rec.kind, &rec.data);
    let url = format!("index.html#/{}/{}", rec.kind, rec.id);
    log_line(app, &format!("spawn {} at {},{}", label, rec.x, rec.y));
    WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title("Floaty")
        .inner_size(w, h)
        .position(rec.x as f64, rec.y as f64)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        // notes + clocks are user-resizable (corner handle in the webview)
        .resizable(matches!(rec.kind.as_str(), "note" | "clock"))
        .skip_taskbar(true)
        // desktop layer for everything by default; per-widget pin-on-top
        // lives in the webview (right-click menu) instead
        .always_on_top(false)
        .build()?;
    Ok(())
}

fn default_data(kind: &str) -> serde_json::Value {
    match kind {
        "note" => serde_json::json!({ "text": "" }),
        "pet" => serde_json::json!({ "name": "bloop" }),
        "app" => serde_json::json!({ "name": "app", "target": "" }),
        "live2d" => serde_json::json!({ "name": "Live2D" }),
        "folder" => serde_json::json!({ "name": "Folder", "items": [] }),
        _ => serde_json::json!({}),
    }
}

fn create_record_with(
    app: &AppHandle,
    kind: &str,
    data: serde_json::Value,
    at: Option<(i32, i32)>,
) -> Result<WidgetRecord, String> {
    if !matches!(kind, "note" | "clock" | "pet" | "app" | "folder" | "live2d") {
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
    let data = default_data(kind);
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

/// Check if an icon data url is low resolution (legacy 32x32 extraction)
fn is_low_res_icon(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    s.starts_with("data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACAAAAAg")
        || s.contains("AAAAACAAAAAg")
        || s.contains("AAAACAAAAAg")
        || s.len() < 5000
}

/// Resolve a launcher's high-resolution app icon: .lnk shortcut targets, MSI advertised
/// shortcuts, explicit IconLocations, or direct binaries. Extracts up to 256x256 (or
/// highest available native resolution) via PrivateExtractIcons and Shell ImageList,
/// returned as a PNG data URL.
fn resolve_icon_data_url(lnk_path: &str) -> Option<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const NO_WINDOW: u32 = 0x08000000;
        let script = r#"
Add-Type -AssemblyName System.Drawing;
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class ShellIcons {
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern uint PrivateExtractIcons(
        string szFileName, int nIconIndex, int cxIcon, int cyIcon,
        out IntPtr phicon, out uint piconid, uint nIcons, uint flags);
    [DllImport("user32.dll")] public static extern bool DestroyIcon(IntPtr hIcon);
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr SHGetFileInfo(string pszPath, uint dwFileAttributes, ref SHFILEINFO psfi, uint cbSizeFileInfo, uint uFlags);
    [DllImport("shell32.dll")] public static extern int SHGetImageList(int iImageList, ref Guid riid, out IntPtr ppv);
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct SHFILEINFO {
        public IntPtr hIcon;
        public int iIcon;
        public uint dwAttributes;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] public string szDisplayName;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 80)] public string szTypeName;
    }
    [ComImport, Guid("46EB5926-582E-4017-9FDF-E8998DAA0950"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IImageList { [PreserveSig] int GetIcon(int i, int flags, out IntPtr picon); }
    public static IntPtr ExtractFromShell(string path) {
        try {
            SHFILEINFO sfi = new SHFILEINFO();
            IntPtr res = SHGetFileInfo(path, 0, ref sfi, (uint)Marshal.SizeOf(sfi), 0x000004000);
            if (res == IntPtr.Zero) return IntPtr.Zero;
            Guid iid = new Guid("46EB5926-582E-4017-9FDF-E8998DAA0950");
            IntPtr pImageList;
            int hr = SHGetImageList(4, ref iid, out pImageList);
            if (hr != 0 || pImageList == IntPtr.Zero) {
                hr = SHGetImageList(2, ref iid, out pImageList);
                if (hr != 0 || pImageList == IntPtr.Zero) return IntPtr.Zero;
            }
            IImageList imgList = (IImageList)Marshal.GetObjectForIUnknown(pImageList);
            IntPtr hIcon = IntPtr.Zero;
            imgList.GetIcon(sfi.iIcon, 1, out hIcon);
            Marshal.Release(pImageList);
            return hIcon;
        } catch { return IntPtr.Zero; }
    }
}
"@;

$path = $env:FLOATY_ICON_PATH;
if (-not (Test-Path $path)) { exit 1 };

$iconFile = $path;
$iconIdx = 0;
$targetPath = $path;

if ($path.EndsWith('.lnk', [StringComparison]::OrdinalIgnoreCase)) {
    try {
        $sh = New-Object -ComObject WScript.Shell;
        $sc = $sh.CreateShortcut($path);
        $targetPath = $sc.TargetPath;
        $iconLoc = $sc.IconLocation;
        if (-not [string]::IsNullOrEmpty($iconLoc)) {
            $parts = $iconLoc.Split(',');
            if (-not [string]::IsNullOrEmpty($parts[0])) {
                $iconFile = $parts[0];
                if ($parts.Length -gt 1) { [void][int]::TryParse($parts[1], [ref]$iconIdx) }
            }
        }
    } catch {}

    if ([string]::IsNullOrEmpty($targetPath)) {
        try {
            $shApp = New-Object -ComObject Shell.Application;
            $dir = [IO.Path]::GetDirectoryName($path);
            $fn = [IO.Path]::GetFileName($path);
            $folder = $shApp.Namespace($dir);
            $item = $folder.ParseName($fn);
            $link = $item.GetLink;
            if ($link -and $link.Target -and $link.Target.Path) {
                $targetPath = $link.Target.Path;
            }
        } catch {}
    }
    if ($iconFile -eq $path -and -not [string]::IsNullOrEmpty($targetPath)) {
        $iconFile = $targetPath;
    }
}

$hIcon = [IntPtr]::Zero;
$iconId = 0;
if (-not [string]::IsNullOrEmpty($iconFile) -and (Test-Path $iconFile)) {
    [void][ShellIcons]::PrivateExtractIcons($iconFile, $iconIdx, 256, 256, [ref]$hIcon, [ref]$iconId, 1, 0);
}
if ($hIcon -eq [IntPtr]::Zero -and -not [string]::IsNullOrEmpty($targetPath) -and $targetPath -ne $iconFile -and (Test-Path $targetPath)) {
    [void][ShellIcons]::PrivateExtractIcons($targetPath, 0, 256, 256, [ref]$hIcon, [ref]$iconId, 1, 0);
}
if ($hIcon -eq [IntPtr]::Zero -and -not [string]::IsNullOrEmpty($targetPath) -and (Test-Path $targetPath)) {
    $hIcon = [ShellIcons]::ExtractFromShell($targetPath);
}
if ($hIcon -eq [IntPtr]::Zero) {
    $hIcon = [ShellIcons]::ExtractFromShell($path);
}

$ico = $null;
if ($hIcon -ne [IntPtr]::Zero) {
    $ico = [System.Drawing.Icon]::FromHandle($hIcon);
} else {
    $check = if (-not [string]::IsNullOrEmpty($targetPath) -and (Test-Path $targetPath)) { $targetPath } else { $path };
    try { $ico = [System.Drawing.Icon]::ExtractAssociatedIcon($check) } catch {}
}

if ($null -eq $ico) { exit 2 };

$bmp = $ico.ToBitmap();
$ms = New-Object IO.MemoryStream;
$bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png);
$b64 = [Convert]::ToBase64String($ms.ToArray());
$ms.Dispose();
$bmp.Dispose();
if ($hIcon -ne [IntPtr]::Zero) { [void][ShellIcons]::DestroyIcon($hIcon) };
[Console]::Out.Write($b64);
"#;

        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("FLOATY_ICON_PATH", lnk_path)
            .creation_flags(NO_WINDOW)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let raw = String::from_utf8_lossy(&out.stdout);
        let b64 = raw
            .lines()
            .map(|l| l.trim())
            .filter(|l| {
                l.len() >= 100
                    && l.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
            })
            .last()?
            .to_string();
        Some(format!("data:image/png;base64,{b64}"))
    }
    #[cfg(not(windows))]
    {
        let _ = lnk_path;
        None
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
        .ok_or("no icon found")?;
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

    if !to_upgrade.is_empty() {
        log_line(app, &format!("upgrade_low_res_icons: upgrading {} app icons", to_upgrade.len()));
    }

    for (id, target) in to_upgrade {
        let target_clone = target.clone();
        if let Ok(Some(hi_res)) =
            tauri::async_runtime::spawn_blocking(move || resolve_icon_data_url(&target_clone)).await
        {
            {
                let state = app.state::<AppState>();
                let mut guard = state.0.lock();
                if let Ok(ref mut g) = guard {
                    if let Some(r) = g.widgets.get_mut(&id) {
                        if let Some(obj) = r.data.as_object_mut() {
                            obj.insert(
                                "icon".to_string(),
                                serde_json::Value::String(hi_res.clone()),
                            );
                        }
                    }
                }
            }
            persist(app);
            app.emit("floaty-icon-refreshed", &id).ok();
            log_line(app, &format!("upgraded icon for {id} to high-res"));
        }
    }

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
                        if is_low_res_icon(&it.icon) && !it.target.trim().is_empty() {
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

    if !folders_to_check.is_empty() {
        log_line(app, &format!("upgrade_low_res_icons: upgrading icons in {} folders", folders_to_check.len()));
    }

    for (folder_id, needed) in folders_to_check {
        let mut changed = false;
        for (idx, target) in needed {
            let target_clone = target.clone();
            if let Ok(Some(hi_res)) =
                tauri::async_runtime::spawn_blocking(move || resolve_icon_data_url(&target_clone)).await
            {
                let state = app.state::<AppState>();
                let mut guard = state.0.lock();
                if let Ok(ref mut g) = guard {
                    if let Some(r) = g.widgets.get_mut(&folder_id) {
                        let mut items = folder_items(r);
                        if idx < items.len() {
                            items[idx].icon = hi_res;
                            set_folder_items(r, &items);
                            changed = true;
                        }
                    }
                }
            }
        }
        if changed {
            persist(app);
            app.emit("floaty-folder-changed", &folder_id).ok();
            log_line(
                app,
                &format!("upgraded folder icons for {folder_id} to high-res"),
            );
        }
    }
}

// ---------- plugins ----------

#[derive(Debug, Clone, Serialize)]
struct PluginInfo {
    id: String,
    name: String,
    description: String,
    enabled: bool,
}

fn all_plugins(disabled: &[String]) -> Vec<PluginInfo> {
    let is_on = |kind: &str| !disabled.iter().any(|d| d == kind);
    vec![
        PluginInfo {
            id: "note".to_string(),
            name: "Note".to_string(),
            description: "Sticky notes with autosave. Drag by the top bar, resize by the corner.".to_string(),
            enabled: is_on("note"),
        },
        PluginInfo {
            id: "clock".to_string(),
            name: "Clock".to_string(),
            description: "Clock plus pomodoro timer. Tap the timer to edit lengths.".to_string(),
            enabled: is_on("clock"),
        },
        PluginInfo {
            id: "pet".to_string(),
            name: "Pet".to_string(),
            description: "A wandering blob. Hover to calm it, double-click to freeze it.".to_string(),
            enabled: is_on("pet"),
        },
        PluginInfo {
            id: "app".to_string(),
            name: "App launcher".to_string(),
            description: "Gravity icons for real apps. Pin, drop, launch; icons group into folders.".to_string(),
            enabled: is_on("app"),
        },
        PluginInfo {
            id: "folder".to_string(),
            name: "Folder".to_string(),
            description: "Groups of launchers. Click to expand into a launch grid.".to_string(),
            enabled: is_on("folder"),
        },
        PluginInfo {
            id: "live2d".to_string(),
            name: "Live2D".to_string(),
            description: "Animated Live2D companion. Drag it around, click it for motions.".to_string(),
            enabled: is_on("live2d"),
        },
    ]
}

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
    app.emit("floaty-plugins-changed", &all_plugins(&s.disabled)).ok();
}

#[tauri::command]
fn floaty_plugins(app: AppHandle) -> Vec<PluginInfo> {
    all_plugins(&load_settings(&app).disabled)
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

fn launcher_item(rec: &WidgetRecord) -> Option<FolderItem> {
    if rec.kind != "app" {
        return None;
    }
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
    })
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
        let item = launcher_item(rec)?;
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
            let mut items = folder_items(rec);
            if !items.iter().any(|it| it.target == dragged.target) {
                if dragged.icon.is_empty() {
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
        launcher_item(trec)?
    };
    let mut items = vec![titem, dragged];
    for it in items.iter_mut() {
        if it.icon.is_empty() {
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

/// Pull one app out of a folder: remove the item, float it as its own
/// pinned launcher where the cursor released it.
#[tauri::command]
fn floaty_ungroup(folder_id: String, index: usize, x: i32, y: i32, app: AppHandle) -> Result<WidgetRecord, String> {
    let item = {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        let rec = guard.widgets.get_mut(&folder_id).ok_or("folder not found")?;
        if rec.kind != "folder" {
            return Err("not a folder".into());
        }
        let mut items = folder_items(rec);
        if index >= items.len() {
            return Err("bad index".into());
        }
        let item = items.remove(index);
        if item.target.trim().is_empty() {
            return Err("empty target".into());
        }
        set_folder_items(rec, &items);
        item
    };
    persist(&app);
    app.emit("floaty-folder-changed", &folder_id).ok();
    let mut data = serde_json::json!({ "name": item.name, "target": item.target, "icon": item.icon });
    data["pinned"] = serde_json::Value::Bool(true);
    let rec = create_record_with(&app, "app", data, Some((x, y)))?;
    log_line(&app, &format!("ungrouped {} from {folder_id} as {}", item.name, rec.id));
    Ok(rec)
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
            let (w, h) = widget_size(&r.kind, &r.data);
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
                for (idx, in_it) in incoming_items.iter_mut().enumerate() {
                    if idx < existing_items.len()
                        && !is_low_res_icon(&existing_items[idx].icon)
                        && is_low_res_icon(&in_it.icon)
                    {
                        in_it.icon = existing_items[idx].icon.clone();
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
            floaty_log
        ])
        .run(tauri::generate_context!())
        .expect("error while running floaty");
}
