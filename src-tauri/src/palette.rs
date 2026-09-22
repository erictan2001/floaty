//! The launcher palette: one window, one global shortcut, and the matching that
//! decides what "code" means.
//!
//! Two rules shaped it:
//!
//! - **A launcher that takes a fifth of a second to appear is not a launcher.** The
//!   window is built once, on the first call, and after that it is only shown and
//!   hidden — no rebuild, no reload, and the page keeps its own state.
//! - **The hotkey is a convenience, never the only way in.** If another app already
//!   owns the key, registering it fails and that is a line in the log: the tray
//!   item and the settings button open the same window.

use serde::{Deserialize, Serialize};
use std::str::FromStr;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

/// The window's label. Deliberately *not* a desktop-layer label: the palette is
/// something you call up, not something that lives on the wallpaper, so it gets an
/// ordinary window's taskbar and z-order behaviour.
pub const WINDOW_LABEL: &str = "palette";

/// Big enough for a search line and six or seven rows. A large palette is a window
/// with extra steps.
const WIDTH: f64 = 560.0;
const HEIGHT: f64 = 380.0;

/// How many rows a search may return. The list scrolls; a query that answers with
/// two hundred things is a query with no answer in it.
pub const MAX_HITS: usize = 30;

/// One row: what the page draws, and what it sends back to be run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaletteHit {
    /// `app`, `file`, `folder` or `floatie` — how running it behaves.
    pub kind: String,
    pub title: String,
    /// The path for something on disk, the kind's own words for a widget.
    pub subtitle: String,
    /// An icon url, or `""` when there is none and the page should draw a letter.
    pub icon: String,
    /// The widget record, for a hit that is a floatie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The file or folder to open, for a hit that is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// How well `text` answers `query`: lower is better, `None` is no answer at all.
///
/// The order is what a launcher is judged on. An exact name beats a name that
/// starts with the query, which beats a match at a word boundary — typing "code"
/// should reach *Visual Studio Code* before *Barcode* — which beats a match inside
/// a word, which beats a subsequence ("vsc" → *Visual Studio Code*, which is how
/// people actually type in a launcher). Ties keep the caller's order, which is the
/// order the desktop itself lists things in.
pub fn score(query: &str, text: &str) -> Option<u8> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let text = text.to_lowercase();
    if text == query {
        return Some(0);
    }
    if text.starts_with(&query) {
        return Some(1);
    }
    if starts_a_word(&text, &query) {
        return Some(2);
    }
    if text.contains(&query) {
        return Some(3);
    }
    if is_subsequence(&query, &text) {
        return Some(4);
    }
    None
}

/// Does the query begin a word inside the text?
fn starts_a_word(text: &str, query: &str) -> bool {
    text.char_indices().any(|(i, _)| {
        i > 0
            && text[..i].chars().next_back().is_some_and(|c| !c.is_alphanumeric())
            && text[i..].starts_with(query)
    })
}

/// Are the query's characters in the text, in order? (`vsc` → `Visual Studio Code`)
fn is_subsequence(query: &str, text: &str) -> bool {
    let mut rest = text.chars();
    query.chars().all(|q| rest.any(|c| c == q))
}

/// The indices of `keys` that answer `query`, best first, capped at `limit`.
///
/// An empty query answers everything in the order given, which is what an empty
/// palette should show: the first page of what there is, not nothing.
pub fn rank(query: &str, keys: &[String], limit: usize) -> Vec<usize> {
    let mut scored: Vec<(u8, usize)> = keys
        .iter()
        .enumerate()
        .filter_map(|(index, key)| score(query, key).map(|rank| (rank, index)))
        .collect();
    scored.sort_by_key(|(rank, index)| (*rank, *index));
    scored.into_iter().take(limit).map(|(_, index)| index).collect()
}

/// The palette window, built on first use and then only shown and hidden.
pub fn ensure_window(app: &AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        return Ok(window);
    }
    let window = WebviewWindowBuilder::new(
        app,
        WINDOW_LABEL,
        WebviewUrl::App("index.html#/palette".into()),
    )
    .title("floaty launcher")
    .inner_size(WIDTH, HEIGHT)
    .transparent(true)
    .decorations(false)
    .shadow(false)
    .resizable(false)
    .skip_taskbar(true)
    .always_on_top(true)
    // built hidden: the first appearance is a hotkey press, not a window popping
    // into existence mid-sentence
    .visible(false)
    .center()
    .build()?;
    Ok(window)
}

/// Show the palette, freshly centred (the monitors can change between calls), and
/// tell the page to start a new search.
pub fn show(app: &AppHandle) -> tauri::Result<()> {
    let window = ensure_window(app)?;
    // On the screen the user is looking at, not always on the primary: with two
    // monitors, a launcher that opens on the other one is a launcher you have to
    // go and fetch.
    if let Some((x, y)) = cursor_monitor_spot(app, WIDTH, HEIGHT) {
        let _ = window.set_position(tauri::LogicalPosition::new(x, y));
    }
    let _ = window.center();
    let _ = window.show();
    let _ = window.set_focus();
    // The page clears its query and refocuses the input on this: reopening a
    // launcher and finding the last search still in it is how a launcher feels
    // broken.
    window.emit("floaty-palette-shown", ()).ok();
    Ok(())
}

pub fn hide(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.hide();
    }
}

