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
    if !matches!(kind, "note" | "clock" | "pet" | "app" | "folder") {
        return Err("unknown widget kind".into());
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

/// Resolve a launcher's real app icon: .lnk target via WScript.Shell, pixels
/// via ExtractAssociatedIcon, returned as a PNG data URL. No new crates —
/// plain powershell.exe, which ships with Windows.
fn resolve_icon_data_url(lnk_path: &str) -> Option<String> {
    let escaped = lnk_path.replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Drawing; $p='{escaped}'; $sh=New-Object -ComObject WScript.Shell; $s=$sh.CreateShortcut($p); \
         $t=$s.TargetPath; if([string]::IsNullOrEmpty($t)){{$t=$p}}; \
         try{{$ico=[System.Drawing.Icon]::ExtractAssociatedIcon($t)}}catch{{exit 2}}; \
         if($null -eq $ico){{exit 3}}; \
         $tmp=[IO.Path]::Combine([IO.Path]::GetTempPath(),[IO.Path]::GetRandomFileName()+'.png'); \
         $ico.ToBitmap().Save($tmp,[System.Drawing.Imaging.ImageFormat]::Png); \
         [Convert]::ToBase64String([IO.File]::ReadAllBytes($tmp))"
    );
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const NO_WINDOW: u32 = 0x08000000;
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(NO_WINDOW)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let b64 = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if b64.len() < 100
            || !b64
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
        {
            return None;
        }
        Some(format!("data:image/png;base64,{b64}"))
    }
    #[cfg(not(windows))]
    {
        let _ = script;
        None
    }
}

#[tauri::command]
async fn floaty_icon(id: String, app: AppHandle) -> Result<String, String> {
    // serve the cached icon if we already resolved one
    {
        let state = app.state::<AppState>();
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        if let Some(r) = guard.widgets.get(&id) {
            if let Some(s) = r.data.get("icon").and_then(|v| v.as_str()) {
                if !s.is_empty() {
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
    let data_url = tauri::async_runtime::spawn_blocking(move || resolve_icon_data_url(&target))
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
fn floaty_save(record: WidgetRecord, app: AppHandle) {
    let state = app.state::<AppState>();
    if let Ok(mut guard) = state.0.lock() {
        if guard.dead.contains(&record.id) {
            return; // removed meanwhile (remove / folder-merge)
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

            // tray
            let add_note = MenuItem::with_id(app, "add-note", "Add note", true, None::<&str>)?;
            let add_clock =
                MenuItem::with_id(app, "add-clock", "Add clock", true, None::<&str>)?;
            let add_pet = MenuItem::with_id(app, "add-pet", "Add pet", true, None::<&str>)?;
            let settings =
                MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Floaty", true, None::<&str>)?;
            let menu =
                Menu::with_items(app, &[&add_note, &add_clock, &add_pet, &settings, &quit])?;
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
                    "add-note" => {
                        create_record(app, "note").ok();
                    }
                    "add-clock" => {
                        create_record(app, "clock").ok();
                    }
                    "add-pet" => {
                        create_record(app, "pet").ok();
                    }
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
                for rec in &ids {
                    if let Err(e) = spawn_widget(&handle, rec) {
                        log_line(&handle, &format!("restore spawn FAILED {}: {e}", rec.id));
                    }
                }
            }
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
            floaty_get_settings,
            floaty_set_settings,
            floaty_scan_apps,
            floaty_add_launcher,
            floaty_icon,
            floaty_launch,
            floaty_launch_target,
            floaty_dropped,
            floaty_layout,
            floaty_log
        ])
        .run(tauri::generate_context!())
        .expect("error while running floaty");
}
