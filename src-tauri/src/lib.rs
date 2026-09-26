// What is left here is the crate root: the entry point, the shared vocabulary the modules are
// built on, and the 20 sections that used to live in this one 9710-line file. The modules each
// carry their own copy of the imports they need, so only what this file itself uses stays.
use std::collections::HashSet;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::Manager;

mod audio;
mod diagnostics;
mod exchange;
mod fs_watch;
mod icons;
mod palette;
mod placement;
mod plugin_install;
mod plugins;
mod presence;
mod screens;
mod shell_ops;
mod sysmon;
mod undo;

mod logging;
pub(crate) use logging::*;

mod store;
pub(crate) use store::*;

mod global_floating_settings;
pub(crate) use global_floating_settings::*;

mod start_on_boot;
pub(crate) use start_on_boot::*;

mod desktop_window_pinning_stay_on_desktop_win_d;
pub(crate) use desktop_window_pinning_stay_on_desktop_win_d::*;

mod sleep_resume;
pub(crate) use sleep_resume::*;

mod desktop_windows;
pub(crate) use desktop_windows::*;

mod app_discovery_launch;
pub(crate) use app_discovery_launch::*;

mod plugin_commands;
pub(crate) use plugin_commands::*;

mod folders;
pub(crate) use folders::*;

mod putting_dragged_items_on_disk;
pub(crate) use putting_dragged_items_on_disk::*;

mod live2d_model_library;
pub(crate) use live2d_model_library::*;

mod commands;
pub(crate) use commands::*;

mod launcher_palette;
pub(crate) use launcher_palette::*;

mod undo_commands;
pub(crate) use undo_commands::*;

mod real_file_operations_desktop_parity;
pub(crate) use real_file_operations_desktop_parity::*;

mod diagnostics_commands;
pub(crate) use diagnostics_commands::*;

mod audio_visualizer;
pub(crate) use audio_visualizer::*;

