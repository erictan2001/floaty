//! Stored icons: PNG files on disk, addressed by their content.
//!
//! Icons used to live *inside* the store as `data:image/png;base64,…` strings.
//! Measured on a real desktop (36 widgets, 451 icon fields) that was 8.39MB of
//! the store's 8.65MB — 97% of it — and every one of those bytes was rewritten
//! whenever anything saved, backed up on rotation, and moved over IPC once per
//! widget mount.
//!
//! They are files now: `<app data>/icons/<fingerprint>.png`, and a record's
//! `icon` field holds the `http://asset.localhost/…` URL that an `<img>` loads
//! directly — the same URL the frontend's `convertFileSrc` builds, so every
//! reader of `icon` (widgets, folder items, plugins) is unchanged. Identical
//! bytes are one file, so the 451 icon fields of that store become far fewer
//! files rather than 451 copies.
//!
//! Two rules worth keeping:
//!
//! - **A legacy data url is still readable.** Old stores, and any icon the
//!   resolver could not write to disk, keep working through `read`/`pixel_size`
//!   — a storage change must never cost someone their icons.
//! - **An icon is never written in place.** Half a PNG is worse than none: it
//!   loads, it is the wrong size, and `is_missing` would call it fine. Write a
//!   temp file and rename it, exactly like the store does.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

/// Where stored icons live. Set early in setup, from the app data dir — the
/// resolver runs on threads that have no `AppHandle`, so this cannot be a
/// parameter. Replaceable rather than `OnceLock` because tests need their own
/// folder; production sets it exactly once.
static DIR: LazyLock<RwLock<Option<PathBuf>>> = LazyLock::new(|| RwLock::new(None));

/// Install the icons folder.
pub fn set_dir(dir: PathBuf) {
    if let Ok(mut guard) = DIR.write() {
        *guard = Some(dir);
    }
}

pub fn dir() -> Option<PathBuf> {
    DIR.read().ok().and_then(|g| g.clone())
}

/// `http://asset.localhost/<percent-encoded path>` — byte for byte what
/// `convertFileSrc` produces on Windows (Tauri's asset handler percent-decodes
/// the path after the leading `/`, then serves the file if the scope allows it).
///
/// `None` when the path is not valid UTF-8: the caller then inlines the icon,
/// because a mangled URL is a silent 404 on the desktop.
pub fn asset_url(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    let mut out = String::with_capacity(text.len() + 32);
    out.push_str("http://asset.localhost/");
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    Some(out)
}

/// The file behind one of our asset urls.
pub fn path_from_url(icon: &str) -> Option<PathBuf> {
    let rest = icon
        .strip_prefix("http://asset.localhost/")
        .or_else(|| icon.strip_prefix("https://asset.localhost/"))?;
    percent_decode(rest).map(PathBuf::from)
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A content fingerprint: FNV-1a over the bytes, with the length appended so a
/// truncated file can never collide with the whole one.
pub(crate) fn fingerprint(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}{:x}", bytes.len())
}

/// Write a PNG into the icons folder and return its url. Identical bytes are
/// one file; an existing file is never rewritten.
pub fn store_bytes(bytes: &[u8]) -> Option<String> {
    let dir = dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let name = format!("{}.png", fingerprint(bytes));
    let path = dir.join(&name);
    if !path.exists() {
        let tmp = dir.join(format!("{name}.tmp"));
        std::fs::write(&tmp, bytes).ok()?;
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return None;
        }
    }
    asset_url(&path)
}

/// Store a base64 png body (what the resolver's PowerShell hands back).
pub fn store_b64(b64: &str) -> Option<String> {
    store_bytes(&base64_decode(b64)?)
}

/// Store these bytes, or hand back the legacy data url when there is nowhere to
/// put them (no icons folder yet, a read-only app data dir, a non-UTF-8 path).
/// Storing is an optimisation; a missing icon is a bug.
pub fn store_or_inline(bytes: &[u8]) -> String {
    store_bytes(bytes).unwrap_or_else(|| format!("data:image/png;base64,{}", base64_encode(bytes)))
}

