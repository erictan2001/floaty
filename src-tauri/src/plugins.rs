//! The plugin manifest: one descriptor per widget kind, and the only place the
//! backend keeps per-kind facts.
//!
//! The split is by *who needs the fact*:
//!
//! - **here (Rust)** — identity (`id`, `name`, `description`), geometry (default
//!   size, resizability, the per-record clamp), the data a fresh widget starts
//!   with, and whether the kind stands for something on disk (`desktop_item`).
//!   The backend owns all of it: it writes the records, computes the hit rects
//!   and answers `floaty_plugins`.
//! - **`src/widgets/plugin.ts` (TypeScript)** — behaviour only: how to mount the
//!   widget, how to describe it in settings, which settings rows it draws, and
//!   its quick-add label. It reads everything above from this manifest through
//!   `floaty_plugins` (see `src/widgets/pluginManifest.ts`), so the numbers and
//!   names exist exactly once.
//!
//! Adding a plugin therefore costs three edits: one entry here, one module in
//! `src/widgets/`, one line in the `BUILTINS` list of `src/widgets/plugin.ts`.
//! `scripts/check-plugins.mjs` (part of `npm run build`) fails when the two id
//! lists disagree, and the tests below pin the invariants of this table.

use serde::{Deserialize, Serialize};

/// What a widget kind needs on top of the plain contract when it stands for a
/// file, folder or shortcut on disk: where its path lives in the record data,
/// the noun the removal dialog uses, and whether it groups other items.
pub struct DesktopItem {
    /// Record key holding the path: "target" for one item, "path" for a group.
    pub path_key: &'static str,
    /// Noun for the removal dialog ("app floatie", "folder floatie", ...).
    pub noun: &'static str,
    /// True for a folder-like group of items, false for a single path.
    pub group: bool,
}

/// One widget kind, as the backend sees it.
pub struct PluginDef {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Size a fresh widget gets, and the base `size` clamps from.
    pub default_size: (f64, f64),
    pub resizable: bool,
    /// Record data a fresh widget of this kind starts with.
    pub default_data: fn() -> serde_json::Value,
    /// Per-record size, e.g. the stored w/h clamped to what the widget allows.
    pub custom_size: Option<CustomSize>,
    /// Set for kinds that stand for something on disk.
    pub desktop_item: Option<DesktopItem>,
    /// Quick-add button label in "+ New floatie".
    pub add_label: Option<&'static str>,
    /// Overlay placement order, lower first (see `DEFAULT_LAYOUT_PRIORITY`).
    pub layout_priority: i32,
}

impl PluginDef {
    /// The size a record of this kind occupies on screen. Kinds with a clamp
    /// read the record's own w/h, the rest are fixed.
    pub fn size(&self, data: &serde_json::Value) -> (f64, f64) {
        match self.custom_size {
            Some(calc) => calc(self.default_size, data),
            None => self.default_size,
        }
    }

    /// Record data a fresh widget of this kind starts with.
    pub fn default_data(&self) -> serde_json::Value {
        (self.default_data)()
    }

    /// True when the kind points at a single path (a launch target) rather than
    /// a group of them.
    pub fn is_path_kind(&self) -> bool {
        matches!(&self.desktop_item, Some(d) if !d.group)
    }

    /// The frontend payload for this kind.
    pub fn info(&self, disabled: &[String]) -> PluginInfo {
        PluginInfo {
            id: self.id.to_string(),
            name: self.name.to_string(),
            description: self.description.to_string(),
            enabled: !disabled.iter().any(|d| d == self.id),
            default_size: [self.default_size.0, self.default_size.1],
            resizable: self.resizable,
            desktop_item: self.desktop_item.as_ref().map(|d| PluginDesktopItem {
                path_key: d.path_key.to_string(),
                noun: d.noun.to_string(),
                group: d.group,
            }),
            add_label: self.add_label.map(|l| l.to_string()),
            layout_priority: self.layout_priority,
            source: "builtin".to_string(),
            entry: None,
            version: String::new(),
            author: String::new(),
            api_version: PLUGIN_API_VERSION,
            trusted: true,
        }
    }
}

/// A record's size when its kind is not one this build knows — a store written
/// by a newer version, or a plugin that has since been removed. Such a record
/// still loads (removing a plugin must not lose the widget), it just gets a
/// neutral square.
pub const UNKNOWN_KIND_SIZE: (f64, f64) = (100.0, 100.0);

/// The size that goes with a kind, clamped from the record's own data when the
/// kind is resizable.
pub fn size(kind: &str, data: &serde_json::Value) -> (f64, f64) {
    if let Some(p) = installed(kind) {
        return p.size(data);
    }
    match find(kind) {
        Some(p) => p.size(data),
        None => UNKNOWN_KIND_SIZE,
    }
}

