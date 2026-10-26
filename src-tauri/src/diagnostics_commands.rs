//! `diagnostics` — moved out of lib.rs verbatim.
//!
//! Visibility is `pub(crate)` because the rest of the crate calls in across the module
//! boundary now; nothing here changed shape on the way out.

use crate::*;
use std::fs;
use tauri::AppHandle;

// ---------- diagnostics ----------

/// Everything the app knows about itself: the log tail, the monitor inventory,
/// where each window actually is, the heartbeat table, and the sizes on disk.
#[tauri::command]
pub(crate) async fn floaty_diagnostics(app: AppHandle) -> diagnostics::Report {
    let dir = app_data_dir(&app);
    let log_path = dir.join("floaty.log");
    let (lines, log_bytes) = diagnostics::read_log(&log_path);

    // The store plus whatever `.bak`/`.tmp` siblings the atomic writes have left.
    let mut store_bytes = fs::metadata(store_file(&app)).map(|m| m.len()).unwrap_or(0);
    let mut backups = 0;
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("floaty-store.json.")
            {
                backups += 1;
                store_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }

    let icon_dir = icons_dir(&app);
    let (icon_files, icon_bytes) = diagnostics::dir_size(&icon_dir);

    let (installed, rejected) = plugins::install_from(&plugins_dir(&app));

    let records = store::with(&app, |s| s.count() as u64);

    let now = elapsed_ms();
    let mut heartbeats: Vec<diagnostics::BeatInfo> = heartbeats()
        .lock()
        .map(|map| {
            map.iter()
                .map(|(label, (at, visibility))| diagnostics::BeatInfo {
                    label: label.clone(),
                    visibility: visibility.clone(),
                    ms_ago: now.saturating_sub(*at),
                })
                .collect()
        })
        .unwrap_or_default();
    heartbeats.sort_by(|a, b| a.label.cmp(&b.label));

    let mut repairs: Vec<diagnostics::RepairInfo> = heartbeat_repairs()
        .lock()
        .map(|map| {
            map.iter()
                .map(|(label, (_, attempts))| diagnostics::RepairInfo {
                    label: label.clone(),
                    attempts: *attempts,
                })
                .collect()
        })
        .unwrap_or_default();
    repairs.sort_by(|a, b| a.label.cmp(&b.label));

    let primary = app
        .primary_monitor()
        .ok()
        .flatten()
        .and_then(|m| m.name().map(|n| n.to_string()));

    let monitors = app
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let name = m.name().map(|n| n.to_string()).unwrap_or_default();
            diagnostics::MonitorInfo {
                primary: Some(name.clone()) == primary,
                key: name.clone(),
                name,
                x: m.position().x,
                y: m.position().y,
                width: m.size().width,
                height: m.size().height,
                scale: m.scale_factor(),
            }
        })
        .collect();

    // Physical px, which is what the platform reports and what the monitor list
    // above is in: "the window is where I think it is" is the whole point.
    let mut windows: Vec<diagnostics::WindowInfo> = app
        .webview_windows()
        .into_iter()
        .map(|(label, window)| {
            let pos = window
                .outer_position()
                .unwrap_or(tauri::PhysicalPosition::new(0, 0));
            let size = window
                .outer_size()
                .unwrap_or(tauri::PhysicalSize::new(0, 0));
            diagnostics::WindowInfo {
                layer: if is_desktop_layer_label(&label) {
                    "desktop".to_string()
                } else {
                    "window".to_string()
                },
                visible: window.is_visible().unwrap_or(false),
                label,
                x: pos.x,
                y: pos.y,
                width: size.width,
                height: size.height,
            }
        })
        .collect();
    windows.sort_by(|a, b| a.label.cmp(&b.label));

    let display = match DISPLAY_STATE.load(std::sync::atomic::Ordering::SeqCst) {
        1 => "on",
        0 => "off",
        _ => "unknown",
    };

    diagnostics::Report {
        version: app.package_info().version.to_string(),
        mode: if load_settings(&app).stay_on_desktop {
            "desktop".to_string()
        } else {
            "floating".to_string()
        },
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        started_ms_ago: now,
        log: diagnostics::LogInfo {
            rotated: diagnostics::rolled_count(&log_path),
            path: log_path.to_string_lossy().to_string(),
            bytes: log_bytes,
            lines,
        },
        store: diagnostics::FileInfo {
            path: store_file(&app).to_string_lossy().to_string(),
            bytes: store_bytes,
            backups,
        },
        icons: diagnostics::DirInfo {
            path: icon_dir.to_string_lossy().to_string(),
            files: icon_files,
            bytes: icon_bytes,
        },
        records,
        installed_plugins: plugins::labels(&installed),
        rejected_plugins: rejected,
        display: display.to_string(),
        heartbeats,
        repairs,
        monitors,
        windows,
    }
}

