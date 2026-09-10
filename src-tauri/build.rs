fn main() {
    // fingerprint the binary so floaty.log proves which backend actually ran
    println!(
        "cargo:rustc-env=FLOATY_BUILD_MARK={}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_else(|_| "unknown".to_string())
    );
    tauri_build::build()
}
