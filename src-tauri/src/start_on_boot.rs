//! `start_on_boot` — moved out of lib.rs verbatim.
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

// ---------- start on boot ----------

#[cfg(windows)]
pub(crate) fn set_windows_startup_delay_zero() {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
        REG_DWORD, REG_OPTION_NON_VOLATILE,
    };
    use windows::core::PCWSTR;

    unsafe {
        let mut key = HKEY::default();
        let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Serialize\0"
            .encode_utf16()
            .collect();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        );
        if status.is_ok() {
            let zero: u32 = 0;
            let bytes = zero.to_ne_bytes();
            let delay_name: Vec<u16> = "StartupDelayInMSec\0".encode_utf16().collect();
            let _ = RegSetValueExW(
                key,
                PCWSTR(delay_name.as_ptr()),
                None,
                REG_DWORD,
                Some(&bytes),
            );
            let idle_name: Vec<u16> = "WaitForIdleState\0".encode_utf16().collect();
            let _ = RegSetValueExW(
                key,
                PCWSTR(idle_name.as_ptr()),
                None,
                REG_DWORD,
                Some(&bytes),
            );
            let _ = RegCloseKey(key);
        }
    }
}

#[cfg(windows)]
pub(crate) fn prioritize_run_key_entry() {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegEnumValueW, RegFlushKey, RegOpenKeyExW, RegQueryValueExW,
        RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_VALUE_TYPE,
    };
    use windows::core::{PCWSTR, PWSTR};

    unsafe {
        let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Run\0"
            .encode_utf16()
            .collect();
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            KEY_ALL_ACCESS,
            &mut key,
        )
        .is_err()
        {
            return;
        }

        let mut items: Vec<(String, Vec<u16>, REG_VALUE_TYPE, Vec<u8>)> = Vec::new();
        let mut idx = 0u32;
        loop {
            let mut name_buf = [0u16; 260];
            let mut name_len = name_buf.len() as u32;
            let mut val_type = REG_VALUE_TYPE::default();
            let mut data_len = 0u32;

            if RegEnumValueW(
                key,
                idx,
                Some(PWSTR(name_buf.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut val_type.0),
                None,
                Some(&mut data_len),
            )
            .is_err()
            {
                break;
            }

            let name_str = String::from_utf16_lossy(&name_buf[..name_len as usize]);
            let mut name_null = name_buf[..name_len as usize].to_vec();
            name_null.push(0);

            let mut data_buf = vec![0u8; data_len as usize];
            let _ = RegQueryValueExW(
                key,
                PCWSTR(name_null.as_ptr()),
                None,
                Some(&mut val_type),
                Some(data_buf.as_mut_ptr()),
                Some(&mut data_len),
            );

            items.push((name_str, name_null, val_type, data_buf));
            idx += 1;
        }

        if items.first().map(|(n, ..)| n.as_str()) != Some("Floaty") {
            if let Some(pos) = items.iter().position(|(n, ..)| n.eq_ignore_ascii_case("Floaty")) {
                let floaty_item = items.remove(pos);
                items.insert(0, floaty_item);

                for (_, name_null, ..) in &items {
                    let _ = RegDeleteValueW(key, PCWSTR(name_null.as_ptr()));
                }
                for (_, name_null, val_type, data) in &items {
                    let _ = RegSetValueExW(
                        key,
                        PCWSTR(name_null.as_ptr()),
                        None,
                        *val_type,
                        Some(data),
                    );
                }
                let _ = RegFlushKey(key);
            }
        }

        let _ = RegCloseKey(key);
    }
}

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
pub(crate) fn set_start_on_boot(app: &AppHandle, on: bool) -> bool {
    use tauri_plugin_autostart::ManagerExt;

    let manager = app.autolaunch();
    #[cfg(windows)]
    if on {
        set_windows_startup_delay_zero();
        prioritize_run_key_entry();
    }

    // `is_enabled` reads the entry back, so this is a no-op when it already says
    // what we want (a launch, or a save that did not touch the setting).
    if manager.is_enabled().unwrap_or(false) == on {
        return true;
    }

    match if on { manager.enable() } else { manager.disable() } {
        Ok(()) => {
            #[cfg(windows)]
            if on {
                prioritize_run_key_entry();
            }
            log_line(
                app,
                if on {
                    "autostart: floaty will start when you sign in (priority: highest, startup delay: 0ms)"
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
