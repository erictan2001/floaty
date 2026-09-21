//! `desktop_window_pinning_stay_on_desktop_win_d` — moved out of lib.rs verbatim.
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

// ---------- desktop window pinning (stay on desktop / Win+D) ----------

#[cfg(windows)]
pub(crate) mod desktop_pin {
    use std::sync::atomic::{AtomicBool, Ordering};

    pub static STAY_ON_DESKTOP_ACTIVE: AtomicBool = AtomicBool::new(true);

    pub fn set_stay_on_desktop_global(active: bool) {
        STAY_ON_DESKTOP_ACTIVE.store(active, Ordering::SeqCst);
    }

    const GWLP_HWNDPARENT: i32 = -8;
    const GWL_STYLE: i32 = -16;
    const GWL_EXSTYLE: i32 = -20;
    const WS_CAPTION: isize = 0x00C00000;
    const WS_THICKFRAME: isize = 0x00040000;
    const WS_BORDER: isize = 0x00800000;
    const WS_DLGFRAME: isize = 0x00400000;
    const WS_EX_TOOLWINDOW: isize = 0x00000080;
    const WS_EX_APPWINDOW: isize = 0x00040000;

    const WM_ERASEBKGND: u32 = 0x0014;
    const WM_NCCALCSIZE: u32 = 0x0083;
    const WM_NCPAINT: u32 = 0x0085;
    const WM_NCACTIVATE: u32 = 0x0086;

    const DWMWA_NCRENDERING_POLICY: u32 = 2;
    const DWMNCRP_DISABLED: u32 = 1;

    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_FRAMECHANGED: u32 = 0x0020;
    const SWP_HIDEWINDOW: u32 = 0x0080;
    const HWND_TOPMOST: isize = -1;

    const WM_SYSCOMMAND: u32 = 0x0112;
    const SC_MINIMIZE: usize = 0xF020;
    const WM_WINDOWPOSCHANGING: u32 = 0x0046;
    const WM_SIZE: u32 = 0x0005;
    const SIZE_MINIMIZED: usize = 1;
    const SW_RESTORE: i32 = 9;

    const SUBCLASS_ID: usize = 0x464C5459; // 'FLTY'

    #[repr(C)]
    struct WINDOWPOS {
        pub(crate) hwnd: isize,
        pub(crate) hwnd_insert_after: isize,
        pub(crate) x: i32,
        pub(crate) y: i32,
        pub(crate) cx: i32,
        pub(crate) cy: i32,
        pub(crate) flags: u32,
    }

    #[allow(non_snake_case)]
    #[repr(C)]
    struct ITaskbarListVtbl {
        pub QueryInterface: unsafe extern "system" fn(this: *mut std::ffi::c_void, riid: *const u8, ppv: *mut *mut std::ffi::c_void) -> i32,
        pub AddRef: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> u32,
        pub Release: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> u32,
        pub HrInit: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> i32,
        pub AddTab: unsafe extern "system" fn(this: *mut std::ffi::c_void, hwnd: isize) -> i32,
        pub DeleteTab: unsafe extern "system" fn(this: *mut std::ffi::c_void, hwnd: isize) -> i32,
        pub ActivateTab: unsafe extern "system" fn(this: *mut std::ffi::c_void, hwnd: isize) -> i32,
        pub SetActiveAlt: unsafe extern "system" fn(this: *mut std::ffi::c_void, hwnd: isize) -> i32,
    }

    #[allow(non_snake_case)]
    #[repr(C)]
    struct ITaskbarList {
        pub lpVtbl: *const ITaskbarListVtbl,
    }