/// Say something in a Windows notification. Raised from Rust on purpose: a
/// third-party plugin reaches it through the widget api, so it needs no permission
/// of its own and no plugin command is exposed to a page.
#[tauri::command]
pub(crate) fn floaty_notify(title: String, body: String, app: AppHandle) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("a notification needs a title".to_string());
    }
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| format!("the notification was refused: {e}"))
}

/// Open the folder the log lives in, which is what a bug report asks for next.
#[tauri::command]
pub(crate) fn floaty_open_log_folder(app: AppHandle) -> Result<(), String> {
    let dir = app_data_dir(&app);
    launch_target(&app, &dir.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this module exists for, in one sentence: *the icons' layout is inconsistent —
    /// turn the machine off for a long time, reopen it, and the layout is messed up.*
    ///
    /// It is not a guess. This machine's log has the whole thing, at 14:01 on the day of the
    /// report: `monitors: the desktop is now 0,0 960x1440` at 14:01:39.850, then
    /// `monitors: brought 19 floatie(s) back onto a screen that still exists [...]` in the
    /// same second — every name in that list is one of the icons the trail had arranged on
    /// the right half of the screen — and then `the desktop is now 0,0 1440x960` again at
    /// 14:01:42.364, with `brought 15` more behind it. Nothing was plugged in: a fullscreen
    /// app changed the display mode and changed it back, and the desktop was rearranged for
    /// the shape in the middle.
    ///
    /// A floatie's place is `pinned`, so physics never moves it and nothing else in the app
    /// has an opinion about position: what a re-home does is permanent by construction. That
    /// is what makes a two-second lie about the screen shape into a permanently wrong
    /// layout.
    ///
    /// One screen, in the space records are stored in.
    fn screen(w: f64, h: f64) -> screens::Screen {
        screens::Screen {
            key: "\\\\.\\DISPLAY1".into(),
            name: "\\\\.\\DISPLAY1".into(),
            physical: screens::Rect::new(0.0, 0.0, w * 2.0, h * 2.0),
            logical: screens::Rect::new(0.0, 0.0, w, h),
            scale: 2.0,
            primary: true,
        }
    }

    /// The shape the machine is really in, and the one it was wrongly reported as.
    fn landscape() -> Vec<screens::Screen> {
        vec![screen(1440.0, 960.0)]
    }

    fn portrait() -> Vec<screens::Screen> {
        vec![screen(960.0, 1440.0)]
    }

    fn placed(s: &mut store::StoreData, id: &str, kind: &str, x: i32, y: i32) {
        s.put(store::WidgetRecord {
            id: id.into(),
            kind: kind.into(),
            x,
            y,
            // A place a person made: pinned, so nothing but a re-home may move it.
            data: serde_json::json!({ "pinned": true, "arranged": true }),
        });
    }

    /// The desktop from the trace: the icons the trail had arranged across the right of the
    /// screen (`folder-18`, `app-16`, `file-32` … are the names in the log's own list), two
    /// that were in the middle of it, and one panel whose body reaches past the middle.
    fn arranged_desktop() -> store::StoreData {
        let mut s = store::StoreData::default();
        for (id, kind, x, y) in [
            ("app-16", "app", 1071, 10),
            ("app-23", "app", 1306, 10),
            ("file-32", "file", 1318, 116),
            ("folder-18", "folder", 1075, 147),
            ("folder-25", "folder", 966, 145),
            ("file-41", "file", 1277, 587),
            ("folder-80", "folder", 1310, 483),
            ("folder-30", "folder", 1215, 677),
            ("folder-29", "folder", 1279, 838),
            ("folder-77", "folder", 1011, 724),
            ("folder-36", "folder", 1120, 728),
            ("app-20", "app", 1021, 838),
            ("folder-15", "folder", 90, 228),
            ("folder-31", "folder", 48, 329),
            ("file-69", "file", 268, 122),
            // A panel: on the portrait screen by its corner, off it by its body.
            ("note-1", "note", 900, 300),
        ] {
            placed(&mut s, id, kind, x, y);
        }
        s
    }

    fn places(s: &store::StoreData) -> Vec<(String, i32, i32)> {
        let mut out: Vec<(String, i32, i32)> = s.iter().map(|r| (r.id.clone(), r.x, r.y)).collect();
        out.sort();
        out
    }

    /// The ones nothing draws: not wholly on any screen that exists.
    fn off_every_screen(s: &store::StoreData, list: &[screens::Screen]) -> Vec<String> {
        let mut out: Vec<String> = s
            .iter()
            .filter(|r| {
                let (w, h) = plugins::size(&r.kind, &r.data);
                !screens::within_one_screen(list, r.x as f64, r.y as f64, w, h)
            })
            .map(|r| r.id.clone())
            .collect();
        out.sort();
        out
    }

    /// The captured trace, replayed the way the display-change path runs it: reports arrive
    /// when they arrived, the watch ticks twice a second between them, and the desktop is
    /// rearranged for a shape only once that shape has stopped changing.
    ///
    /// Returns `(shapes acted on, floaties moved)` — the first says the shape that stood
    /// still was not simply never looked at, the second is the damage.
    ///
    /// The clock is driven rather than slept on, so this is the same code the app runs and
    /// still finishes in milliseconds.
    fn replay(
        desktop: &mut store::StoreData,
        trace: &[(Vec<screens::Screen>, u64)],
    ) -> (usize, usize) {
        let mut pending = None;
        let mut acted = 0usize;
        let mut moved = 0usize;
        let last = trace.iter().map(|(_, at)| *at).max().unwrap_or(0);
        let mut arriving = 0usize;
        for tick in 0..=(last + 2 * ARRANGEMENT_SETTLE_MS) / 500 {
            let now = tick * 500;
            // what Windows said, when it said it
            while arriving < trace.len() && trace[arriving].1 <= now {
                let (list, at) = &trace[arriving];
                note_arrangement(
                    &mut pending,
                    screens::arrangement(screens::union(list)),
                    *at,
                );
                arriving += 1;
            }
            let Some(shape) = settled_arrangement(&mut pending, now, ARRANGEMENT_SETTLE_MS) else {
                continue;
            };
            // and what the machine is actually in at that moment
            let now_list: Vec<screens::Screen> = trace
                .iter()
                .rev()
                .find(|(_, at)| *at <= now)
                .map(|(list, _)| list.clone())
                .unwrap_or_default();
            if screens::arrangement(screens::union(&now_list)) != shape {
                continue;
            }
            acted += 1;
            moved += rehome_into(desktop, &now_list, true).len();
        }
        (acted, moved)
    }

    #[test]
    fn a_desktop_is_not_rearranged_for_a_shape_that_does_not_stay() {
        let mut desktop = arranged_desktop();
        let before = places(&desktop);
        assert_eq!(
            off_every_screen(&desktop, &landscape()),
            Vec::<String>::new(),
            "the fixture is a desktop that fits the shape it is on"
        );

        // 14:01:39 — a report of a shape the display is not going to stay in
        // 14:01:42 — the same display, 1440x960 again
        let (acted, moved) = replay(&mut desktop, &[(portrait(), 0), (landscape(), 2_500)]);

        assert_eq!(
            moved, 0,
            "the desktop is not moved for a shape that was reported and taken back"
        );
        assert_eq!(
            acted, 1,
            "... and the shape that did stand still was acted on, once"
        );
        assert_eq!(
            places(&desktop),
            before,
            "the desktop is where the user put it"
        );
    }

    #[test]
    fn a_place_belongs_to_the_arrangement_it_was_chosen_for() {
        let mut desktop = arranged_desktop();
        let before = places(&desktop);

        // A shape that stays — a screen really is portrait for a while, a dock is
        // unplugged — moves everything onto it, whole: nothing is left where nothing draws
        // it, and that is what the re-home is for.
        rehome_into(&mut desktop, &portrait(), true);
        assert_eq!(
            off_every_screen(&desktop, &portrait()),
            Vec::<String>::new(),
            "every floatie is drawn by an overlay again"
        );
        assert_ne!(
            places(&desktop),
            before,
            "a portrait screen is not this desktop's shape"
        );

        // ... and when the arrangement it *was* chosen for comes back, so do the places.
        // Without this the four seconds of a fullscreen app changing mode are permanent.
        rehome_into(&mut desktop, &landscape(), true);
        assert_eq!(
            places(&desktop),
            before,
            "the place belongs to the arrangement it was chosen for"
        );
    }

    #[test]
    fn a_place_the_user_moved_since_is_not_given_back() {
        let mut desktop = arranged_desktop();
        rehome_into(&mut desktop, &portrait(), true);

        // The user drags one of the displaced floaties somewhere they want it. That place
        // is the newer truth, and the memory of the old one is spent.
        let (id, x, y) = desktop
            .iter()
            .find(|r| r.id == "folder-29")
            .map(|r| (r.id.clone(), r.x, r.y))
            .expect("folder-29 is on the desktop");
        assert!(
            x != 1279 || y != 838,
            "it was displaced for the portrait shape"
        );
        desktop.edit(&id, |rec| {
            rec.x = 160;
            rec.y = 300;
        });

        rehome_into(&mut desktop, &landscape(), true);
        let rec = desktop.get(&id).expect("still there");
        assert_eq!(
            (rec.x, rec.y),
            (160, 300),
            "the place the user chose is not undone by an arrangement coming back"
        );
    }
}
