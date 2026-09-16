#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    {
        // Elevate process and main thread priority to High so Floaty starts first
        // and renders its desktop overlay without being preempted by other startup apps.
        unsafe {
            use windows::Win32::System::Threading::{
                GetCurrentProcess, GetCurrentThread, SetPriorityClass, SetThreadPriority,
                HIGH_PRIORITY_CLASS, THREAD_PRIORITY_HIGHEST,
            };
            let _ = SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        }

        let existing = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").unwrap_or_default();
        let low_mem_args = "--process-per-site --disable-features=Translate,OptimizationHints,MediaRouter --disable-background-networking";
        let combined = if existing.is_empty() {
            low_mem_args.to_string()
        } else {
            format!("{existing} {low_mem_args}")
        };
        std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", combined);
    }

    floaty::run()
}
