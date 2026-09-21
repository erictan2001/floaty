//! `system_monitor` — moved out of lib.rs verbatim.
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

// ---------- system monitor ----------

/// Claim system sampling (ref-counted, started by the first sysmon widget).
#[tauri::command]
pub(crate) fn floaty_sysmon_start(interval_ms: Option<u32>, app: AppHandle) {
    sysmon::start(&app, interval_ms);
}

/// Release one sysmon widget's claim on the sampler.
#[tauri::command]
pub(crate) fn floaty_sysmon_stop() {
    sysmon::stop();
}

#[tauri::command]
pub(crate) fn floaty_sysmon_set_interval(ms: u32) {
    sysmon::set_interval(ms);
}

/// Number of samples the widget graph keeps — one source of truth.
#[tauri::command]
pub(crate) fn floaty_sysmon_history() -> usize {
    sysmon::HISTORY
}

/// False when the sampler could not start (e.g. no GPU performance counters),
/// which lets the widget say so instead of showing dashes forever.
#[tauri::command]
pub(crate) fn floaty_sysmon_status() -> bool {
    sysmon::is_running()
}
