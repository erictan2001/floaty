//! When to get out of the way: a fullscreen app in front, or nobody at the machine.
//!
//! The decision is a pure function of what the machine looks like (`decide`), so it can
//! be tested without a desktop; the Win32 queries that describe the machine live in
//! `observe` and are the only part that needs a real one.
//!
//! Two separate behaviours, because they are worth different things:
//!
//! - **Hidden** — a fullscreen app is in front, so the desktop is not on screen at all.
//!   The overlays are hidden, which is also what stops the per-screen presents.
//! - **Quiet** — nobody has touched the machine for a while. The floaties stay where they
//!   are (this *is* the desktop), but the motion, the visualizer and the audio capture
//!   stop; that is where the watts are.
//!
//! A maximized window is deliberately *not* fullscreen: it covers the work area but not
//! the taskbar, and hiding the desktop because somebody maximized Explorer would be
//! absurd.

use std::time::Duration;

/// A rectangle in physical px — the space windows are placed in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    /// Does this rectangle cover `screen`, allowing `slack` px of disagreement?
    ///
    /// Slack is for the pixel a borderless fullscreen window loses to rounding, not for
    /// the taskbar: a maximized window is out by the taskbar's height, which is tens of
    /// pixels, and must not count.
    pub fn covers(&self, screen: &Rect, slack: f64) -> bool {
        self.x <= screen.x + slack
            && self.y <= screen.y + slack
            && self.x + self.w >= screen.x + screen.w - slack
            && self.y + self.h >= screen.y + screen.h - slack
    }
}

/// The window in front, described in the terms the decision cares about.
#[derive(Clone, Debug, PartialEq)]
pub struct Foreground {
    pub rect: Rect,
    /// A shell window (the desktop, the taskbar, the start menu) — never fullscreen.
    pub shell: bool,
    /// One of our own windows.
    pub ours: bool,
}

/// What we would like the desktop to be doing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct State {
    pub hidden: bool,
    pub quiet: bool,
    /// Why, for the log and the diagnostics tab — "fullscreen", "idle", or nothing.
    pub reason: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rules {
    pub hide_for_fullscreen: bool,
    pub quiet_when_idle: bool,
    pub idle_after: Duration,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            hide_for_fullscreen: true,
            quiet_when_idle: true,
            idle_after: Duration::from_secs(600),
        }
    }
}

/// How far a fullscreen window may be out, in px, and still count as fullscreen.
const FULLSCREEN_SLACK: f64 = 2.0;

/// The shell classes that are never "an app in front": the desktop itself, the taskbar,
/// the start menu and its hosts, the alt-tab overlay, and the IME/foreground staging
/// windows Windows creates for its own bookkeeping.
pub const SHELL_CLASSES: [&str; 7] = [
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Windows.UI.Core.CoreWindow",
    "XamlExplorerHostIslandWindow",
    "ForegroundStaging",
    "MultitaskingViewFrame",
];

pub fn is_shell_class(class: &str) -> bool {
    SHELL_CLASSES.iter().any(|c| class.eq_ignore_ascii_case(c))
}

/// The decision: hidden beats quiet, and neither applies when the rules are off.
pub fn decide(foreground: Option<&Foreground>, screens: &[Rect], rules: &Rules, idle_ms: u64) -> State {
    let fullscreen = rules.hide_for_fullscreen
        && foreground
            .filter(|f| !f.shell && !f.ours)
            .is_some_and(|f| screens.iter().any(|s| f.rect.covers(s, FULLSCREEN_SLACK)));
    if fullscreen {
        return State {
            hidden: true,
            quiet: true,
            reason: Some("fullscreen"),
        };
    }
    let idle = rules.quiet_when_idle
        && rules.idle_after > Duration::ZERO
        && idle_ms >= rules.idle_after.as_millis() as u64;
    State {
        hidden: false,
        quiet: idle,
        reason: if idle { Some("idle") } else { None },
    }
}

// ---------------------------------------------------------------------------------------
// The machine, as Win32 describes it.
// ---------------------------------------------------------------------------------------

/// What the machine looks like right now. `Err` carries the reason, to be logged once
/// rather than every poll.
pub struct Observations {
    pub foreground: Option<Foreground>,
    pub idle_ms: u64,
    pub screens: Vec<Rect>,
}

#[cfg(windows)]
pub fn observe(screens: Vec<Rect>, own: &[isize]) -> Observations {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowRect, IsWindowVisible,
    };

    unsafe {
        let mut info = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        // System-wide: the last time *any* input arrived, from anybody — the app being
        // idle is a different thing and would flatten the desktop while somebody reads.
        let idle_ms = if GetLastInputInfo(&mut info).as_bool() {
            GetTickCount().wrapping_sub(info.dwTime) as u64
        } else {
            0
        };

        let hwnd = GetForegroundWindow();
        let mut foreground = None;
        if !hwnd.is_invalid() && IsWindowVisible(hwnd).as_bool() {
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_ok() {
                let mut class = [0u16; 256];
                let len = GetClassNameW(hwnd, &mut class);
                let class = String::from_utf16_lossy(&class[..len.max(0) as usize]);
                foreground = Some(Foreground {
                    rect: Rect::new(
                        rect.left as f64,
                        rect.top as f64,
                        (rect.right - rect.left) as f64,
                        (rect.bottom - rect.top) as f64,
                    ),
                    shell: is_shell_class(&class),
                    ours: own.contains(&(hwnd.0 as isize)) || class.eq_ignore_ascii_case("Tauri Window"),
                });
            }
        }
        let _ = HWND::default();
        Observations {
            foreground,
            idle_ms,
            screens,
        }
    }
}