fn size_from_wh(
    base: (f64, f64),
    data: &serde_json::Value,
    min: (f64, f64),
    max: (f64, f64),
) -> (f64, f64) {
    let w = data.get("w").and_then(|v| v.as_f64()).unwrap_or(base.0);
    let h = data.get("h").and_then(|v| v.as_f64()).unwrap_or(base.1);
    (w.clamp(min.0, max.0), h.clamp(min.1, max.1))
}

pub const PLUGINS: &[PluginDef] = &[
    PluginDef {
        id: "note",
        name: "Note",
        description: "Sticky notes with autosave. Drag by the top bar, resize by the corner.",
        default_size: (300.0, 330.0),
        resizable: true,
        default_data: || serde_json::json!({ "text": "" }),
        custom_size: Some(|base, data| size_from_wh(base, data, (180.0, 140.0), (1400.0, 1400.0))),
        desktop_item: None,
        add_label: Some("+ note"),
        layout_priority: 2,
    },
    PluginDef {
        id: "clock",
        name: "Clock",
        description: "Clock plus pomodoro timer. Tap the timer to edit lengths.",
        default_size: (250.0, 330.0),
        resizable: true,
        default_data: || serde_json::json!({}),
        custom_size: Some(|base, data| size_from_wh(base, data, (200.0, 260.0), (1400.0, 1400.0))),
        desktop_item: None,
        add_label: Some("+ clock"),
        layout_priority: 3,
    },
    PluginDef {
        id: "pet",
        name: "Pet",
        description: "A wandering blob. Hover to calm it, double-click to freeze it.",
        default_size: (170.0, 170.0),
        resizable: false,
        default_data: || serde_json::json!({ "name": "bloop" }),
        custom_size: None,
        desktop_item: None,
        add_label: Some("+ pet"),
        layout_priority: 4,
    },
    PluginDef {
        id: "app",
        name: "App launcher",
        description: "Gravity icons for real apps. Pin, drop, launch; icons group into folders.",
        default_size: (92.0, 112.0),
        resizable: false,
        default_data: || serde_json::json!({ "name": "app", "target": "" }),
        custom_size: None,
        desktop_item: Some(DesktopItem { path_key: "target", noun: "app floatie", group: false }),
        add_label: None,
        layout_priority: 10,
    },
    PluginDef {
        id: "file",
        name: "File",
        description: "Loose desktop files (documents, images, archives). Double-click opens them with their default app.",
        default_size: (92.0, 112.0),
        resizable: false,
        default_data: || serde_json::json!({ "name": "file", "target": "" }),
        custom_size: None,
        desktop_item: Some(DesktopItem { path_key: "target", noun: "file floatie", group: false }),
        add_label: None,
        layout_priority: 10,
    },
    PluginDef {
        id: "folder",
        name: "Folder",
        description: "Groups of launchers. Click to expand into a launch grid.",
        default_size: (92.0, 112.0),
        resizable: false,
        default_data: || serde_json::json!({ "name": "Folder", "items": [] }),
        custom_size: None,
        desktop_item: Some(DesktopItem { path_key: "path", noun: "folder floatie", group: true }),
        add_label: Some("+ folder"),
        layout_priority: 10,
    },
    PluginDef {
        id: "live2d",
        name: "Live2D",
        description: "Animated Live2D companion. Drag it around, click it for motions.",
        default_size: (300.0, 400.0),
        resizable: false,
        default_data: || serde_json::json!({ "name": "Live2D" }),
        // The zoom grows the widget box with the model (the frontend also clamps
        // it to the desktop), so the record's w/h is the source of truth.
        custom_size: Some(|base, data| size_from_wh(base, data, (150.0, 200.0), (900.0, 1200.0))),
        desktop_item: None,
        add_label: Some("+ live2d"),
        layout_priority: 1,
    },
    PluginDef {
        id: "visualizer",
        name: "Audio visualizer",
        description: "Spectrum bars for whatever the machine is playing (system audio, not the mic). Click it to switch bars/wave/dots.",
        default_size: (280.0, 130.0),
        resizable: true,
        default_data: || serde_json::json!({ "name": "Visualizer", "mode": "bars" }),
        custom_size: Some(|base, data| size_from_wh(base, data, (140.0, 70.0), (1400.0, 900.0))),
        desktop_item: None,
        add_label: Some("+ visualizer"),
        layout_priority: 5,
    },
    PluginDef {
        id: "sysmon",
        name: "System monitor",
        description: "Live CPU, 3D-GPU and RAM load with a scrolling graph. Click it to graph a different metric.",
        default_size: (250.0, 170.0),
        resizable: true,
        default_data: || serde_json::json!({ "name": "System monitor", "graph": "gpu" }),
        custom_size: Some(|base, data| size_from_wh(base, data, (180.0, 110.0), (900.0, 700.0))),
        desktop_item: None,
        add_label: Some("+ system monitor"),
        layout_priority: 5,
    },
];

pub fn find(kind: &str) -> Option<&'static PluginDef> {
    PLUGINS.iter().find(|p| p.id == kind)
}