mod system_monitor;
pub(crate) use system_monitor::*;

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
        // Notifications are raised from Rust (`floaty_notify`) and reached by
        // plugins through the widget api, so no notification permission reaches
        // any page.
        .plugin(tauri_plugin_notification::init())
        // Updating in place: the manifest is signed and so is the payload, and the public
        // key that checks both lives in `tauri.conf.json`. The private key is the user's
        // (see the README) — nothing here can sign anything.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // The launcher hotkey. Registered and handled in Rust, so the palette page
        // needs no plugin permission to use it — and a press only shows a window,
        // which cannot fail in a way worth telling anyone about.
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state != tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        return;
                    }
                    // One handler for every global key floaty holds: the launcher's, and
                    // the paste key.
                    let is = |text: &str| {
                        std::str::FromStr::from_str(text)
                            .ok()
                            .is_some_and(|key: tauri_plugin_global_shortcut::Shortcut| &key == shortcut)
                    };
                    if is(UNDO_ACCELERATOR) {
                        // off the main thread: undo moves files
                        let handle = app.clone();
                        std::thread::spawn(move || undo_from_shortcut(&handle));
                        return;
                    }
                    if is(REDO_ACCELERATOR) {
                        // off the main thread, for the same reason
                        let handle = app.clone();
                        std::thread::spawn(move || redo_from_shortcut(&handle));
                        return;
                    }
                    let paste = is(PASTE_ACCELERATOR);
                    if paste {
                        // off the main thread: the paste moves files and takes the
                        // clipboard, neither of which belongs on the pumping thread
                        let handle = app.clone();
                        std::thread::spawn(move || {
                            if let Err(why) = paste_at_cursor(&handle) {
                                log_line(&handle, &format!("paste: {why}"));
                            }
                        });
                        return;
                    }
                    show_palette(app);
                })
                .build(),
        )
        .manage(store::new_state())
        .setup(|app| {
            let _ = SHARED_APP.set(app.handle().clone());
            // Icons are files (`<app data>/icons/<hash>.png`) that records point
            // at with an asset url. Install the folder here, before any resolver
            // runs: the resolver works on threads that have no AppHandle.
            icons::set_dir(icons_dir(app.handle()));
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
            // Get out of the way of a fullscreen app, and go quiet when nobody is here.
            set_presence_rules(rules_from(&load_settings(&handle)));
            start_presence_watch(handle.clone());
            log_line(&handle, "=== floaty starting ===");
            log_line(&handle, &format!("backend build {}", env!("FLOATY_BUILD_MARK")));

            #[cfg(windows)]
            {
                let stay = load_settings(&handle).stay_on_desktop;
                desktop_pin::set_stay_on_desktop_global(stay);
            }

            // restore persisted widgets into state
            let mut saved = load_all(&handle);
            // A store written before icons were files carries every icon inlined
            // as base64 — 97% of its bytes. Move them out (idempotent, so this is
            // free on a store that has already been through it) and remember
            // whether the store loaded at all, which is what licenses the sweep
            // further down.
            let store_loaded = !saved.is_empty();
            let mut icons_moved = 0usize;
            for rec in saved.iter_mut() {
                icons_moved += icons::migrate_json(&mut rec.data);
            }
            if icons_moved > 0 {
                log_line(
                    &handle,
                    &format!(
                        "icons: moved {icons_moved} inlined icons into {}",
                        icons_dir(&handle).display()
                    ),
                );
            }
            // the store we just loaded is known good: pin it as the backup
            refresh_backup(&store_file(&handle), true);
            let mut reclassified = 0usize;
            store::with(&handle, |s| {
                let mut max_n: u64 = 0;
                let mut loaded = Vec::with_capacity(saved.len());
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
                    loaded.push(rec);
                }
                s.load_records(loaded);
                s.set_next(max_n);
            });
            if reclassified > 0 || icons_moved > 0 {
                persist(&handle);
            }

            // Drop icon files no record mentions any more (a removed widget, or
            // the inlined copies the migration above just replaced). Only ever on
            // a store that loaded: a damaged or first-run store references
            // nothing, and sweeping then would delete the whole desktop's icons.
            if store_loaded {
                let mut referenced = HashSet::new();
                store::with(&handle, |s| {
                    for rec in s.iter() {
                        icons::collect_referenced(&rec.data, &mut referenced);
                    }
                });
                let (removed, freed) = icons::sweep(&referenced);
                if removed > 0 {
                    log_line(
                        &handle,
                        &format!(
                            "icons: swept {removed} unused files ({:.1} MB) — {} still in use",
                            freed as f64 / 1048576.0,
                            referenced.len()
                        ),
                    );
                }
            }

            // Start on boot is a registry entry, so reconcile it with the setting
            // at launch: this is what repairs the entry when the executable has
            // moved (an update in place, a new build), which a toggle alone cannot
            // do.
            let boot = load_settings(&handle);
            if !set_start_on_boot(&handle, boot.start_on_boot) {
                log_line(&handle, "autostart: could not reconcile the startup entry");
            }
            // The first-run guard (ADR 0004): the overlay asks for the answer, and
            // the log says which way it went, so a launch can be read without the
            // screen. A probe's override says so in as many words, because a
            // desktop that came up without a question is otherwise indistinguishable
            // from a store that was already answered.
            if let Some(seed) = root_confirmed_from_env() {
                log_line(
                    &handle,
                    &format!("first run: FLOATY_ROOT_CONFIRMED says {seed} — the answer comes from the environment, not the store"),
                );
            } else if !root_confirmed_for_this_launch(&boot) {
                log_line(
                    &handle,
                    "first run: the root has not been confirmed — the desktop waits for an answer",
                );
            }
            apply_palette_shortcut(&handle);

            // tray: settings + quit only (widgets are managed via settings)
            let search = MenuItem::with_id(app, "search", "Search…", true, None::<&str>)?;
            let settings =
                MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Floaty", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&search, &settings, &quit])?;
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
                    // The launcher has a hotkey, but a hotkey another app can steal
                    // must never be the only way in.
                    "search" => show_palette(app),
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
                // also a clock for the visual check
                create_record(&handle, "clock").ok();
            }

            // A display that went away while floaty was not running leaves records at
            // coordinates no screen covers, and they are pinned, so nothing else will
            // ever move them: bring them home before they are mounted.
            rehome_stranded(&handle, "startup");

            // spawn restored widgets, or a welcome note on first run
            let ids: Vec<WidgetRecord> = store::with(&handle, |s| s.iter().cloned().collect());
            if ids.is_empty() {
                if let Ok(rec) = create_record(&handle, "note") {
                    store::with(&handle, |s| {
                        s.edit(&rec.id, |r| {
                            r.data = serde_json::json!({
                                "text": "welcome to floaty!\n\n- drag me by the top bar\n- right-click the tray icon for more\n- i live on your desktop now"
                            });
                        });
                    });
                }
            }
            // Before the overlay asks for its manifest: it reads the approval state
            // out of the settings, and a plugin that was already here must read as
            // approved on this very first pass, or its widgets never mount.
            migrate_plugin_trust(&handle);

            // Spawn overlay window as early as possible so WebView2 initializes
            // concurrently with disk scanning and plugin installation.
            if let Err(e) = spawn_overlay_windows(&handle) {
                log_line(&handle, &format!("spawn_overlay_window FAILED: {e}"));
            }
            // A pin survives a restart: whatever was pinned comes back in the top
            // layer, which means building that layer again.
            sync_top_overlay(&handle);

            let (installed, rejected) = plugins::install_from(&plugins_dir(&handle));
            if !installed.is_empty() || !rejected.is_empty() {
                log_line(
                    &handle,
                    &format!(
                        "plugins: {} installed [{}]{}",
                        installed.len(),
                        plugins::labels(&installed).join(", "),
                        if rejected.is_empty() {
                            String::new()
                        } else {
                            format!(", {} rejected [{}]", rejected.len(), rejected.join("; "))
                        }
                    ),
                );
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
            floaty_get_record,
            floaty_create,
            floaty_save,
            floaty_refresh,
            floaty_remove,
            floaty_delete,
            floaty_undo,
            floaty_redo,
            floaty_undo_state,
            floaty_undo_checkpoint,
            floaty_palette_search,
            floaty_palette_run,
            floaty_palette_hide,
            floaty_show_palette,
            floaty_palette_state,
            floaty_presence,
            floaty_drop_paths,
            floaty_clipboard_paste,
            floaty_clipboard_copy,
            floaty_gesture_begin,
            floaty_gesture_end,
            floaty_check_update,
            floaty_install_update,
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
            floaty_install_plugin,
            floaty_inspect_plugin,
            floaty_uninstall_plugin,
            floaty_set_plugin_enabled,
            floaty_get_settings,
            floaty_set_settings,
            floaty_root_guard,
            floaty_confirm_root,
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
            floaty_diagnostics,
            floaty_open_log_folder,
            floaty_plugin_trust,
            floaty_notify,
            floaty_rehome_floaties,
            floaty_desktop_rect,
            floaty_monitors,
            floaty_screens,
            floaty_overlay_area,
            floaty_drag_to,
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
    fn a_merge_undo_only_moves_the_record_that_was_dragged() {
        // the folder the file went into did not move, so the remembered origin is not its
        // to use: doing that is what "undoing put my folder where the file was" means
        assert!(origin_applies("file-7", "file-7"));
        assert!(!origin_applies("file-7", "folder-33"));
    }

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

    /// The first-run answer is new, so an existing store does not have it: it must
    /// still load, and it must read as *unanswered* — that is what makes the guard
    /// appear on the first launch after the upgrade rather than never (ADR 0004).
    #[test]
    fn an_old_settings_file_leaves_the_root_unconfirmed() {
        let old = r#"{ "gravity": 1200.0, "files_root": "C:\\Desktop" }"#;
        let s: FloatSettings =
            serde_json::from_str(old).expect("an old settings file must still parse");
        assert!(
            !s.root_confirmed,
            "a file without the field is an unanswered first run"
        );
        assert_eq!(s.files_root, "C:\\Desktop", "the root it did name survives");
        // and it round-trips, so the answer is written out from now on
        let back: FloatSettings =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).expect("round trip");
        assert!(!back.root_confirmed);
    }

    /// The first-run screen says what is in the folder, one level deep: the count the
    /// desktop would show, which of the files are launchable, and a few names. An
    /// empty folder and a folder that is not there are different answers — neither is
    /// an empty list.
    #[test]
    fn the_root_preview_counts_what_the_desktop_would_show() {
        let dir = std::env::temp_dir().join(format!("floaty-root-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Holiday photos")).unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        std::fs::write(dir.join("Arc.lnk"), b"x").unwrap();
        for n in 0..10 {
            std::fs::write(dir.join(format!("f{n}.txt")), b"x").unwrap();
        }

        let report = root_preview(&dir.to_string_lossy(), false);
        assert!(!report.confirmed, "the screen is up because nobody has answered");
        assert!(report.exists);
        assert_eq!(report.items, 13, "1 folder + 12 files");
        assert_eq!(report.folders, 1);
        assert_eq!(report.files, 12);
        assert_eq!(report.apps, 1, "the .lnk is the one that becomes an app floatie");
        assert_eq!(report.names.len(), ROOT_GUARD_NAMES);
        assert_eq!(report.more, 5, "the rest are counted, not listed");

        // an empty folder says it is empty rather than showing an empty list
        let empty = std::env::temp_dir().join(format!("floaty-root-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).unwrap();
        let report = root_preview(&empty.to_string_lossy(), false);
        assert!(report.exists, "there, but with nothing in it");
        assert_eq!(report.items, 0);
        assert!(report.names.is_empty());

        // and a folder that is not there is missing, not empty
        let report = root_preview(&dir.join("gone").to_string_lossy(), false);
        assert!(!report.exists);
        assert_eq!(report.items, 0);
        assert_eq!(report.root, dir.join("gone").to_string_lossy());
        // nothing asked for at all is the same answer
        let report = root_preview("   ", false);
        assert!(!report.exists);
        assert_eq!(report.root, "");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
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
        assert_eq!(
            placement::free_name(&dir, placement::NEW_FOLDER_BASE),
            dir.join("New folder")
        );

        let first = placement::create_new_folder(&dir).unwrap();
        assert_eq!(first, dir.join("New folder"));
        assert_eq!(
            placement::create_new_folder(&dir).unwrap(),
            dir.join("New folder (2)")
        );
        assert_eq!(
            placement::create_new_folder(&dir).unwrap(),
            dir.join("New folder (3)")
        );

        // the first free name in the sequence wins, exactly like the shell: with
        // "New folder" deleted again, that is the one it hands out next
        std::fs::remove_dir(&first).unwrap();
        assert_eq!(
            placement::create_new_folder(&dir).unwrap(),
            dir.join("New folder")
        );

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

        let dir = placement::create_new_folder(&root).unwrap();
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

        let dir = placement::create_new_folder(&root).unwrap();
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
        // the shell's numbering, and the same one a file moved into a folder gets: the
        // unnumbered name is the first, so the second one is (2)
        assert_eq!(std::path::PathBuf::from(second), root.join("Thing (2).lnk"));

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

        let dir = placement::create_new_folder(&root).unwrap();
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
        let dest_file = placement::free_name(dest_dir, file_inside.file_name().unwrap());
        assert_eq!(dest_file, temp_dir.join("doc.txt"));

        std::fs::rename(&file_inside, &dest_file).unwrap();
        assert!(!file_inside.exists());
        assert!(dest_file.exists());

        // 2. Test moving directory out to parent (root)
        let sub_inside = folder_dir.join("SubProject");
        std::fs::create_dir_all(&sub_inside).unwrap();
        std::fs::write(sub_inside.join("inner.txt"), "inner").unwrap();
        assert!(sub_inside.exists());

        let dest_sub = placement::free_name(dest_dir, sub_inside.file_name().unwrap());
        assert_eq!(dest_sub, temp_dir.join("SubProject"));

        std::fs::rename(&sub_inside, &dest_sub).unwrap();
        assert!(!sub_inside.exists());
        assert!(dest_sub.is_dir());
        assert!(dest_sub.join("inner.txt").exists());

        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_fast_ico_and_url_extraction() {
        // Test with real Steam game url if present
        let test_url = r"C:\Users\erict\OneDrive\Desktop\games\Magical Princess.url";
        if std::path::Path::new(test_url).exists() {
            let icon = try_extract_fast_url(test_url)
                .expect("try_extract_fast_url should successfully extract Steam icon");
            // Whichever form it comes back in — a stored file url, or an inlined
            // data url when there is no icons folder to write to — it has to be a
            // real, readable icon, because that is what the tile renders.
            let (w, h) = icons::pixel_size(&icon).expect("a readable png");
            assert!(w >= 32 && h >= 32, "{w}x{h}");
            assert!(!icons::is_missing(&icon));
            assert!(
                icons::read(&icon).unwrap().len() > 1000,
                "should produce high-res icon data"
            );
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

    #[cfg(windows)]
    #[test]
    fn test_windows_startup_configuration() {
        use windows::Win32::System::Registry::{
            RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
            HKEY_CURRENT_USER, KEY_ALL_ACCESS, KEY_READ, REG_DWORD,
        };
        use windows::core::PCWSTR;

        let serialize: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Serialize\0"
            .encode_utf16()
            .collect();

        // A REG_DWORD's state before this test touched it: `Some(v)` if it existed, `None`
        // if it did not.
        let read_dword = |key: HKEY, name: &str| -> Option<u32> {
            let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let mut data = 0u32;
            let mut data_len = 4u32;
            let mut val_type = REG_DWORD;
            let ok = unsafe {
                RegQueryValueExW(
                    key,
                    PCWSTR(wide.as_ptr()),
                    None,
                    Some(&mut val_type),
                    Some(&mut data as *mut u32 as *mut u8),
                    Some(&mut data_len),
                )
                .is_ok()
            };
            ok.then_some(data)
        };
        // Put one back exactly as it was: its value, or no value at all.
        let restore_dword = |key: HKEY, name: &str, prior: Option<u32>| {
            let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            unsafe {
                match prior {
                    Some(value) => {
                        let bytes = value.to_ne_bytes();
                        let _ = RegSetValueExW(key, PCWSTR(wide.as_ptr()), None, REG_DWORD, Some(&bytes));
                    }
                    None => {
                        let _ = RegDeleteValueW(key, PCWSTR(wide.as_ptr()));
                    }
                }
            }
        };

        // The two values this test asserts on live in the *real* user profile, and the call
        // under test writes them. A test must not change the machine it runs on, so read
        // what is there first and put it back afterwards — the same shape the Run-key half
        // below already uses for its Floaty entry.
        let mut prior_delay = None;
        let mut prior_idle = None;
        unsafe {
            let mut key = HKEY::default();
            if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(serialize.as_ptr()), None, KEY_ALL_ACCESS, &mut key).is_ok() {
                prior_delay = read_dword(key, "StartupDelayInMSec");
                prior_idle = read_dword(key, "WaitForIdleState");
                let _ = RegCloseKey(key);
            }
        }

        set_windows_startup_delay_zero();
        prioritize_run_key_entry();

        // Verify Serialize key
        unsafe {
            let mut key = HKEY::default();
            let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Serialize\0"
                .encode_utf16()
                .collect();
            if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(subkey.as_ptr()), None, KEY_READ, &mut key).is_ok() {
                let mut data = 0u32;
                let mut data_len = 4u32;
                let mut val_type = REG_DWORD;
                let delay_name: Vec<u16> = "StartupDelayInMSec\0".encode_utf16().collect();
                if RegQueryValueExW(
                    key,
                    PCWSTR(delay_name.as_ptr()),
                    None,
                    Some(&mut val_type),
                    Some(&mut data as *mut u32 as *mut u8),
                    Some(&mut data_len),
                ).is_ok() {
                    assert_eq!(data, 0, "StartupDelayInMSec must be 0");
                }
                let idle_name: Vec<u16> = "WaitForIdleState\0".encode_utf16().collect();
                if RegQueryValueExW(
                    key,
                    PCWSTR(idle_name.as_ptr()),
                    None,
                    Some(&mut val_type),
                    Some(&mut data as *mut u32 as *mut u8),
                    Some(&mut data_len),
                ).is_ok() {
                    assert_eq!(data, 0, "WaitForIdleState must be 0");
                }
                let _ = RegCloseKey(key);
            }

            // Verify Floaty is at index 0 of the Run key.
            //
            // The promotion has nothing to do unless Floaty is in the key at all,
            // which is true of a machine that has run the app and false of a clean
            // CI runner. So put it there when it is missing — added last, so the
            // call above has to do the moving — and take it back out afterwards, so
            // the test leaves the machine exactly as it found it.
            let run_subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Run\0"
                .encode_utf16()
                .collect();
            if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(run_subkey.as_ptr()), None, KEY_ALL_ACCESS, &mut key).is_ok() {
                use windows::Win32::System::Registry::{
                    RegDeleteValueW, RegEnumValueW, RegSetValueExW, REG_SZ,
                };
                use windows::core::PWSTR;

                let floaty_name: Vec<u16> = "Floaty\0".encode_utf16().collect();
                let present = RegQueryValueExW(
                    key,
                    PCWSTR(floaty_name.as_ptr()),
                    None,
                    None,
                    None,
                    None,
                ).is_ok();

                if !present {
                    let exe = std::env::current_exe().unwrap_or_default();
                    let command: Vec<u16> = format!("\"{}\"\0", exe.display()).encode_utf16().collect();
                    let bytes: Vec<u8> = command.iter().flat_map(|w| w.to_le_bytes()).collect();
                    let _ = RegSetValueExW(key, PCWSTR(floaty_name.as_ptr()), None, REG_SZ, Some(&bytes));
                    // the call under test again, now with something to promote
                    prioritize_run_key_entry();
                }

                let mut name_buf = [0u16; 260];
                let mut name_len = name_buf.len() as u32;
                if RegEnumValueW(
                    key,
                    0,
                    Some(PWSTR(name_buf.as_mut_ptr())),
                    &mut name_len,
                    None,
                    None,
                    None,
                    None,
                ).is_ok() {
                    let first_name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
                    assert_eq!(first_name, "Floaty", "Floaty must be at index 0 of the Run key");
                } else {
                    panic!("the Run key has no values to enumerate");
                }

                if !present {
                    let _ = RegDeleteValueW(key, PCWSTR(floaty_name.as_ptr()));
                }
                let _ = RegCloseKey(key);
            }

            // Put the two Serialize values back exactly as they were — their old value, or
            // no value at all — so the profile this test ran against is the profile it
            // leaves behind. Same shape as the Floaty entry just above: touch only what the
            // call under test needs, then take it back.
            let mut key = HKEY::default();
            if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(serialize.as_ptr()), None, KEY_ALL_ACCESS, &mut key).is_ok() {
                restore_dword(key, "StartupDelayInMSec", prior_delay);
                restore_dword(key, "WaitForIdleState", prior_idle);
                let _ = RegCloseKey(key);
            }
        }
    }
    #[test]
    fn a_disk_op_reverses_what_the_action_did_to_the_files() {
        let dir = std::env::temp_dir().join(format!("floaty-undo-disk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sub = dir.join("New folder");
        std::fs::create_dir_all(&sub).unwrap();
        let original = dir.join("a.txt");
        let moved = sub.join("a.txt");
        std::fs::write(&original, b"the file").unwrap();

        // what a merge does, then the undo of it
        std::fs::rename(&original, &moved).unwrap();
        assert!(!original.exists());
        let what = reverse_disk(&undo::DiskOp::moved(
            &original.to_string_lossy(),
            &moved.to_string_lossy(),
        ))
        .unwrap();
        assert!(what.contains("back to"), "{what}");
        assert_eq!(std::fs::read(&original).unwrap(), b"the file");
        assert!(!moved.exists());

        // a file that came back by hand first is not overwritten
        std::fs::rename(&original, &moved).unwrap();
        std::fs::write(&original, b"something else").unwrap();
        let err = reverse_disk(&undo::DiskOp::moved(
            &original.to_string_lossy(),
            &moved.to_string_lossy(),
        ))
        .unwrap_err();
        assert!(err.contains("in the way"), "{err}");
        assert_eq!(std::fs::read(&original).unwrap(), b"something else");
        // and one that is not there to move back says so
        std::fs::remove_file(&moved).unwrap();
        let err = reverse_disk(&undo::DiskOp::moved(
            &original.to_string_lossy(),
            &moved.to_string_lossy(),
        ))
        .unwrap_err();
        assert!(err.contains("not there"), "{err}");

        // a folder the grouping created goes again — unless something is in it
        let made = dir.join("Grouped");
        std::fs::create_dir_all(&made).unwrap();
        reverse_disk(&undo::DiskOp::discard(&made.to_string_lossy())).unwrap();
        assert!(!made.exists());
        std::fs::create_dir_all(&made).unwrap();
        std::fs::write(made.join("kept.txt"), b"still wanted").unwrap();
        let what = reverse_disk(&undo::DiskOp::discard(&made.to_string_lossy())).unwrap();
        assert!(what.contains("left in place"), "{what}");
        assert!(made.join("kept.txt").exists());
        // a shortcut written into a folder is just a file
        let lnk = dir.join("shortcut.lnk");
        std::fs::write(&lnk, b"x").unwrap();
        reverse_disk(&undo::DiskOp::discard(&lnk.to_string_lossy())).unwrap();
        assert!(!lnk.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_checkpoint_that_changed_nothing_is_not_worth_an_undo_press() {
        let mut store = StoreData::default();
        let rec = WidgetRecord {
            id: "app-1".to_string(),
            kind: "app".to_string(),
            x: 40,
            y: 50,
            data: serde_json::json!({ "name": "Arc" }),
        };
        store.put(rec.clone());

        let step = |restore: Vec<undo::Restore>, disk: Vec<undo::DiskOp>| undo::Step {
            label: "test".to_string(),
            restore,
            disk,
        };
        let same = |store: &StoreData| {
            vec![undo::Restore {
                id: "app-1".to_string(),
                record: store
                    .get("app-1")
                    .and_then(|r| serde_json::to_value(r).ok()),
            }]
        };

        // nothing touched: the store is exactly what the checkpoint recorded
        assert!(!step_changes_anything(&store, &step(same(&store), vec![])));
        // a moved widget, a removed one, and a file move all count
        let mut moved = store.get("app-1").unwrap().clone();
        moved.x = 999;
        store.put(moved);
        assert!(step_changes_anything(&store, &step(same(&store), vec![])) == false);
        let before = step(
            vec![undo::Restore {
                id: "app-1".to_string(),
                record: Some(serde_json::to_value(&rec).unwrap()),
            }],
            vec![],
        );
        assert!(step_changes_anything(&store, &before), "the position differs");
        assert!(step_changes_anything(
            &store,
            &step(vec![], vec![undo::DiskOp::recycled("C:\\gone.txt")])
        ));
        // a record the action created is a change: it is gone from the store
        assert!(step_changes_anything(
            &store,
            &step(
                vec![undo::Restore {
                    id: "app-1".to_string(),
                    record: None
                }],
                vec![]
            )
        ));
        // ... and one it created that is somehow still there is not
        assert!(!step_changes_anything(
            &store,
            &step(
                vec![undo::Restore {
                    id: "folder-99".to_string(),
                    record: None
                }],
                vec![]
            )
        ));
    }

}