    #[allow(non_snake_case)]
    #[link(name = "user32")]
    #[link(name = "comctl32")]
    #[link(name = "ole32")]
    #[link(name = "kernel32")]
    #[link(name = "gdi32")]
    #[link(name = "dwmapi")]
    extern "system" {
        pub fn DwmSetWindowAttribute(
            hwnd: isize,
            dwAttribute: u32,
            pvAttribute: *const std::ffi::c_void,
            cbAttribute: u32,
        ) -> i32;
        pub fn FindWindowW(lpClassName: *const u16, lpWindowName: *const u16) -> isize;
        pub fn FindWindowExW(
            hWndParent: isize,
            hWndChildAfter: isize,
            lpszClass: *const u16,
            lpszWindow: *const u16,
        ) -> isize;
        pub fn GetShellWindow() -> isize;
        pub fn GetWindowLongPtrW(hWnd: isize, nIndex: i32) -> isize;
        pub fn SetWindowLongPtrW(hWnd: isize, nIndex: i32, dwNewLong: isize) -> isize;
        pub fn SetWindowPos(
            hWnd: isize,
            hWndInsertAfter: isize,
            X: i32,
            Y: i32,
            cx: i32,
            cy: i32,
            uFlags: u32,
        ) -> isize;
        pub fn ShowWindow(hWnd: isize, nCmdShow: i32) -> i32;
        pub fn IsWindowVisible(hWnd: isize) -> i32;
        pub fn SetWindowSubclass(
            hWnd: isize,
            pfnSubclass: unsafe extern "system" fn(
                hWnd: isize,
                uMsg: u32,
                wParam: usize,
                lParam: isize,
                uIdSubclass: usize,
                dwRefData: usize,
            ) -> isize,
            uIdSubclass: usize,
            dwRefData: usize,
        ) -> i32;
        pub fn DefSubclassProc(hWnd: isize, uMsg: u32, wParam: usize, lParam: isize) -> isize;
        pub fn OpenInputDesktop(dwFlags: u32, fInherit: i32, dwDesiredAccess: u32) -> isize;
        pub fn SetThreadDesktop(hDesktop: isize) -> i32;
        pub fn CoCreateInstance(
            rclsid: *const u8,
            pUnkOuter: *mut std::ffi::c_void,
            dwClsContext: u32,
            riid: *const u8,
            ppv: *mut *mut std::ffi::c_void,
        ) -> i32;
        pub fn CreateRectRgn(x1: i32, y1: i32, x2: i32, y2: i32) -> isize;
        pub fn CombineRgn(hrgnDst: isize, hrgnSrc1: isize, hrgnSrc2: isize, iMode: i32) -> i32;
        pub fn DeleteObject(ho: isize) -> i32;
        pub fn SetWindowRgn(hWnd: isize, hRgn: isize, bRedraw: i32) -> i32;

        pub fn InvalidateRect(hWnd: isize, lpRect: *const std::ffi::c_void, bErase: i32) -> i32;
        pub fn RedrawWindow(
            hWnd: isize,
            lprcUpdate: *const std::ffi::c_void,
            hrgnUpdate: isize,
            flags: u32,
        ) -> i32;
    }

    const RDW_INVALIDATE: u32 = 0x0001;
    const RDW_ALLCHILDREN: u32 = 0x0080;

    /// Make the window paint itself from scratch.
    ///
    /// A WebView2's first frame is white and Chromium only repaints the damage it
    /// knows about, so after a standby that white can survive in regions the page
    /// paints nothing into — it shows up as solid white bands through the
    /// translucent tiles sitting above it. Raw Win32, on purpose: this must not
    /// queue behind a wedged renderer.
    pub fn force_repaint(hwnd: isize) {
        if hwnd == 0 {
            return;
        }
        unsafe {
            InvalidateRect(hwnd, std::ptr::null(), 0);
            // No RDW_UPDATENOW on purpose: that waits for the paint to happen, and
            // the paint is what can be stuck. Invalidate and let it come back.
            RedrawWindow(hwnd, std::ptr::null(), 0, RDW_INVALIDATE | RDW_ALLCHILDREN);
        }
    }

    pub fn delete_taskbar_tab(hwnd: isize) {
        unsafe {
            // CLSID_TaskbarList {56FDF342-FD6D-11d0-958A-006097C9A090}
            let clsid: [u8; 16] = [0x42, 0xF3, 0xFD, 0x56, 0x6D, 0xFD, 0xd0, 0x11, 0x95, 0x8A, 0x00, 0x60, 0x97, 0xC9, 0xA0, 0x90];
            // IID_ITaskbarList {56FDF344-FD6D-11d0-958A-006097C9A090}
            let iid: [u8; 16] = [0x44, 0xF3, 0xFD, 0x56, 0x6D, 0xFD, 0xd0, 0x11, 0x95, 0x8A, 0x00, 0x60, 0x97, 0xC9, 0xA0, 0x90];
            let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            if CoCreateInstance(clsid.as_ptr(), std::ptr::null_mut(), 1, iid.as_ptr(), &mut ptr) == 0 && !ptr.is_null() {
                let tbl = ptr as *mut ITaskbarList;
                let vtbl = &*(*tbl).lpVtbl;
                let _ = (vtbl.HrInit)(ptr);
                let _ = (vtbl.DeleteTab)(ptr, hwnd);
                let _ = (vtbl.Release)(ptr);
            }
        }
    }