/// The same, for a base64 body that came out of the resolver.
pub fn store_b64_or_inline(b64: &str) -> String {
    store_b64(b64).unwrap_or_else(|| format!("data:image/png;base64,{b64}"))
}

/// True for a url this module wrote.
pub fn is_stored_url(icon: &str) -> bool {
    icon.starts_with("http://asset.localhost/") || icon.starts_with("https://asset.localhost/")
}

/// An icon's PNG bytes, whichever form it is stored in.
pub fn read(icon: &str) -> Option<Vec<u8>> {
    if icon.is_empty() || icon == "none" {
        return None;
    }
    if let Some(rest) = icon.strip_prefix("data:") {
        let (head, body) = rest.split_once(',')?;
        return if head.contains("base64") {
            base64_decode(body)
        } else {
            Some(body.as_bytes().to_vec())
        };
    }
    if is_stored_url(icon) {
        return std::fs::read(path_from_url(icon)?).ok();
    }
    // A bare absolute path, which is what a folder item could carry before
    // icons were ever stored.
    if Path::new(icon).is_absolute() {
        return std::fs::read(icon).ok();
    }
    None
}

/// Pixel size, read out of the PNG header (IHDR). `None` when there is no
/// readable icon — the one thing that justifies re-running the resolver.
pub fn pixel_size(icon: &str) -> Option<(u32, u32)> {
    // Only the header is asked for: a legacy inlined icon is ~18KB of base64,
    // and decoding all of it to find out how big the picture is was half of
    // what made the size checks expensive enough to be worth avoiding.
    let head = if let Some(rest) = icon.strip_prefix("data:") {
        let (kind, body) = rest.split_once(',')?;
        if !kind.contains("base64") {
            return None;
        }
        base64_decode_prefix(body, 24)?
    } else {
        let bytes = read(icon)?;
        bytes[..bytes.len().min(24)].to_vec()
    };
    if head.len() < 24 || &head[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes([head[16], head[17], head[18], head[19]]);
    let h = u32::from_be_bytes([head[20], head[21], head[22], head[23]]);
    Some((w, h))
}

/// Nothing usable stored: empty, the literal "none", or an icon we cannot read.
/// Only *this* forces a re-resolution — a 32px icon is the upgrade pass's
/// business, not every mount's.
pub fn is_missing(icon: &str) -> bool {
    if icon.is_empty() || icon == "none" {
        return true;
    }
    // A url whose file is gone *is* missing: that is the whole point of asking
    // the file rather than trusting the string.
    if is_stored_url(icon) {
        return read(icon).is_none();
    }
    if icon.starts_with("data:") {
        return pixel_size(icon).is_none();
    }
    // Anything else is not a form this build writes; call it absent so it gets
    // resolved again instead of rendering as a broken image for ever.
    true
}

/// Smaller than the shell's jumbo size, so the background upgrade pass may try
/// for a crisper one. Never a reason to throw a stored icon away.
pub fn is_low_res(icon: &str) -> bool {
    match pixel_size(icon) {
        Some((w, h)) => w < 64 || h < 64,
        None => true,
    }
}

/// Move every `icon` field in `data` from an inlined data url to a file, and
/// count what moved. Idempotent: a stored url is left alone, so this can run on
/// every launch without touching a migrated store.
pub fn migrate_json(data: &mut serde_json::Value) -> usize {
    fn walk(value: &mut serde_json::Value, moved: &mut usize) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, child) in map.iter_mut() {
                    if key == "icon" {
                        if let serde_json::Value::String(text) = child {
                            if let Some(url) = migrate_one(text) {
                                *text = url;
                                *moved += 1;
                            }
                        }
                    } else {
                        walk(child, moved);
                    }
                }
            }
            serde_json::Value::Array(items) => {
                for item in items.iter_mut() {
                    walk(item, moved);
                }
            }
            _ => {}
        }
    }
    let mut moved = 0;
    walk(data, &mut moved);
    moved
}