/// The plugin format this build understands; `apiVersion` in `plugin.json`.
///
/// Additive: 2 adds the notification, timer and event members to the widget api,
/// and a manifest that says 1 still loads with exactly the old surface. Bumping
/// this is how a *plugin* can tell which build it is running on.
pub const PLUGIN_API_VERSION: u32 = 2;

/// The oldest `apiVersion` still accepted.
pub const PLUGIN_API_MIN: u32 = 1;

/// Folder users drop plugins into, under the app data directory.
pub const PLUGINS_DIR_NAME: &str = "plugins";

/// Priority of the icon/folder grid: anything hand-placed sorts before it.
pub const DEFAULT_LAYOUT_PRIORITY: i32 = 10;

/// A plugin installed by the user: the same facts as a built-in, read from
/// `<plugins dir>/<id>/plugin.json` at startup instead of compiled in. The
/// JavaScript half sits next to it as an ES module the windows import.
#[derive(Debug, Clone)]
pub struct InstalledPlugin {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: String,
    /// The `apiVersion` the manifest declared. Always the one this build speaks,
    /// because `read_plugin` refuses anything else — kept so the settings window can
    /// say which contract a plugin was written against.
    pub api_version: u32,
    pub default_size: (f64, f64),
    pub resizable: bool,
    pub min_size: Option<(f64, f64)>,
    pub max_size: Option<(f64, f64)>,
    pub default_data: serde_json::Value,
    pub desktop_item: Option<PluginDesktopItem>,
    pub add_label: Option<String>,
    pub layout_priority: i32,
    /// Absolute path of the module the windows import.
    pub entry: std::path::PathBuf,
}

impl InstalledPlugin {
    pub fn size(&self, data: &serde_json::Value) -> (f64, f64) {
        if !self.resizable {
            return self.default_size;
        }
        let w = data.get("w").and_then(|v| v.as_f64()).unwrap_or(self.default_size.0);
        let h = data.get("h").and_then(|v| v.as_f64()).unwrap_or(self.default_size.1);
        let min = self.min_size.unwrap_or((80.0, 60.0));
        let max = self.max_size.unwrap_or((2000.0, 2000.0));
        (w.clamp(min.0, max.0), h.clamp(min.1, max.1))
    }
}

static INSTALLED: std::sync::RwLock<Vec<InstalledPlugin>> = std::sync::RwLock::new(Vec::new());

fn installed(kind: &str) -> Option<InstalledPlugin> {
    INSTALLED.read().ok()?.iter().find(|p| p.id == kind).cloned()
}

/// The plugins the user has installed, as loaded at startup.
pub fn installed_plugins() -> Vec<InstalledPlugin> {
    INSTALLED.read().map(|g| g.clone()).unwrap_or_default()
}

pub fn exists(kind: &str) -> bool {
    find(kind).is_some() || installed(kind).is_some()
}

pub fn resizable(kind: &str) -> bool {
    if let Some(p) = installed(kind) {
        return p.resizable;
    }
    find(kind).map(|p| p.resizable).unwrap_or(false)
}

/// True when the kind points at a single path on disk (an app or a loose file).
pub fn is_path_kind(kind: &str) -> bool {
    if let Some(p) = installed(kind) {
        return matches!(&p.desktop_item, Some(d) if !d.group);
    }
    find(kind).map(|p| p.is_path_kind()).unwrap_or(false)
}

/// True when the kind stands for something on disk at all: those get the shell
/// verbs and a Recycle Bin delete.
pub fn is_desktop_item(kind: &str) -> bool {
    if let Some(p) = installed(kind) {
        return p.desktop_item.is_some();
    }
    find(kind).map(|p| p.desktop_item.is_some()).unwrap_or(false)
}

/// The record key a desktop item keeps its path in, or None for other kinds.
pub fn path_key(kind: &str) -> Option<String> {
    if let Some(p) = installed(kind) {
        return p.desktop_item.map(|d| d.path_key);
    }
    find(kind).and_then(|p| p.desktop_item.as_ref().map(|d| d.path_key.to_string()))
}

pub fn default_data(kind: &str) -> serde_json::Value {
    if let Some(p) = installed(kind) {
        return p.default_data.clone();
    }
    find(kind)
        .map(|p| p.default_data())
        .unwrap_or_else(|| serde_json::json!({}))
}

/// Every plugin with its enabled state: built-ins in declaration order, then the
/// ones the user installed, so the settings list stays stable.
pub fn manifest(disabled: &[String]) -> Vec<PluginInfo> {
    let mut out: Vec<PluginInfo> = PLUGINS.iter().map(|p| p.info(disabled)).collect();
    for p in installed_plugins() {
        out.push(PluginInfo {
            id: p.id.clone(),
            name: p.name.clone(),
            description: p.description.clone(),
            enabled: !disabled.iter().any(|d| d == &p.id),
            default_size: [p.default_size.0, p.default_size.1],
            resizable: p.resizable,
            desktop_item: p.desktop_item.clone(),
            add_label: p.add_label.clone(),
            layout_priority: p.layout_priority,
            source: "installed".to_string(),
            entry: Some(p.entry.to_string_lossy().to_string()),
            version: p.version.clone(),
            author: p.author.clone(),
            api_version: p.api_version,
            trusted: true,
        });
    }
    out
}

