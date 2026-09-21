//! `folders` — moved out of lib.rs verbatim.
//!
//! Visibility is `pub(crate)` because the rest of the crate calls in across the module
//! boundary now; nothing here changed shape on the way out.

#![allow(unused_imports)]
use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

// ---------- folders ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct FolderItem {
    pub(crate) name: String,
    pub(crate) target: String,
    #[serde(default)]
    pub(crate) icon: String,
    #[serde(default)]
    pub(crate) is_dir: bool,
}

pub(crate) fn folder_items(rec: &WidgetRecord) -> Vec<FolderItem> {
    rec.data
        .get("items")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

pub(crate) fn set_folder_items(rec: &mut WidgetRecord, items: &[FolderItem]) {
    if let Some(obj) = rec.data.as_object_mut() {
        obj.insert(
            "items".to_string(),
            serde_json::to_value(items).unwrap_or(serde_json::Value::Null),
        );
    }
}

pub(crate) fn widget_as_folder_item(rec: &WidgetRecord) -> Option<FolderItem> {
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

pub(crate) fn unique_dest_path(dest_dir: &std::path::Path, file_name: &std::ffi::OsStr) -> std::path::PathBuf {
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