/// One icon string: a PNG data url becomes a file, anything else is left as it
/// is (including a data url we could not store, which must not be dropped).
fn migrate_one(icon: &str) -> Option<String> {
    let body = icon.strip_prefix("data:image/png;base64,")?;
    let url = store_b64_or_inline(body);
    if is_stored_url(&url) {
        Some(url)
    } else {
        None
    }
}

/// A stored url for an icon string that may still be a data url.
///
/// `migrate_json` does this to a whole record at startup; this is the same move for one
/// string, for the resolvers, which build data urls of their own and write them straight into
/// records. Anything that is not a PNG data url is returned unchanged — a data url we cannot
/// store must never be dropped.
pub fn store_url(icon: &str) -> String {
    match migrate_one(icon) {
        Some(url) => url,
        None => icon.to_string(),
    }
}

/// Every icon file `data` mentions, as absolute paths.
pub fn collect_referenced(data: &serde_json::Value, out: &mut HashSet<PathBuf>) {
    match data {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                if key == "icon" {
                    if let serde_json::Value::String(text) = child {
                        if let Some(path) = path_from_url(text) {
                            out.insert(path);
                        }
                    }
                } else {
                    collect_referenced(child, out);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_referenced(item, out);
            }
        }
        _ => {}
    }
}

/// Delete icon files nothing refers to any more, and report
/// `(files removed, bytes freed)`.
///
/// The caller decides when this is safe: a store that came back empty (a
/// damaged file, a first run) references nothing, and sweeping then would throw
/// away every icon on the desktop. Only sweep a store that loaded.
pub fn sweep(referenced: &HashSet<PathBuf>) -> (usize, u64) {
    let Some(dir) = dir() else {
        return (0, 0);
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return (0, 0);
    };
    let mut removed = 0;
    let mut freed = 0u64;
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        match path.extension().and_then(|e| e.to_str()) {
            Some("png") => {}
            // Also clears a `.tmp` a kill left behind mid-write.
            Some("tmp") => {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            _ => continue,
        }
        if referenced.contains(&path) {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            freed += meta.len();
        }
        if std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    (removed, freed)
}

/// Standalone base64 encoder with zero external dependencies.
pub fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        result.push(CHARS[(b0 >> 2) as usize] as char);
        result.push(CHARS[(((b0 & 3) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[(((b1 & 0xF) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(b2 & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

/// Minimal base64 decoder (no deps needed), whitespace tolerant.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let chars: Vec<u8> = text.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(chars.len() / 4 * 3);
    for chunk in chars.chunks(4) {
        if chunk.len() < 2 {
            break;
        }
        let pad = chunk.iter().filter(|&&c| c == b'=').count();
        let mut v = [0u8; 4];
        for (i, &c) in chunk.iter().enumerate() {
            v[i] = if c == b'=' { 0 } else { val(c)? };
        }
        out.push((v[0] << 2) | (v[1] >> 4));
        if chunk.len() > 2 && pad < 2 {
            out.push((v[1] << 4) | (v[2] >> 2));
        }
        if chunk.len() > 3 && pad < 1 {
            out.push((v[2] << 6) | v[3]);
        }
    }
    Some(out)
}

/// Decode the first `max_bytes` of a base64 body — enough for the PNG header,
/// without decoding a 250KB icon to ask how big it is.
pub fn base64_decode_prefix(text: &str, max_bytes: usize) -> Option<Vec<u8>> {
    let take = (max_bytes.div_ceil(3) * 4).max(4);
    let head: String = text.chars().take(take).collect();
    let mut bytes = base64_decode(&head)?;
    bytes.truncate(max_bytes);
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// One test at a time: these share the process-wide icons folder.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&13u32.to_be_bytes()); // IHDR length
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&w.to_be_bytes());
        bytes.extend_from_slice(&h.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes.extend_from_slice(&[0, 0, 0, 0]); // crc
        bytes.extend_from_slice(&[0u8; 64]);
        bytes
    }

    fn data_url(bytes: &[u8]) -> String {
        format!("data:image/png;base64,{}", base64_encode(bytes))
    }

    /// A fresh icons folder for one test, and the lock that goes with it.
    fn use_dir(name: &str) -> (PathBuf, MutexGuard<'static, ()>) {
        let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("floaty-icons-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        set_dir(dir.clone());
        (dir, guard)
    }

    #[test]
    fn base64_round_trips_and_tolerates_padding() {
        assert_eq!(base64_encode(b"hello world"), "aGVsbG8gd29ybGQ=");
        for raw in [b"".to_vec(), b"a".to_vec(), b"ab".to_vec(), b"abc".to_vec()] {
            let text = base64_encode(&raw);
            assert_eq!(base64_decode(&text).unwrap(), raw, "{text}");
        }
        // whitespace (PowerShell line wrapping) must not break a decode
        assert_eq!(base64_decode("aGVs\nbG8g\nd29y bGQ=").unwrap(), b"hello world");
        assert!(base64_decode("not base64!!").is_none());
    }

    #[test]
    fn the_asset_url_is_what_the_webview_asks_for() {
        let path = Path::new(r"C:\Users\e\AppData\Roaming\com.floaty.app\icons\ab.png");
        let url = asset_url(path).unwrap();
        assert_eq!(
            url,
            "http://asset.localhost/C%3A%5CUsers%5Ce%5CAppData%5CRoaming%5Ccom.floaty.app%5Cicons%5Cab.png"
        );
        // and it decodes back to the same path, which is what the handler does
        assert_eq!(path_from_url(&url).unwrap(), path.to_path_buf());
        assert!(asset_url(Path::new("/tmp/a b.png")).unwrap().contains("%20"));
    }

    #[test]
    fn equal_bytes_are_one_file_and_an_icon_is_never_half_written() {
        let (dir, _guard) = use_dir("store");
        let bytes = png_bytes(256, 256);
        let first = store_bytes(&bytes).unwrap();
        let second = store_bytes(&bytes).unwrap();
        assert_eq!(first, second, "same bytes must be the same file");
        assert_ne!(store_bytes(&png_bytes(32, 32)).unwrap(), first);
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        files.sort();
        assert_eq!(files.len(), 2, "two distinct icons, two files: {files:?}");
        assert!(!files.iter().any(|f| f.ends_with(".tmp")));
        // and the stored url reads back as the icon it was
        assert_eq!(read(&first).unwrap(), bytes);
        assert_eq!(pixel_size(&first), Some((256, 256)));
        assert!(!is_low_res(&first));
        assert!(!is_missing(&first));
    }

    /// The resolvers build PNG data urls of their own and hand them to code that writes them
    /// straight into records — which is how the store got back to 480 inline icons after the
    /// startup migration had emptied it.
    #[test]
    fn store_url_moves_a_png_data_url_into_a_file_and_leaves_the_rest_alone() {
        let (_dir, _guard) = use_dir("store-url");
        let bytes = png_bytes(256, 256);
        let legacy = data_url(&bytes);
        let stored = store_url(&legacy);
        assert!(is_stored_url(&stored), "a png data url becomes a file: {stored}");
        assert_eq!(read(&stored).unwrap(), bytes);
        // idempotent, and it never drops something it cannot store
        assert_eq!(store_url(&stored), stored);
        assert_eq!(store_url("none"), "none");
        assert_eq!(store_url(""), "");
        let svg = "data:image/svg+xml;base64,PHN2Zy8+";
        assert_eq!(store_url(svg), svg, "only PNGs are stored; anything else is left as it is");
    }

    #[test]
    fn a_legacy_data_url_still_reads_and_migrates_once() {
        let (_dir, _guard) = use_dir("legacy");
        let bytes = png_bytes(48, 48);
        let legacy = data_url(&bytes);
        // readable while it is still inlined
        assert_eq!(pixel_size(&legacy), Some((48, 48)));
        assert!(is_low_res(&legacy), "48px is low-res");
        assert!(!is_missing(&legacy));

        let mut data = serde_json::json!({
            "name": "Arc",
            "icon": legacy,
            "items": [{ "target": "a.lnk", "icon": data_url(&png_bytes(256, 256)) }],
        });
        assert_eq!(migrate_json(&mut data), 2, "both fields move");
        let moved = data["icon"].as_str().unwrap().to_string();
        assert!(is_stored_url(&moved));
        assert!(is_stored_url(data["items"][0]["icon"].as_str().unwrap()));
        assert_eq!(read(&moved).unwrap(), bytes);
        assert_eq!(pixel_size(&moved), Some((48, 48)));
        // idempotent: a second pass changes nothing and moves nothing
        assert_eq!(migrate_json(&mut data), 0);
        assert_eq!(data["icon"].as_str().unwrap(), moved);
    }

    #[test]
    fn a_url_whose_file_is_gone_is_missing_again() {
        let (_dir, _guard) = use_dir("gone");
        let bytes = png_bytes(256, 256);
        let url = store_bytes(&bytes).unwrap();
        assert!(!is_missing(&url));
        assert!(!is_low_res(&url));
        std::fs::remove_file(path_from_url(&url).unwrap()).unwrap();
        assert!(is_missing(&url), "the file is what makes an icon real");
        assert!(is_low_res(&url));
    }

    #[test]
    fn only_missing_or_unreadable_icons_are_resolved_again() {
        let (_dir, _guard) = use_dir("missing");
        assert!(is_missing(""));
        assert!(is_missing("none"));
        assert!(is_missing("data:image/png;base64,not a png at all"));
        assert!(is_missing(r"C:\nope\not-there.png"));
        // a readable PNG, of any size, is not "missing" — only low-res
        let small = data_url(&png_bytes(32, 32));
        assert!(!is_missing(&small));
        assert!(is_low_res(&small));
        let big = data_url(&png_bytes(256, 256));
        assert!(!is_missing(&big));
        assert!(!is_low_res(&big));
    }

    #[test]
    fn only_the_icons_the_records_mention_survive_a_sweep() {
        let (dir, _guard) = use_dir("sweep");
        let kept = store_bytes(&png_bytes(256, 256)).unwrap();
        let orphan = store_bytes(&png_bytes(32, 32)).unwrap();
        let data = serde_json::json!({ "icon": kept, "items": [{ "icon": "none" }] });
        let mut referenced = HashSet::new();
        collect_referenced(&data, &mut referenced);
        assert_eq!(referenced.len(), 1);

        std::fs::write(dir.join("leftover.png.tmp"), b"half a write").unwrap();
        let (removed, freed) = sweep(&referenced);
        assert_eq!(removed, 1, "only the orphan goes");
        assert!(freed > 0);
        assert!(
            !dir.join("leftover.png.tmp").exists(),
            "and the tmp is cleared"
        );
        assert!(path_from_url(&kept).unwrap().exists());
        assert!(!path_from_url(&orphan).unwrap().exists());
    }

    #[test]
    fn an_icon_that_cannot_be_stored_is_inlined_instead_of_lost() {
        let bytes = png_bytes(256, 256);
        let inline = format!("data:image/png;base64,{}", base64_encode(&bytes));
        // the inline form is a first-class icon: readable, sized, not missing
        assert_eq!(pixel_size(&inline), Some((256, 256)));
        assert!(!is_missing(&inline));
        // and the fallback never invents bytes it could not decode
        assert_eq!(store_b64("not base64!!"), None);
        assert!(store_b64_or_inline("not base64!!").starts_with("data:"));
    }
}