/// The plugin's `id`, or why it is not usable: 2-32 characters of `a-z`, `0-9`,
/// `-` and `_`, starting with a letter or a digit, and never a built-in's.
///
/// Built-ins are refused here rather than merged over later, because a plugin
/// sharing a built-in's id would have its records read as the built-in's.
fn validate_plugin_id(json: &serde_json::Value) -> Result<String, String> {
    let id = json
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !valid_id(&id) {
        return Err(format!("id {id:?} must be 2-32 chars of a-z, 0-9, '-' or '_'"));
    }
    if find(&id).is_some() {
        return Err(format!("id {id:?} is a built-in plugin"));
    }
    Ok(id)
}

/// Where a widget kind answers with its own size: width, height, and the min/max it allows.
type CustomSize = fn((f64, f64), &serde_json::Value) -> (f64, f64);
/// `(default, min, max)` as a plugin manifest declares them, before the kind's own rules apply.
type PluginSizes = ((f64, f64), Option<(f64, f64)>, Option<(f64, f64)>);

fn validate_plugin_sizes(json: &serde_json::Value) -> Result<PluginSizes, String> {
    let size = json.get("size").ok_or("size { w, h } is required")?;
    let w = size.get("w").and_then(|v| v.as_f64()).unwrap_or_default();
    let h = size.get("h").and_then(|v| v.as_f64()).unwrap_or_default();
    if !(60.0..=4000.0).contains(&w) || !(60.0..=4000.0).contains(&h) {
        return Err(format!("size {w}x{h} is outside 60..4000"));
    }
    let pair = |key: &str| -> Option<(f64, f64)> {
        let v = json.get(key)?;
        Some((v.get("w")?.as_f64()?, v.get("h")?.as_f64()?))
    };
    let min_size = pair("minSize");
    let max_size = pair("maxSize");
    if let (Some(a), Some(b)) = (min_size, max_size) {
        if a.0 > b.0 || a.1 > b.1 {
            return Err("minSize is larger than maxSize".into());
        }
    }
    Ok(((w, h), min_size, max_size))
}

fn validate_entry_path(dir: &std::path::Path, json: &serde_json::Value) -> Result<std::path::PathBuf, String> {
    let entry_rel = json.get("entry").and_then(|v| v.as_str()).unwrap_or("index.js");
    let entry_rel_path = std::path::Path::new(entry_rel);
    if entry_rel_path.is_absolute()
        || entry_rel_path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        })
    {
        return Err(format!(
            "entry {entry_rel:?} must be a relative path inside the plugin folder"
        ));
    }
    let entry = dir.join(entry_rel_path);
    if !entry.is_file() {
        return Err(format!("entry {entry_rel:?} does not exist in the plugin folder"));
    }
    Ok(entry)
}

fn parse_desktop_item(json: &serde_json::Value, name: &str) -> Option<PluginDesktopItem> {
    json.get("desktopItem")
        .and_then(|v| v.as_object())
        .map(|d| PluginDesktopItem {
            path_key: d
                .get("pathKey")
                .and_then(|v| v.as_str())
                .unwrap_or("target")
                .to_string(),
            noun: d
                .get("noun")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("{name} floatie")),
            group: d.get("group").and_then(|v| v.as_bool()).unwrap_or(false),
        })
}

/// How `incoming` compares to what is already installed: `"new"`, `"same"`,
/// `"upgrade"`, `"downgrade"` or `"unknown"` when either version is not a version.
///
/// Numeric fields, so 1.10 is newer than 1.9 — a string compare gets that wrong, and
/// getting it wrong is how an update silently goes backwards. Anything that does not
/// parse is `"unknown"` and left for the person to judge, which is why the caller
/// shows both versions and not only a verdict.
pub fn version_relation(installed: Option<&str>, incoming: &str) -> &'static str {
    let Some(installed) = installed else {
        return "new";
    };
    let parse = |text: &str| -> Option<Vec<u64>> {
        let trimmed = text.trim().trim_start_matches('v');
        if trimmed.is_empty() {
            return None;
        }
        trimmed
            .split('.')
            .map(|part| {
                let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
                if digits.is_empty() {
                    None
                } else {
                    digits.parse::<u64>().ok()
                }
            })
            .collect()
    };
    let (Some(a), Some(b)) = (parse(installed), parse(incoming)) else {
        return "unknown";
    };
    if a == b {
        "same"
    } else if b > a {
        "upgrade"
    } else {
        "downgrade"
    }
}