/// True when the palette is up. A hidden window is still a window, so `is_visible`
/// is the question — not "does it exist".
pub fn is_visible(app: &AppHandle) -> bool {
    app.get_webview_window(WINDOW_LABEL)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

/// The accelerator as it should be stored, or `None` when it is not one.
///
/// Separate from registering so that a *settings write* can ask "is this a key at
/// all?" without taking the working shortcut away to find out.
pub fn parse_shortcut(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    tauri_plugin_global_shortcut::Shortcut::from_str(text)
        .ok()
        .map(|_| text.to_string())
}

/// Where to put the palette: centred on the monitor the pointer is on, a third of
/// the way down it. `None` when the platform will not say where the pointer is —
/// the window then stays wherever it was built.
fn cursor_monitor_spot(app: &AppHandle, w: f64, h: f64) -> Option<(f64, f64)> {
    let cursor = app.cursor_position().ok()?;
    // The screen model does the arithmetic: the cursor is in physical px and the
    // palette is placed in record space, which is what `to_virtual` is for.
    let on = crate::screens(app).into_iter().find(|s| {
        cursor.x >= s.physical.x
            && cursor.y >= s.physical.y
            && cursor.x < s.physical.x + s.physical.w
            && cursor.y < s.physical.y + s.physical.h
    })?;
    let local_x = (cursor.x - on.physical.x) / on.scale;
    let (vx, _) = on.to_virtual(local_x, 0.0);
    // centred horizontally on the pointer's screen, a third of the way down it
    let left = vx.clamp(on.logical.x, (on.logical.right() - w).max(on.logical.x));
    let top = on.logical.y + (on.logical.h - h) / 3.0;
    Some((left.round(), top.round()))
}

/// Point the launcher hotkey at `accelerator`, replacing whatever was there.
///
/// The parse comes first on purpose: a typo in settings must not be able to take a
/// working shortcut away. Everything after it is reported to the caller, which logs
/// it — a key another app owns is not a reason to fail to start.
pub fn register_shortcut(app: &AppHandle, accelerator: &str) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let text = accelerator.trim();
    if text.is_empty() {
        return Err("no launcher key set".to_string());
    }
    let shortcut = tauri_plugin_global_shortcut::Shortcut::from_str(text)
        .map_err(|e| format!("'{text}' is not a key combination ({e})"))?;
    // Only ours: unregistering everything is safe here because this is the only
    // global shortcut floaty registers.
    let _ = app.global_shortcut().unregister_all();
    let registered = app
        .global_shortcut()
        .register(shortcut)
        .map_err(|e| format!("'{text}' could not be registered ({e})"));
    // Every global key floaty holds is registered here, because `unregister_all` is how
    // this function changes its mind: a key registered anywhere else would vanish the
    // next time the launcher's row is touched. A key that is already taken is logged and
    // survived rather than fatal — the launcher must still open if something else owns it.
    for (key, what) in [
        (crate::PASTE_ACCELERATOR, "clipboard"),
        (crate::UNDO_ACCELERATOR, "undo"),
        (crate::REDO_ACCELERATOR, "redo"),
    ] {
        if let Ok(shortcut) = tauri_plugin_global_shortcut::Shortcut::from_str(key) {
            if let Err(e) = app.global_shortcut().register(shortcut) {
                crate::log_line(app, &format!("{what}: {key} is taken ({e})"));
            }
        }
    }
    registered
}

/// Whether the key in the settings is the key that is actually registered.
pub fn shortcut_is_live(app: &AppHandle, accelerator: &str) -> bool {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    tauri_plugin_global_shortcut::Shortcut::from_str(accelerator.trim())
        .map(|shortcut| app.global_shortcut().is_registered(shortcut))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_name_that_starts_with_the_query_beats_one_that_merely_contains_it() {
        assert_eq!(score("code", "Visual Studio Code"), Some(2), "starts a word");
        assert_eq!(score("barcode", "Barcode"), Some(0), "exact");
        assert_eq!(score("bar", "Barcode"), Some(1), "prefix");
        assert_eq!(score("code", "Barcode"), Some(3), "inside a word");
        assert!(score("code", "Visual Studio Code") < score("code", "Barcode"));
        assert_eq!(score("zz", "Arc"), None);
    }

    #[test]
    fn the_query_matches_case_insensitively_and_by_initials() {
        assert_eq!(score("ARC", "Arc"), Some(0));
        assert_eq!(score("vsc", "Visual Studio Code"), Some(4));
        // deliberately loose: v-sc-ode is in there in order, so it matches. The
        // rule is "the letters are there in that order", not "the words start
        // with them" — a launcher that insists on word starts cannot be typed at.
        assert_eq!(score("vscode", "Visual Studio Code"), Some(4));
        assert_eq!(
            score("codex", "Visual Studio Code"),
            None,
            "and a letter that is not there, in order, does not match"
        );
        assert_eq!(score("", "anything"), Some(0), "an empty query asks for everything");
        assert_eq!(score("  ", "anything"), Some(0));
    }

    #[test]
    fn ranking_keeps_the_desktops_own_order_within_a_score() {
        let keys = keys(&["Barcode", "Arc", "Visual Studio Code", "Arcade"]);
        assert_eq!(
            rank("arc", &keys, 10),
            vec![1, 3, 0],
            "two prefixes in the order given, then Barcode, which merely contains it"
        );
        assert_eq!(rank("code", &keys, 10), vec![2, 0], "word-start before inside");
        assert_eq!(rank("", &keys, 2), vec![0, 1], "an empty query is the first page");
        assert_eq!(rank("zzz", &keys, 10), Vec::<usize>::new());
        assert_eq!(rank("a", &keys, 1).len(), 1, "the cap is the cap");
    }
}