#[cfg(not(windows))]
pub fn observe(screens: Vec<Rect>, _own: &[isize]) -> Observations {
    Observations {
        foreground: None,
        idle_ms: 0,
        screens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> Vec<Rect> {
        vec![
            Rect::new(0.0, 0.0, 2880.0, 1920.0),
            Rect::new(2880.0, 0.0, 1920.0, 1080.0),
        ]
    }

    fn rules() -> Rules {
        Rules::default()
    }

    fn fullscreen_on(x: f64, w: f64, h: f64) -> Foreground {
        Foreground {
            rect: Rect::new(x, 0.0, w, h),
            shell: false,
            ours: false,
        }
    }

    #[test]
    fn a_fullscreen_app_hides_the_desktop() {
        let fg = fullscreen_on(0.0, 2880.0, 1920.0);
        let state = decide(Some(&fg), &pair(), &rules(), 0);
        assert!(state.hidden && state.quiet);
        assert_eq!(state.reason, Some("fullscreen"));
        // a fullscreen app on the *second* screen is just as good a reason
        let fg = fullscreen_on(2880.0, 1920.0, 1080.0);
        assert_eq!(decide(Some(&fg), &pair(), &rules(), 0).reason, Some("fullscreen"));
    }

    #[test]
    fn a_maximized_window_is_not_fullscreen() {
        // the work area of the primary: the taskbar's 96px are still visible
        let fg = fullscreen_on(0.0, 2880.0, 1920.0 - 96.0);
        let state = decide(Some(&fg), &pair(), &rules(), 0);
        assert!(!state.hidden, "a maximized window must leave the desktop alone");
        // ...and a window that covers one screen but not the other is not fullscreen either
        let fg = fullscreen_on(0.0, 1440.0, 960.0);
        assert!(!decide(Some(&fg), &pair(), &rules(), 0).hidden);
    }

    #[test]
    fn the_shell_and_our_own_windows_are_never_a_reason_to_hide() {
        let mut fg = fullscreen_on(0.0, 2880.0, 1920.0);
        fg.shell = true;
        assert!(!decide(Some(&fg), &pair(), &rules(), 0).hidden);
        fg.shell = false;
        fg.ours = true;
        assert!(!decide(Some(&fg), &pair(), &rules(), 0).hidden);
        assert!(!decide(None, &pair(), &rules(), 0).hidden);
        assert!(is_shell_class("Progman") && is_shell_class("workerw"));
        assert!(!is_shell_class("Chrome_WidgetWin_1"));
    }

    #[test]
    fn idle_goes_quiet_without_hiding_anything() {
        let state = decide(None, &pair(), &rules(), 600_000);
        assert!(!state.hidden);
        assert!(state.quiet);
        assert_eq!(state.reason, Some("idle"));
        // one millisecond short of the threshold is still awake
        assert!(!decide(None, &pair(), &rules(), 599_999).quiet);
    }

    #[test]
    fn the_rules_turn_both_behaviours_off() {
        let off = Rules {
            hide_for_fullscreen: false,
            quiet_when_idle: false,
            idle_after: Duration::from_secs(600),
        };
        let fg = fullscreen_on(0.0, 2880.0, 1920.0);
        let state = decide(Some(&fg), &pair(), &off, 10_000_000);
        assert!(!state.hidden && !state.quiet);
        // an idle threshold of zero means "never", not "immediately": a settings box
        // left at 0 must not flatten the desktop the moment it is saved
        let zero = Rules {
            idle_after: Duration::ZERO,
            ..rules()
        };
        assert!(!decide(None, &pair(), &zero, u64::MAX).quiet);
    }

    #[test]
    fn a_fullscreen_app_wins_over_idle() {
        let fg = fullscreen_on(0.0, 2880.0, 1920.0);
        assert_eq!(
            decide(Some(&fg), &pair(), &rules(), 10_000_000).reason,
            Some("fullscreen"),
        );
    }

    #[test]
    fn covering_allows_rounding_but_not_a_taskbar() {
        let screen = Rect::new(0.0, 0.0, 2880.0, 1920.0);
        assert!(Rect::new(-1.0, 0.0, 2881.0, 1920.0).covers(&screen, FULLSCREEN_SLACK));
        assert!(!Rect::new(0.0, 0.0, 2880.0, 1824.0).covers(&screen, FULLSCREEN_SLACK));
        assert!(!Rect::new(0.0, 0.0, 1920.0, 1920.0).covers(&screen, FULLSCREEN_SLACK));
    }
}