    pub fn remove_from_taskbar(hwnd: isize) {
        unsafe {
            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let new_style = (ex_style | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW;
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_style);
            SetWindowPos(
                hwnd,
                0,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
            delete_taskbar_tab(hwnd);
        }
    }

    /// The inverse of `delete_taskbar_tab`: put the window's taskbar button back.
    pub fn add_taskbar_tab(hwnd: isize) {
        unsafe {
            // CLSID_TaskbarList {56FDF342-FD6D-11d0-958A-006097C9A090}
            let clsid: [u8; 16] = [
                0x42, 0xF3, 0xFD, 0x56, 0x6D, 0xFD, 0xd0, 0x11, 0x95, 0x8A, 0x00, 0x60, 0x97, 0xC9,
                0xA0, 0x90,
            ];
            // IID_ITaskbarList {56FDF344-FD6D-11d0-958A-006097C9A090}
            let iid: [u8; 16] = [
                0x44, 0xF3, 0xFD, 0x56, 0x6D, 0xFD, 0xd0, 0x11, 0x95, 0x8A, 0x00, 0x60, 0x97, 0xC9,
                0xA0, 0x90,
            ];
            let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            if CoCreateInstance(
                clsid.as_ptr(),
                std::ptr::null_mut(),
                1,
                iid.as_ptr(),
                &mut ptr,
            ) == 0
                && !ptr.is_null()
            {
                let tbl = ptr as *mut ITaskbarList;
                let vtbl = &*(*tbl).lpVtbl;
                let _ = (vtbl.HrInit)(ptr);
                let _ = (vtbl.AddTab)(ptr, hwnd);
                let _ = (vtbl.Release)(ptr);
            }
        }
    }

    /// Hand a window back to the shell as an ordinary app window.
    ///
    /// `apply_desktop_pin` marks a window a tool window and deletes its taskbar
    /// tab, which is what the overlays and the one-window-per-widget floaties
    /// want, and exactly what breaks everything else the app opens: a tool
    /// window with no tab cannot be minimized and come back, never joins
    /// Alt-Tab, and — with the desktop shell as its owner — drags its own title
    /// bar around behind every other window. The settings window is an ordinary
    /// window, so any pass that walks *every* label has to give it back.
    ///
    /// Returns true when it actually changed something, so a caller can say so
    /// in the log instead of repairing silently.
    pub fn restore_ordinary_window(hwnd: isize) -> bool {
        if hwnd == 0 {
            return false;
        }
        unsafe {
            let mut changed = false;
            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let wanted = (ex_style & !WS_EX_TOOLWINDOW) | WS_EX_APPWINDOW;
            if wanted != ex_style {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted);
                SetWindowPos(
                    hwnd,
                    0,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
                changed = true;
            }
            // no owner: an owned window follows its owner's z-order and cannot be
            // activated on its own
            if GetWindowLongPtrW(hwnd, GWLP_HWNDPARENT) != 0 {
                SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, 0);
                changed = true;
            }
            // the tab comes back only for a window that is on screen, so the
            // hidden manager window never puts one up for nothing
            if changed && IsWindowVisible(hwnd) != 0 {
                add_taskbar_tab(hwnd);
            }
            changed
        }
    }

    pub fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn ensure_input_desktop() {
        unsafe {
            let dt = OpenInputDesktop(0, 0, 0x01FF);
            if dt != 0 {
                SetThreadDesktop(dt);
            }
        }
    }

    pub fn get_desktop_shell_hwnd() -> isize {
        ensure_input_desktop();
        let progman_cls = to_wide("Progman");
        let defview_cls = to_wide("SHELLDLL_DefView");
        let workerw_cls = to_wide("WorkerW");

        // 1. Try finding SHELLDLL_DefView directly under Progman (Win 11 24H2+, etc.)
        let progman = unsafe { FindWindowW(progman_cls.as_ptr(), std::ptr::null()) };
        if progman != 0 {
            let defview = unsafe { FindWindowExW(progman, 0, defview_cls.as_ptr(), std::ptr::null()) };
            if defview != 0 {
                return defview;
            }
        }

        // 2. Try finding SHELLDLL_DefView under top-level WorkerW windows (Win 10 / earlier Win 11)
        let mut workerw = unsafe { FindWindowExW(0, 0, workerw_cls.as_ptr(), std::ptr::null()) };
        while workerw != 0 {
            let defview = unsafe { FindWindowExW(workerw, 0, defview_cls.as_ptr(), std::ptr::null()) };
            if defview != 0 {
                return defview;
            }
            workerw = unsafe { FindWindowExW(0, workerw, workerw_cls.as_ptr(), std::ptr::null()) };
        }

        // 3. Fallback to GetShellWindow() or Progman
        let shell = unsafe { GetShellWindow() };
        if shell != 0 {
            return shell;
        }
        progman
    }

