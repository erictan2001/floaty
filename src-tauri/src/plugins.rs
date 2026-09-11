use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
}

pub struct PluginDef {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub default_size: (f64, f64),
    pub resizable: bool,
    pub default_data: fn() -> serde_json::Value,
    pub custom_size: Option<fn(base: (f64, f64), data: &serde_json::Value) -> (f64, f64)>,
}

pub const PLUGINS: &[PluginDef] = &[
    PluginDef {
        id: "note",
        name: "Note",
        description: "Sticky notes with autosave. Drag by the top bar, resize by the corner.",
        default_size: (300.0, 330.0),
        resizable: true,
        default_data: || serde_json::json!({ "text": "" }),
        custom_size: Some(|base, data| {
            let w = data.get("w").and_then(|v| v.as_f64()).unwrap_or(base.0);
            let h = data.get("h").and_then(|v| v.as_f64()).unwrap_or(base.1);
            (w.clamp(180.0, 1400.0), h.clamp(140.0, 1400.0))
        }),
    },
    PluginDef {
        id: "clock",
        name: "Clock",
        description: "Clock plus pomodoro timer. Tap the timer to edit lengths.",
        default_size: (250.0, 330.0),
        resizable: true,
        default_data: || serde_json::json!({}),
        custom_size: Some(|base, data| {
            let w = data.get("w").and_then(|v| v.as_f64()).unwrap_or(base.0);
            let h = data.get("h").and_then(|v| v.as_f64()).unwrap_or(base.1);
            (w.clamp(200.0, 1400.0), h.clamp(260.0, 1400.0))
        }),
    },
    PluginDef {
        id: "pet",
        name: "Pet",
        description: "A wandering blob. Hover to calm it, double-click to freeze it.",
        default_size: (170.0, 170.0),
        resizable: false,
        default_data: || serde_json::json!({ "name": "bloop" }),
        custom_size: None,
    },
    PluginDef {
        id: "app",
        name: "App launcher",
        description: "Gravity icons for real apps. Pin, drop, launch; icons group into folders.",
        default_size: (92.0, 112.0),
        resizable: false,
        default_data: || serde_json::json!({ "name": "app", "target": "" }),
        custom_size: None,
    },
    PluginDef {
        id: "folder",
        name: "Folder",
        description: "Groups of launchers. Click to expand into a launch grid.",
        default_size: (92.0, 112.0),
        resizable: false,
        default_data: || serde_json::json!({ "name": "Folder", "items": [] }),
        custom_size: None,
    },
    PluginDef {
        id: "live2d",
        name: "Live2D",
        description: "Animated Live2D companion. Drag it around, click it for motions.",
        default_size: (300.0, 400.0),
        resizable: false,
        default_data: || serde_json::json!({ "name": "Live2D" }),
        custom_size: None,
    },
];

pub fn find_plugin(kind: &str) -> Option<&'static PluginDef> {
    PLUGINS.iter().find(|p| p.id == kind)
}

pub fn is_valid_kind(kind: &str) -> bool {
    find_plugin(kind).is_some()
}

pub fn widget_size(kind: &str, data: &serde_json::Value) -> (f64, f64) {
    if let Some(p) = find_plugin(kind) {
        if let Some(calc) = p.custom_size {
            calc(p.default_size, data)
        } else {
            p.default_size
        }
    } else {
        (300.0, 330.0)
    }
}

pub fn is_resizable(kind: &str) -> bool {
    find_plugin(kind).map(|p| p.resizable).unwrap_or(false)
}

pub fn default_data(kind: &str) -> serde_json::Value {
    find_plugin(kind)
        .map(|p| (p.default_data)())
        .unwrap_or_else(|| serde_json::json!({}))
}

pub fn all_plugin_info(disabled: &[String]) -> Vec<PluginInfo> {
    PLUGINS
        .iter()
        .map(|p| PluginInfo {
            id: p.id.to_string(),
            name: p.name.to_string(),
            description: p.description.to_string(),
            enabled: !disabled.iter().any(|d| d == p.id),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_all_known_plugins() {
        let expected = ["note", "clock", "pet", "app", "folder", "live2d"];
        for id in &expected {
            assert!(is_valid_kind(id), "plugin {} should exist", id);
            let p = find_plugin(id).unwrap();
            assert_eq!(p.id, *id);
            assert!(!p.name.is_empty());
        }
        assert!(!is_valid_kind("non_existent"));
    }

    #[test]
    fn test_widget_sizes() {
        let empty = serde_json::json!({});
        assert_eq!(widget_size("pet", &empty), (170.0, 170.0));
        assert_eq!(widget_size("app", &empty), (92.0, 112.0));
        assert_eq!(widget_size("folder", &empty), (92.0, 112.0));
        assert_eq!(widget_size("live2d", &empty), (300.0, 400.0));

        // Note custom sizing with clamp
        let note_custom = serde_json::json!({ "w": 500.0, "h": 600.0 });
        assert_eq!(widget_size("note", &note_custom), (500.0, 600.0));
        let note_clamped = serde_json::json!({ "w": 10.0, "h": 5000.0 });
        assert_eq!(widget_size("note", &note_clamped), (180.0, 1400.0));
    }

    #[test]
    fn test_plugin_info_disabled() {
        let disabled = vec!["live2d".to_string(), "pet".to_string()];
        let info = all_plugin_info(&disabled);
        assert_eq!(info.len(), PLUGINS.len());
        for p in info {
            if p.id == "live2d" || p.id == "pet" {
                assert!(!p.enabled);
            } else {
                assert!(p.enabled);
            }
        }
    }
}