/// Read and validate one plugin folder.
///
/// Pure: no global state, so the tests can point it at a fixture directory.
/// Everything a broken or hostile plugin could get wrong is checked here — the
/// id, the API version, the entry path staying inside the folder, the sizes.
pub fn read_plugin(dir: &std::path::Path) -> Result<InstalledPlugin, String> {
    let manifest_path = dir.join("plugin.json");
    let text =
        std::fs::read_to_string(&manifest_path).map_err(|e| format!("plugin.json unreadable: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("plugin.json is not valid JSON: {e}"))?;

    let id = validate_plugin_id(&json)?;

    let api = json.get("apiVersion").and_then(|v| v.as_u64()).unwrap_or_default() as u32;
    // A range, not an equality: the format is additive, so every older apiVersion
    // still loads with the surface it was written against, and only a *newer*
    // manifest (or a missing/garbage one) is refused.
    if !(PLUGIN_API_MIN..=PLUGIN_API_VERSION).contains(&api) {
        return Err(format!(
            "apiVersion {api} is not supported (this build speaks {PLUGIN_API_MIN}..{PLUGIN_API_VERSION})"
        ));
    }
    let name = json
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if name.is_empty() {
        return Err("name is required".into());
    }

    let (default_size, min_size, max_size) = validate_plugin_sizes(&json)?;
    let entry = validate_entry_path(dir, &json)?;

    let default_data = json
        .get("defaultData")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if !default_data.is_object() {
        return Err("defaultData must be an object".into());
    }

    let desktop_item = parse_desktop_item(&json, &name);

    Ok(InstalledPlugin {
        name,
        description: json
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        version: json.get("version").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
        author: json.get("author").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
        api_version: api,
        default_size,
        resizable: json.get("resizable").and_then(|v| v.as_bool()).unwrap_or(false),
        min_size,
        max_size,
        default_data,
        desktop_item,
        add_label: json.get("addLabel").and_then(|v| v.as_str()).map(|s| s.to_string()),
        layout_priority: json
            .get("layoutPriority")
            .and_then(|v| v.as_i64())
            .unwrap_or(DEFAULT_LAYOUT_PRIORITY as i64) as i32,
        entry,
        id,
    })
}

fn valid_id(id: &str) -> bool {
    let len = id.len();
    (2..=32).contains(&len)
        && id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn list_plugin_dirs(dir: &std::path::Path) -> Result<Vec<std::path::PathBuf>, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("{}: cannot create the plugins folder: {e}", dir.display()))?;
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: cannot read the plugins folder: {e}", dir.display()))?;
    let mut dirs: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    Ok(dirs)
}

/// Load every plugin folder in `dir` (created if missing).
///
/// Returns `(accepted descriptions, rejections)`. A broken plugin is reported,
/// never fatal: one bad folder must not cost the user their desktop.
pub fn install_from(dir: &std::path::Path) -> (Vec<String>, Vec<String>) {
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();
    let mut loaded: Vec<InstalledPlugin> = Vec::new();

    let dirs = match list_plugin_dirs(dir) {
        Ok(list) => list,
        Err(err) => return (accepted, vec![err]),
    };

    for plugin_dir in dirs {
        match read_plugin(&plugin_dir) {
            Ok(plugin) => {
                if loaded.iter().any(|p: &InstalledPlugin| p.id == plugin.id) {
                    rejected.push(format!("{}: duplicate id", plugin.id));
                    continue;
                }
                let version = if plugin.version.is_empty() {
                    String::new()
                } else {
                    format!(" v{}", plugin.version)
                };
                accepted.push(format!("{}{}", plugin.id, version));
                loaded.push(plugin);
            }
            Err(reason) => {
                let name = plugin_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                rejected.push(format!("{name}: {reason}"));
            }
        }
    }

    if let Ok(mut guard) = INSTALLED.write() {
        *guard = loaded;
    }
    (accepted, rejected)
}

/// One plugin as the windows see it, built-in or installed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    /// Fresh-widget size, so the frontend never repeats the numbers.
    pub default_size: [f64; 2],
    pub resizable: bool,
    pub desktop_item: Option<PluginDesktopItem>,
    /// Quick-add button label, or null for "no button in + New floatie".
    pub add_label: Option<String>,
    pub layout_priority: i32,
    /// "builtin" or "installed".
    pub source: String,
    /// Absolute path of the module to import (installed plugins only).
    pub entry: Option<String>,
    pub version: String,
    pub author: String,
    /// The plugin contract this kind was written against.
    pub api_version: u32,
    /// Whether the user has approved this code. Built-ins always are; an installed
    /// plugin is approved when it is installed from an archive, or from the
    /// Plugins tab afterwards, and its widgets do not mount until then. Stamped by
    /// the command, because only it has the settings to compare fingerprints with.
    pub trusted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDesktopItem {
    pub path_key: String,
    pub noun: String,
    pub group: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    const KINDS: [&str; 9] = [
        "note",
        "clock",
        "pet",
        "app",
        "file",
        "folder",
        "live2d",
        "visualizer",
        "sysmon",
    ];

    #[test]
    fn every_kind_is_present_once() {
        for id in KINDS {
            assert!(exists(id), "plugin {id} should exist");
            let p = find(id).unwrap();
            assert_eq!(p.id, id);
            assert!(!p.name.is_empty() && !p.description.is_empty());
        }
        assert_eq!(
            PLUGINS.len(),
            KINDS.len(),
            "manifest has an unlisted plugin"
        );
        assert!(!exists("non_existent"));
    }

    #[test]
    fn desktop_items_are_the_kinds_that_live_on_disk() {
        // The ids the desktop scanner produces (kind_for_path) and the kinds the
        // shell verbs run on; this pins the coupling in one place.
        let desktop: Vec<&str> = PLUGINS
            .iter()
            .filter(|p| p.desktop_item.is_some())
            .map(|p| p.id)
            .collect();
        assert_eq!(desktop, ["app", "file", "folder"]);
        assert!(is_path_kind("app") && is_path_kind("file"));
        assert!(!is_path_kind("folder") && !is_path_kind("note"));
        assert!(is_desktop_item("folder") && !is_desktop_item("note"));
        assert_eq!(path_key("folder").as_deref(), Some("path"));
        assert_eq!(path_key("app").as_deref(), Some("target"));
        assert_eq!(path_key("note"), None);
        for p in PLUGINS.iter().filter(|p| p.desktop_item.is_some()) {
            let d = p.desktop_item.as_ref().unwrap();
            assert!(d.noun.ends_with("floatie"), "{}: odd noun {}", p.id, d.noun);
        }
    }

    #[test]
    fn sizes_are_the_defaults_and_clamp_the_record() {
        let empty = serde_json::json!({});
        for p in PLUGINS {
            assert_eq!(size(p.id, &empty), p.default_size, "{} default size", p.id);
            let (w, h) = size(p.id, &serde_json::json!({ "w": 1e9, "h": 1e9 }));
            assert_eq!(size(p.id, &serde_json::json!({ "w": w, "h": h })), (w, h));
            assert!(
                w >= 92.0 && h >= 70.0,
                "{} clamps to something usable",
                p.id
            );
        }
        assert_eq!(
            size("note", &serde_json::json!({ "w": 10.0, "h": 5000.0 })),
            (180.0, 1400.0)
        );
        assert_eq!(
            size("live2d", &serde_json::json!({ "w": 499.0, "h": 665.0 })),
            (499.0, 665.0)
        );
        assert_eq!(
            size("live2d", &serde_json::json!({ "w": 10.0, "h": 10.0 })),
            (150.0, 200.0)
        );
        // unknown kinds keep a neutral square instead of panicking
        assert_eq!(size("gone", &empty), UNKNOWN_KIND_SIZE);
        assert_eq!(default_data("gone"), serde_json::json!({}));
    }

    #[test]
    fn a_fresh_record_starts_from_the_manifest() {
        for p in PLUGINS {
            let data = default_data(p.id);
            assert!(data.is_object(), "{} default data", p.id);
            if let Some(key) = path_key(p.id) {
                // fresh app/file records carry an empty target; a folder only
                // gets its path once it is created or grouped
                let value = data.get(key).and_then(|v| v.as_str());
                assert!(
                    value.map_or(true, |v| v.is_empty()),
                    "{} starts without a path",
                    p.id
                );
            }
        }
    }

    #[test]
    fn versions_are_compared_as_numbers_not_as_text() {
        assert_eq!(version_relation(None, "1.0.0"), "new");
        assert_eq!(version_relation(Some("1.0.0"), "1.0.0"), "same");
        assert_eq!(version_relation(Some("1.0.0"), "1.10.0"), "upgrade");
        assert_eq!(version_relation(Some("1.10.0"), "1.9.0"), "downgrade");
        assert_eq!(version_relation(Some("v2"), "2.1"), "upgrade");
        assert_eq!(version_relation(Some(""), "1.0"), "unknown");
        assert_eq!(version_relation(Some("abc"), "1.0"), "unknown");
        assert_eq!(version_relation(Some("1.0"), "beta"), "unknown");
    }

    #[test]
    fn manifest_reports_the_enabled_state() {
        let disabled = vec!["live2d".to_string(), "pet".to_string()];
        let info = manifest(&disabled);
        let builtin: Vec<&PluginInfo> = info.iter().filter(|p| p.source == "builtin").collect();
        assert_eq!(builtin.len(), PLUGINS.len());
        for p in info {
            assert_eq!(p.enabled, !disabled.contains(&p.id));
            assert_eq!(p.default_size.len(), 2);
        }
        let live2d = find("live2d").unwrap();
        let payload = live2d.info(&disabled);
        assert!(!payload.enabled && payload.resizable == live2d.resizable);
        assert!(payload.desktop_item.is_none());
        let folder = find("folder").unwrap().info(&[]);
        let d = folder.desktop_item.unwrap();
        assert_eq!((d.path_key.as_str(), d.group), ("path", true));
    }

    /// The frontend reads exactly these keys (`src/widgets/pluginManifest.ts`).
    /// Renaming a field here without renaming it there used to be a silent
    /// runtime failure — the sizes would quietly fall back to 100x100.
    #[test]
    fn the_payload_carries_the_keys_the_frontend_reads() {
        let folder = serde_json::to_value(find("folder").unwrap().info(&[])).unwrap();
        assert_eq!(folder["id"], "folder");
        assert_eq!(folder["default_size"], serde_json::json!([92.0, 112.0]));
        assert_eq!(folder["resizable"], false);
        assert_eq!(folder["desktop_item"]["path_key"], "path");
        assert_eq!(folder["desktop_item"]["noun"], "folder floatie");
        assert_eq!(folder["desktop_item"]["group"], true);
        assert!(folder.get("defaultSize").is_none());

        let note = serde_json::to_value(find("note").unwrap().info(&["note".to_string()])).unwrap();
        assert_eq!(note["default_size"], serde_json::json!([300.0, 330.0]));
        assert_eq!(note["resizable"], true);
        assert!(note["desktop_item"].is_null());
        assert_eq!(note["enabled"], false);
        assert_eq!(note["add_label"], "+ note");
        assert_eq!(note["layout_priority"], 2);
        assert_eq!(note["source"], "builtin");
        assert!(note["entry"].is_null());
    }

    // ---------- user-installed plugins ----------

    fn fixture(name: &str, manifest: &str, entry: Option<&str>) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("floaty-plugin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.json"), manifest).unwrap();
        if let Some(source) = entry {
            std::fs::write(dir.join("index.js"), source).unwrap();
        }
        dir
    }

    const GOOD: &str = r#"{
        "id": "countdown",
        "name": "Countdown",
        "description": "Counts down to a moment.",
        "version": "1.2.0",
        "author": "someone",
        "apiVersion": 1,
        "size": { "w": 240, "h": 140 },
        "resizable": true,
        "minSize": { "w": 160, "h": 100 },
        "maxSize": { "w": 600, "h": 400 },
        "defaultData": { "seconds": 300 },
        "addLabel": "+ countdown",
        "layoutPriority": 6
    }"#;

    #[test]
    fn a_manifest_may_speak_an_older_api_version_but_not_a_newer_one() {
        // The format is additive, so an older manifest still loads — that is the
        // whole reason the range exists and the reason the bump did not break the
        // examples in `examples/plugins`.
        for version in [PLUGIN_API_MIN, PLUGIN_API_VERSION] {
            let manifest = GOOD.replace("\"apiVersion\": 1", &format!("\"apiVersion\": {version}"));
            let dir = fixture(&format!("api{version}"), &manifest, Some("export default {};"));
            let plugin = read_plugin(&dir).unwrap_or_else(|e| panic!("apiVersion {version}: {e}"));
            assert_eq!(plugin.api_version, version);
        }
        // A newer one is refused, and the sentence says which range this build has.
        let ahead = GOOD.replace("\"apiVersion\": 1", "\"apiVersion\": 99");
        let dir = fixture("api99", &ahead, Some("export default {};"));
        let err = read_plugin(&dir).unwrap_err();
        assert!(err.contains("99"), "{err}");
        assert!(
            err.contains(&format!("{PLUGIN_API_MIN}..{PLUGIN_API_VERSION}")),
            "the refusal has to name the range: {err}"
        );
        // And a missing or zero apiVersion is not "the oldest one", it is a broken
        // manifest: reading it as v1 would let a typo skip the contract entirely.
        let none = GOOD.replace("\"apiVersion\": 1,", "");
        let dir = fixture("apinone", &none, Some("export default {};"));
        assert!(read_plugin(&dir).is_err(), "a manifest with no apiVersion");
    }

    #[test]
    fn a_valid_plugin_folder_reads_back() {
        let dir = fixture("good", GOOD, Some("export default { mount() {} };"));
        let p = read_plugin(&dir).unwrap();
        assert_eq!(p.id, "countdown");
        assert_eq!(p.name, "Countdown");
        assert_eq!(p.version, "1.2.0");
        assert_eq!(p.default_size, (240.0, 140.0));
        assert!(p.resizable);
        assert_eq!(p.min_size, Some((160.0, 100.0)));
        assert_eq!(p.default_data, serde_json::json!({ "seconds": 300 }));
        assert_eq!(p.add_label.as_deref(), Some("+ countdown"));
        assert_eq!(p.layout_priority, 6);
        assert_eq!(p.entry, dir.join("index.js"));
        assert!(p.entry.is_file());
        // the size a record gets: the stored w/h inside the clamp
        assert_eq!(p.size(&serde_json::json!({})), (240.0, 140.0));
        assert_eq!(p.size(&serde_json::json!({ "w": 5000.0, "h": 10.0 })), (600.0, 100.0));
        // a fixed-size plugin ignores the record
        let fixed = fixture("fixed", &GOOD.replace("\"resizable\": true", "\"resizable\": false"), Some("x"));
        assert_eq!(read_plugin(&fixed).unwrap().size(&serde_json::json!({ "w": 999.0 })), (240.0, 140.0));
    }

    #[test]
    fn broken_plugin_folders_are_rejected_with_a_reason() {
        let cases: [(&str, &str, Option<&str>, &str); 11] = [
            ("no-id", r#"{ "name": "x", "apiVersion": 1, "size": {"w":100,"h":100} }"#, Some("x"), "id"),
            ("bad-id", r#"{ "id": "Bad Id", "name": "x", "apiVersion": 1, "size": {"w":100,"h":100} }"#, Some("x"), "id"),
            ("builtin-id", r#"{ "id": "note", "name": "x", "apiVersion": 1, "size": {"w":100,"h":100} }"#, Some("x"), "built-in"),
            // a *newer* apiVersion than this build speaks is the refusal; an older
            // one is not (see `a_manifest_may_speak_an_older_api_version_...`)
            ("new-api", r#"{ "id": "sample", "name": "x", "apiVersion": 99, "size": {"w":100,"h":100} }"#, Some("x"), "not supported"),
            ("no-name", r#"{ "id": "sample", "apiVersion": 1, "size": {"w":100,"h":100} }"#, Some("x"), "name is required"),
            ("no-size", r#"{ "id": "sample", "name": "x", "apiVersion": 1 }"#, Some("x"), "size"),
            ("tiny-size", r#"{ "id": "sample", "name": "x", "apiVersion": 1, "size": {"w":10,"h":4000} }"#, Some("x"), "outside"),
            ("min-over-max", r#"{ "id": "sample", "name": "x", "apiVersion": 1, "size": {"w":200,"h":200}, "minSize": {"w":300,"h":100}, "maxSize": {"w":200,"h":200} }"#, Some("x"), "minSize"),
            ("no-entry-file", r#"{ "id": "sample", "name": "x", "apiVersion": 1, "size": {"w":100,"h":100} }"#, None, "does not exist"),
            ("escaping-entry", r#"{ "id": "sample", "name": "x", "apiVersion": 1, "size": {"w":100,"h":100}, "entry": "../evil.js" }"#, Some("x"), "relative path"),
            ("bad-data", r#"{ "id": "sample", "name": "x", "apiVersion": 1, "size": {"w":100,"h":100}, "defaultData": 7 }"#, Some("x"), "defaultData"),
        ];
        for (name, manifest, entry, expected) in cases {
            let dir = fixture(name, manifest, entry);
            let err = read_plugin(&dir).expect_err(name).to_string();
            assert!(
                err.contains(expected),
                "{name}: expected {expected:?} in {err:?}"
            );
        }
    }

    #[test]
    fn install_from_keeps_the_good_folders_and_names_the_bad_ones() {
        let root = std::env::temp_dir().join(format!("floaty-plugins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("countdown")).unwrap();
        std::fs::write(root.join("countdown/plugin.json"), GOOD).unwrap();
        std::fs::write(root.join("countdown/index.js"), "export default {};").unwrap();
        std::fs::create_dir_all(root.join("broken")).unwrap();
        std::fs::write(root.join("broken/plugin.json"), "{ not json").unwrap();

        let (accepted, rejected) = install_from(&root);
        assert_eq!(accepted, vec!["countdown v1.2.0"]);
        assert_eq!(rejected.len(), 1);
        assert!(rejected[0].contains("broken"), "{rejected:?}");

        // the installed plugin answers every question the backend asks a kind
        assert!(exists("countdown"));
        assert!(resizable("countdown"));
        assert_eq!(size("countdown", &serde_json::json!({})), (240.0, 140.0));
        assert_eq!(default_data("countdown")["seconds"], 300);
        assert!(!is_desktop_item("countdown") && path_key("countdown").is_none());

        let listed = manifest(&[]);
        let entry = listed.iter().find(|p| p.id == "countdown").unwrap();
        assert_eq!(entry.source, "installed");
        assert_eq!(entry.default_size, [240.0, 140.0]);
        assert_eq!(entry.version, "1.2.0");
        assert_eq!(entry.author, "someone");
        assert_eq!(entry.add_label.as_deref(), Some("+ countdown"));
        assert_eq!(entry.layout_priority, 6);
        assert!(entry.resizable);
        assert!(entry.entry.as_ref().unwrap().ends_with("index.js"));

        // unloading leaves the built-ins alone
        let _ = install_from(&root.join("empty"));
        assert!(!exists("countdown") && exists("note"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
