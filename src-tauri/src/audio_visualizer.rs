//! `audio_visualizer` — moved out of lib.rs verbatim.
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

// ---------- audio visualizer ----------

/// Claim the system-audio capture (ref-counted, started by the first
/// visualizer widget). `fps` is the redraw rate the widget wants.
#[tauri::command]
pub(crate) fn floaty_audio_start(fps: Option<u32>, app: AppHandle) {
    audio::start(&app, fps);
}

/// Release one visualizer's claim on the capture thread.
#[tauri::command]
pub(crate) fn floaty_audio_stop() {
    audio::stop();
}

#[tauri::command]
pub(crate) fn floaty_audio_set_fps(fps: u32) {
    audio::set_fps(fps);
}

/// False when loopback capture could not start, so the visualizer can say so
/// instead of sitting there as a flat line.
#[tauri::command]
pub(crate) fn floaty_audio_status() -> bool {
    audio::is_running()
}