    /// `ref_data` for windows the desktop-layer policy applies to. Ordinary
    /// windows get `0` and the policy leaves them alone.
    const DESKTOP_LAYER_REF: usize = 1;

    unsafe extern "system" fn widget_subclass_proc(
        hwnd: isize,
        msg: u32,
        wparam: usize,
        lparam: isize,
        _id_subclass: usize,
        ref_data: usize,
    ) -> isize {
        // The desktop-layer policy lives in this one procedure, and it is only
        // meant for the windows that *are* the desktop. Applied to the settings
        // window it is the "settings is broken" bug: the first drag step stripped
        // its taskbar button (this runs on every WM_WINDOWPOSCHANGING, and a move
        // is a stream of them) and the minimize button did nothing.
        let desktop_layer = ref_data == DESKTOP_LAYER_REF;

        // Enforce WS_EX_TOOLWINDOW is preserved so taskbar never shows the window
        if desktop_layer && msg == WM_WINDOWPOSCHANGING {
            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            if (ex_style & WS_EX_TOOLWINDOW) == 0 || (ex_style & WS_EX_APPWINDOW) != 0 {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex_style | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW);
            }
        }

        // Sleep/resume lands here: the webview is never told, so the app has to
        // put itself back together. Two shapes: a resume code after a classic
        // sleep, or the display switching back on after Modern Standby (which
        // sends no resume at all).
        // A display was plugged in, unplugged or re-scaled. Nothing acted on this
        // before, so the overlay kept the size it was built with, the pages kept the
        // layout they had read at mount, and a floatie that had been dragged onto the
        // screen that just went away kept its coordinates — pinned, so nothing was
        // ever going to move it back. Every top-level window gets this message, and
        // the debounce inside treats one change as one job.
        if msg == crate::WM_DISPLAYCHANGE {
            std::thread::spawn(crate::display_arrangement_changed);
        }

        if msg == crate::WM_POWERBROADCAST {
            if wparam == crate::PBT_POWERSETTINGCHANGE && lparam != 0 {
                let setting = unsafe { &*(lparam as *const windows::Win32::System::Power::POWERBROADCAST_SETTING) };
                if setting.PowerSetting == windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE
                    && setting.DataLength >= 1
                    && setting.Data[0] == 1
                {
                    // Never do this work on the pumping thread: it touches the
                    // webview, the compositor and the disk, and a stall in any of
                    // them makes Windows mark the whole app "Not Responding".
                    std::thread::spawn(|| crate::recover_windows("display back on"));
                    return 1;
                }
            } else if wparam == crate::PBT_APMRESUMEAUTOMATIC
                || wparam == crate::PBT_APMRESUMESUSPEND
                || wparam == crate::PBT_APMRESUMECRITICAL
            {
                std::thread::spawn(|| crate::recover_windows("woke from sleep"));
                return 1;
            }
        }

        if desktop_layer && STAY_ON_DESKTOP_ACTIVE.load(Ordering::Relaxed) {
            // Block SC_MINIMIZE syscommand
            if msg == WM_SYSCOMMAND && (wparam & 0xFFF0) == SC_MINIMIZE {
                return 0;
            }
            // Strip SWP_HIDEWINDOW when Windows tries to hide windows on Win+D / Show Desktop
            if msg == WM_WINDOWPOSCHANGING && lparam != 0 {
                let wp = &mut *(lparam as *mut WINDOWPOS);
                if (wp.flags & SWP_HIDEWINDOW) != 0 {
                    wp.flags &= !SWP_HIDEWINDOW;
                }
            }
            // If window receives SIZE_MINIMIZED, restore it
            if msg == WM_SIZE && wparam == SIZE_MINIMIZED {
                ShowWindow(hwnd, SW_RESTORE);
                return 0;
            }
        }

