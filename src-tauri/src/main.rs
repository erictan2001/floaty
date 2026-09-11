#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    {
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