        // Suppress non-client frame rendering & background erase. This prevents
        // Windows from drawing a default white title bar / caption strip along
        // the top edge of shaped window regions — but only for the windows that
        // have no frame to draw in the first place. Swallowing these on an
        // ordinary window is what made the settings window lose its title bar
        // the first time it was maximized: the frame is painted through
        // WM_NCPAINT, and nothing repaints it once the message is answered with
        // "nothing to do" (the initial frame is only visible because it was
        // painted at creation, before this procedure was armed).
        if desktop_layer {
            if msg == WM_NCCALCSIZE {
                return 0;
            }
            if msg == WM_NCPAINT {
                return 0;
            }
            if msg == WM_NCACTIVATE {
                return 1;
            }
            if msg == WM_ERASEBKGND {
                return 1;
            }
        }

        DefSubclassProc(hwnd, msg, wparam, lparam)
    }

    /// Install the window procedure that blocks minimize/show-desktop tricks and
    /// answers the sleep/resume messages. Idempotent.
    ///
    /// Timing matters: a window's procedure can still be replaced while the
    /// webview is being created, which drops an earlier subclass out of the
    /// chain — and then the display-state notification is delivered into
    /// nothing. Installing it again once the window is up (the first heartbeat)
    /// is what makes the wake-up recovery reliable.
    /// Arm the window procedure on a window. `desktop_layer` decides which
    /// policy the procedure enforces for it: `true` keeps the tool-window style
    /// on and swallows minimize (the overlays, the one-window-per-widget
    /// floaties), `false` leaves an ordinary window alone.
    pub fn install_subclass(hwnd: isize, desktop_layer: bool) {
        if hwnd != 0 {
            unsafe {
                SetWindowSubclass(
                    hwnd,
                    widget_subclass_proc,
                    SUBCLASS_ID,
                    if desktop_layer { DESKTOP_LAYER_REF } else { 0 },
                );
            }
        }
    }

    pub fn apply_desktop_pin(hwnd: isize, stay_on_desktop: bool) {
        unsafe {
            remove_from_taskbar(hwnd);
            // one window takes the display-state notification for the process
            crate::watch_display_state(hwnd);

            // Register subclass (idempotent if already registered)
            install_subclass(hwnd, true);

            if stay_on_desktop {
                let desktop_hwnd = get_desktop_shell_hwnd();
                if desktop_hwnd != 0 {
                    SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, desktop_hwnd);
                }
            } else {
                SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, 0);
            }
            SetWindowPos(
                hwnd,
                0,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct HitRect {
        pub id: String,
        pub x: i32,
        pub y: i32,
        pub w: i32,
        pub h: i32,
    }

    const RGN_OR: i32 = 2;
    const WS_EX_TRANSPARENT: isize = 0x00000020;

    /// The overlay windows and their click-through regions, keyed by window
    /// label: `desktop-overlay` is the desktop layer, `top-overlay` holds the
    /// widgets pinned above other windows. Each window keeps its own region — a
    /// region applied to the wrong window is either a window that swallows every
    /// click on the screen or one that cannot be clicked at all.
    static OVERLAY_HWNDS: std::sync::RwLock<Vec<(String, isize)>> = std::sync::RwLock::new(Vec::new());
    static OVERLAY_HIT_RECTS: std::sync::RwLock<Vec<(String, Vec<HitRect>)>> =
        std::sync::RwLock::new(Vec::new());
    static OVERLAY_IS_DRAGGING: std::sync::RwLock<Vec<String>> = std::sync::RwLock::new(Vec::new());

    fn remember_hwnd(label: &str, hwnd: isize) {
        if let Ok(mut guard) = OVERLAY_HWNDS.write() {
            match guard.iter_mut().find(|(l, _)| l == label) {
                Some(slot) => slot.1 = hwnd,
                None => guard.push((label.to_string(), hwnd)),
            }
        }
    }

    fn hwnd_for(label: &str) -> isize {
        OVERLAY_HWNDS
            .read()
            .ok()
            .and_then(|g| g.iter().find(|(l, _)| l == label).map(|(_, h)| *h))
            .unwrap_or(0)
    }

    fn rects_for(label: &str) -> Vec<HitRect> {
        OVERLAY_HIT_RECTS
            .read()
            .ok()
            .and_then(|g| g.iter().find(|(l, _)| l == label).map(|(_, r)| r.clone()))
            .unwrap_or_default()
    }

    fn is_dragging(label: &str) -> bool {
        OVERLAY_IS_DRAGGING
            .read()
            .map(|g| g.iter().any(|l| l == label))
            .unwrap_or(false)
    }

    pub fn apply_hit_regions(hwnd: isize, rects: &[HitRect]) {
        if hwnd == 0 {
            return;
        }
        unsafe {
            let valid: Vec<&HitRect> = rects.iter().filter(|r| r.w > 0 && r.h > 0).collect();
            if valid.is_empty() {
                // An empty region excludes the entire window from receiving clicks
                let empty = CreateRectRgn(0, 0, 0, 0);
                SetWindowRgn(hwnd, empty, 0);
                return;
            }

            let first = valid[0];
            let combined = CreateRectRgn(first.x, first.y, first.x + first.w, first.y + first.h);
            for r in &valid[1..] {
                let item = CreateRectRgn(r.x, r.y, r.x + r.w, r.y + r.h);
                CombineRgn(combined, combined, item, RGN_OR);
                DeleteObject(item);
            }

            // SetWindowRgn transfers ownership of `combined` to the operating system.
            // bRedraw must be 1: the strip newly added to the region still holds
            // stale (transparent) pixels, and Chromium only repaints damage it
            // knows about — a widget whose pixels were clipped away before the
            // region included them would stay invisible until something else
            // damaged it. Client redraws are safe here: the subclass suppresses
            // WM_NCCALCSIZE / WM_NCPAINT and swallows WM_ERASEBKGND.
            SetWindowRgn(hwnd, combined, 1);
            InvalidateRect(hwnd, std::ptr::null(), 0);
        }
    }

    pub fn set_hit_rects(label: String, rects: Vec<HitRect>) {
        let changed = if let Ok(mut guard) = OVERLAY_HIT_RECTS.write() {
            match guard.iter_mut().find(|(l, _)| *l == label) {
                Some(slot) => {
                    if slot.1 == rects {
                        false
                    } else {
                        slot.1 = rects.clone();
                        true
                    }
                }
                None => {
                    guard.push((label.clone(), rects.clone()));
                    true
                }
            }
        } else {
            false
        };
        let hwnd = hwnd_for(&label);
        if changed && !is_dragging(&label) && hwnd != 0 {
            apply_hit_regions(hwnd, &rects);
        }
    }

    pub fn set_dragging(label: String, dragging: bool) {
        if let Ok(mut guard) = OVERLAY_IS_DRAGGING.write() {
            guard.retain(|l| *l != label);
            if dragging {
                guard.push(label.clone());
            }
        }
        let hwnd = hwnd_for(&label);
        if hwnd != 0 {
            if dragging {
                // Clear the window region during dragging so the entire desktop can receive drag events
                unsafe {
                    SetWindowRgn(hwnd, 0, 0);
                }
            } else {
                apply_hit_regions(hwnd, &rects_for(&label));
            }
        }
    }

    pub fn cleanup_hook() {}

    /// Put a window in front of *everything*, other topmost windows included.
    ///
    /// `WS_EX_TOPMOST` is not exclusive: every topmost window keeps the flag, and
    /// the band is ordered by whoever raised (or was activated) last. A remote
    /// desktop window that puts itself on top therefore covers a window that is
    /// also topmost — measured: RustDesk's session window one slot above the
    /// pinned layer. Re-asserting the position is what "always on top" means in
    /// practice, and it is what desktop "keep on top" utilities do.
    ///
    /// `SWP_NOACTIVATE` is the point of the flags: the z-order changes, the
    /// focus does not — the user keeps typing where they were typing.
    pub fn raise_above_everything(hwnd: isize) {
        if hwnd == 0 {
            return;
        }
        unsafe {
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// Style an overlay window and remember it under its label, so the regions
    /// its page sends land on the right window.
    pub fn apply_overlay_desktop_pin(label: &str, hwnd: isize, stay_on_desktop: bool) {
        apply_desktop_pin(hwnd, stay_on_desktop);
        remember_hwnd(label, hwnd);
        unsafe {
            // Remove WS_EX_TRANSPARENT so the shaped regions receive clicks normally
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex & !WS_EX_TRANSPARENT);

            // Strip caption / thickframe / border styles to eliminate non-client frame
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(WS_CAPTION | WS_THICKFRAME | WS_BORDER | WS_DLGFRAME));

            // Disable DWM non-client rendering policy (no caption, frame, or drop shadows)
            let policy: u32 = DWMNCRP_DISABLED;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_NCRENDERING_POLICY,
                &policy as *const u32 as *const std::ffi::c_void,
                std::mem::size_of::<u32>() as u32,
            );

            SetWindowPos(
                hwnd,
                0,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
        if let Ok(guard) = OVERLAY_HIT_RECTS.read() {
            if let Some((_, rects)) = guard.iter().find(|(l, _)| l == label) {
                apply_hit_regions(hwnd, rects);
            }
        }
    }
}
