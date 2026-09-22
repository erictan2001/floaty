//! `app_discovery_launch` — moved out of lib.rs verbatim.
//!
//! Visibility is `pub(crate)` because the rest of the crate calls in across the module
//! boundary now; nothing here changed shape on the way out.

use crate::*;
use serde::{ Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

// ---------- app discovery + launch ----------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DiscoveredApp {
    pub(crate) name: String,
    pub(crate) path: String,
}

pub(crate) fn collect_lnk(dir: &str, out: &mut Vec<DiscoveredApp>, seen: &mut HashSet<String>, depth: u8) {
    if depth > 2 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if depth < 2 {
                collect_lnk(&p.to_string_lossy(), out, seen, depth + 1);
            }
            continue;
        }
        let is_lnk = p
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case("lnk"))
            .unwrap_or(false);
        if !is_lnk {
            continue;
        }
        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let lower = stem.to_lowercase();
        if stem.is_empty() || lower.contains("uninstall") || lower.contains("unins0") {
            continue;
        }
        if seen.insert(lower) {
            out.push(DiscoveredApp {
                name: stem,
                path: p.to_string_lossy().to_string(),
            });
        }
    }
}

/// Blocking filesystem enumeration, shared by the async command (via
/// spawn_blocking, so a slow/cloud-backed Start Menu walk can never stall the
/// command pipeline and hang the settings window) and the setup demo harness.
pub(crate) fn scan_apps_blocking() -> Vec<DiscoveredApp> {
    let mut dirs: Vec<String> = vec![];
    if let Ok(v) = std::env::var("APPDATA") {
        dirs.push(format!("{v}\\Microsoft\\Windows\\Start Menu\\Programs"));
    }
    if let Ok(v) = std::env::var("PROGRAMDATA") {
        dirs.push(format!("{v}\\Microsoft\\Windows\\Start Menu\\Programs"));
    }
    if let Ok(v) = std::env::var("USERPROFILE") {
        dirs.push(format!("{v}\\Desktop"));
    }
    dirs.push("C:\\Users\\Public\\Desktop".to_string());
    let mut out: Vec<DiscoveredApp> = vec![];
    let mut seen: HashSet<String> = HashSet::new();
    for d in &dirs {
        collect_lnk(d, &mut out, &mut seen, 0);
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out.truncate(250);
    out
}

#[tauri::command]
pub(crate) async fn floaty_scan_apps(app: AppHandle) -> Vec<DiscoveredApp> {
    let out = tauri::async_runtime::spawn_blocking(scan_apps_blocking)
        .await
        .unwrap_or_default();
    log_line(&app, &format!("scan found {} apps", out.len()));
    out
}

/// Parse ICO binary format directly in Rust and extract the largest embedded PNG frame if available
pub(crate) fn try_extract_png_from_ico_bytes(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 22 {
        return None;
    }
    let id_type = u16::from_le_bytes([bytes[2], bytes[3]]);
    if id_type != 1 {
        return None;
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    if count == 0 || bytes.len() < 6 + count * 16 {
        return None;
    }

    let mut best_width = 0u32;
    let mut best_data: Option<&[u8]> = None;

    for i in 0..count {
        let entry = 6 + i * 16;
        let raw_w = bytes[entry] as u32;
        let w = if raw_w == 0 { 256 } else { raw_w };
        let bytes_in_res = u32::from_le_bytes([
            bytes[entry + 8],
            bytes[entry + 9],
            bytes[entry + 10],
            bytes[entry + 11],
        ]) as usize;
        let image_offset = u32::from_le_bytes([
            bytes[entry + 12],
            bytes[entry + 13],
            bytes[entry + 14],
            bytes[entry + 15],
        ]) as usize;

        if image_offset + bytes_in_res <= bytes.len() && bytes_in_res >= 8 {
            let img = &bytes[image_offset..image_offset + bytes_in_res];
            // PNG magic signature: 0x89 'P' 'N' 'G' 0x0D 0x0A 0x1A 0x0A
            if img.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])
                && w > best_width {
                    best_width = w;
                    best_data = Some(img);
                }
        }
    }

    best_data.map(|d| d.to_vec())
}

/// Instantly extract a PNG frame from an ICO file on disk in Rust (sub-millisecond)
pub(crate) fn try_extract_fast_ico(ico_path: &str) -> Option<String> {
    let p = std::path::Path::new(ico_path);
    if !p.is_file() {
        return None;
    }
    let bytes = std::fs::read(p).ok()?;
    let png = try_extract_png_from_ico_bytes(&bytes)?;
    Some(icons::store_or_inline(&png))
}

/// Instantly extract Steam game and internet shortcut (.url) icons in Rust (sub-millisecond)
pub(crate) fn try_extract_fast_url(url_path: &str) -> Option<String> {
    let p = std::path::Path::new(url_path);
    if !p.is_file() {
        return None;
    }
    let content = std::fs::read_to_string(p).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("IconFile=") {
            let ico_path = rest.trim();
            if ico_path.ends_with(".ico") || ico_path.ends_with(".ICO") {
                if let Some(data_url) = try_extract_fast_ico(ico_path) {
                    return Some(data_url);
                }
            }
        }
    }
    None
}

/// A resolved icon plus a fingerprint of the file it came from, so a replaced
/// or re-downloaded file refreshes instead of serving the old icon forever
/// (that was the "some tiles use the previous cache" complaint).
#[derive(Clone)]
pub(crate) struct CachedIcon {
    pub(crate) data: String,
    pub(crate) stamp: Option<(u64, u64)>,
}

pub(crate) fn path_stamp(path: &str) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some((mtime, meta.len()))
}

pub(crate) static ICON_CACHE: std::sync::LazyLock<Mutex<HashMap<String, CachedIcon>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) fn cache_icon(path: &str, data: &str) {
    if let Ok(mut guard) = ICON_CACHE.lock() {
        guard.insert(
            path.to_string(),
            CachedIcon {
                data: data.to_string(),
                stamp: path_stamp(path),
            },
        );
    }
}

/// Resolve icons for `paths`. Cached entries are reused while the file is
/// unchanged; `force` skips the cache so the upgrade pass can really try for a
/// crisper icon instead of being handed back the 32px one it wants to replace.
pub(crate) fn resolve_icons_batch(paths: &[String], force: bool) -> HashMap<String, String> {
    // Every value this hands out is a *stored* url. Its callers write them straight into
    // records, and a data url there is what put the store back to 8.5 MB after the startup
    // migration had emptied it: measured, the background pass re-inlined all 477 icons at
    // 13:35 and the store was 480 inline / 0 asset by 13:36, while the backup it was meant to
    // replace held 477 asset / 3 inline. The fast paths here build PNG data urls in Rust and
    // the in-memory cache holds whatever came back, so the fix belongs on the way *out*.
    resolve_icons_batch_raw(paths, force)
        .into_iter()
        .map(|(target, icon)| (target, icons::store_url(&icon)))
        .collect()
}

pub(crate) fn resolve_icons_batch_raw(paths: &[String], force: bool) -> HashMap<String, String> {
    let mut results = HashMap::new();
    let mut needed = Vec::new();

    if let Ok(guard) = ICON_CACHE.lock() {
        for p in paths {
            if !force {
                if let Some(cached) = guard.get(p) {
                    if cached.stamp == path_stamp(p) {
                        results.insert(p.clone(), cached.data.clone());
                        continue;
                    }
                }
            }
            needed.push(p.clone());
        }
    } else {
        needed.extend_from_slice(paths);
    }

    if needed.is_empty() {
        return results;
    }

    // 1. Rust-native fast path for .url and .ico (executes in 0ms without spawning processes)
    let mut still_needed = Vec::new();
    for p in &needed {
        let fast_res = if p.ends_with(".url") || p.ends_with(".URL") {
            try_extract_fast_url(p)
        } else if p.ends_with(".ico") || p.ends_with(".ICO") {
            try_extract_fast_ico(p)
        } else {
            None
        };

        if let Some(icon_data) = fast_res {
            results.insert(p.clone(), icon_data.clone());
            cache_icon(p, &icon_data);
        } else {
            still_needed.push(p.clone());
        }
    }

    if still_needed.is_empty() {
        return results;
    }

    #[cfg(windows)]
    {
        use std::io::Write as _;
        use std::os::windows::process::CommandExt;
        const NO_WINDOW: u32 = 0x08000000;

        let script = r#"
# Explicit UTF-8 on both pipes: the console codepage mangles any path with
# non-ASCII characters, which silently skipped those icons entirely.
$reader = New-Object System.IO.StreamReader([Console]::OpenStandardInput(), (New-Object Text.UTF8Encoding($false)))
$writer = New-Object System.IO.StreamWriter([Console]::OpenStandardOutput(), (New-Object Text.UTF8Encoding($false)))
$writer.AutoFlush = $true
Add-Type -AssemblyName System.Drawing;
# PresentationCore/WindowsBase load WPF, which is what converts the shell's
# HBITMAP to PNG with its alpha channel intact.
Add-Type -AssemblyName PresentationCore, WindowsBase;

$IconFetchSrc = @'
using System;
using System.Runtime.InteropServices;
using System.Drawing;
using System.Drawing.Imaging;
using System.IO;
public static class IconFetch {
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern uint PrivateExtractIcons(string szFileName, int nIconIndex, int cxIcon, int cyIcon, IntPtr[] phicon, uint[] piconid, uint nIcons, uint flags);
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr SHGetFileInfo(string pszPath, uint dwFileAttributes, ref SHFILEINFO psfi, uint cbFileInfo, uint uFlags);
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct SHFILEINFO {
        public IntPtr hIcon;
        public int iIcon;
        public uint dwAttributes;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] public string szDisplayName;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 80)] public string szTypeName;
    }
    [DllImport("user32.dll")]
    public static extern bool DestroyIcon(IntPtr hIcon);
    [DllImport("gdi32.dll")]
    public static extern bool DeleteObject(IntPtr hObject);

    // The shell's own item image: the exact bitmap Explorer draws for a path,
    // and the only route that returns more than 32px for file types whose
    // registered icon is a small resource (pdf / txt / zip / md / folders).
    [ComImport, Guid("bcc18b79-ba16-442f-80c4-8a59c30c463b"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IShellItemImageFactory {
        void GetImage(SIZE size, int flags, out IntPtr phbm);
    }
    [StructLayout(LayoutKind.Sequential)]
    public struct SIZE { public int cx; public int cy; }
    [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
    public static extern void SHCreateItemFromParsingName(string path, IntPtr pbc, ref Guid riid, [MarshalAs(UnmanagedType.Interface)] out object ppv);

    public static IntPtr GetShellImageHandle(string path, int px) {
        Guid iid = new Guid("bcc18b79-ba16-442f-80c4-8a59c30c463b");
        object o;
        SHCreateItemFromParsingName(path, IntPtr.Zero, ref iid, out o);
        var factory = (IShellItemImageFactory)o;
        IntPtr hbm;
        factory.GetImage(new SIZE { cx = px, cy = px }, 4, out hbm); // 4 = SIIGBF_ICONONLY
        return hbm;
    }

    // How much of the frame the artwork actually covers: 0-100, the larger of the
    // width/height span. Some apps ship a 256px frame whose art is drawn tiny in
    // the middle; the shell hands that frame back as-is, so a tile built from it
    // looks like a minimised icon next to the others.
    public static int ArtSpanPercent(string pngB64) {
        try {
            byte[] bytes = Convert.FromBase64String(pngB64);
            using (var ms = new MemoryStream(bytes, false))
            using (var bmp = new Bitmap(ms)) {
                int w = bmp.Width, h = bmp.Height;
                var data = bmp.LockBits(new Rectangle(0, 0, w, h),
                    ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
                try {
                    int stride = data.Stride;
                    byte[] buf = new byte[stride * h];
                    Marshal.Copy(data.Scan0, buf, 0, buf.Length);
                    int minX = w, minY = h, maxX = -1, maxY = -1;
                    for (int y = 0; y < h; y++) {
                        int row = y * stride;
                        for (int x = 0; x < w; x++) {
                            if (buf[row + x * 4 + 3] > 96) {
                                if (x < minX) minX = x;
                                if (x > maxX) maxX = x;
                                if (y < minY) minY = y;
                                if (y > maxY) maxY = y;
                            }
                        }
                    }
                    if (maxX < 0) return 0;
                    int pctW = (maxX - minX + 1) * 100 / w;
                    int pctH = (maxY - minY + 1) * 100 / h;
                    return pctW > pctH ? pctW : pctH;
                } finally {
                    bmp.UnlockBits(data);
                }
            }
        } catch { return -1; }
    }
}
'@

Add-Type -TypeDefinition $IconFetchSrc -ReferencedAssemblies System.Drawing;

# Bitmap -> PNG base64 for a shell icon handle, releasing the handle afterwards.
function Get-HiconB64([IntPtr]$hIcon) {
    if ($hIcon -eq [IntPtr]::Zero) { return $null }
    $res = $null
    try {
        $ico = [System.Drawing.Icon]::FromHandle($hIcon)
        $bmp = $ico.ToBitmap()
        $ms = New-Object IO.MemoryStream
        $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
        $res = [Convert]::ToBase64String($ms.ToArray())
        $ms.Dispose(); $bmp.Dispose()
    } catch { $res = $null }
    [void][IconFetch]::DestroyIcon($hIcon)
    return $res
}

# Real 256px icon straight out of an exe/dll/ico. ExtractAssociatedIcon only ever
# returns 32x32, which is what made app icons look soft.
function Get-BigIconB64($spec) {
    $path = $spec
    $index = 0
    if ($spec -match '^(.*),(\d+)$') { $path = $matches[1]; [void][int]::TryParse($matches[2], [ref]$index) }
    if (-not (Test-Path -LiteralPath $path)) { return $null }
    try {
        $handles = New-Object IntPtr[] 1
        $ids = New-Object uint32[] 1
        $n = [IconFetch]::PrivateExtractIcons($path, $index, 256, 256, $handles, $ids, 1, 0)
        if ($n -ge 1 -and $handles[0] -ne [IntPtr]::Zero) { return Get-HiconB64 $handles[0] }
    } catch {}
    return $null
}

# Icon bitmap straight out of the shell's item image factory. This is what
# Explorer itself draws, and the only way to get 256px for types whose
# registered icon is a 32px resource (.pdf/.txt/.zip/.md/.py/.json, folders).
# The HBITMAP -> PNG step goes through WPF on purpose: System.Drawing's
# Bitmap.FromHbitmap throws the alpha channel away, which turned every shell
# icon into a black square on the desktop.
function Get-ShellImageB64($path) {
    if ([string]::IsNullOrEmpty($path) -or -not (Test-Path -LiteralPath $path)) { return $null }
    $ms = $null
    try {
        $hbm = [IconFetch]::GetShellImageHandle($path, 256)
        if ($hbm -eq [IntPtr]::Zero) { return $null }
        try {
            $src = [System.Windows.Interop.Imaging]::CreateBitmapSourceFromHBitmap($hbm, [IntPtr]::Zero, [System.Windows.Int32Rect]::Empty, [System.Windows.Media.Imaging.BitmapSizeOptions]::FromEmptyOptions())
            $enc = New-Object System.Windows.Media.Imaging.PngBitmapEncoder
            $enc.Frames.Add([System.Windows.Media.Imaging.BitmapFrame]::Create($src))
            $ms = New-Object IO.MemoryStream
            $enc.Save($ms)
            return [Convert]::ToBase64String($ms.ToArray())
        } finally {
            [void][IconFetch]::DeleteObject($hbm)
        }
    } catch { return $null }
    finally { if ($ms) { $ms.Dispose() } }
}

# Directories have no file to extract from, so ask the shell for their glyph.
function Get-DirIconB64($path) {
    $shellImg = Get-ShellImageB64 $path
    if (-not [string]::IsNullOrEmpty($shellImg)) { return $shellImg }
    try {
        $info = New-Object 'IconFetch+SHFILEINFO'
        $size = [Runtime.InteropServices.Marshal]::SizeOf($info)
        $flags = 0x100
        $r = [IconFetch]::SHGetFileInfo($path, 0x10, [ref]$info, [uint32]$size, $flags)
        if ($r -ne [IntPtr]::Zero -and $info.hIcon -ne [IntPtr]::Zero) { return Get-HiconB64 $info.hIcon }
    } catch {}
    return $null
}

function Get-IconB64($filePath) {
    if ([string]::IsNullOrEmpty($filePath) -or -not (Test-Path -LiteralPath $filePath)) { return $null }
    if (Test-Path -LiteralPath $filePath -PathType Container) { return Get-DirIconB64 $filePath }

    if ($filePath.EndsWith('.ico', [StringComparison]::OrdinalIgnoreCase)) {
        # A .ico holds several frames, usually including a 256px one in BMP
        # format. PrivateExtractIcons picks the biggest; the PNG-only frame scan
        # below returned 32-48px for these (Sprite/Steam/App icons).
        $bigIco = Get-BigIconB64 $filePath
        if (-not [string]::IsNullOrEmpty($bigIco)) { return $bigIco }
        try {
            $bytes = [System.IO.File]::ReadAllBytes($filePath)
            if ($bytes.Length -ge 22) {
                $cnt = [BitConverter]::ToUInt16($bytes, 4)
                $bestIdx = -1; $bestW = 0
                for ($i = 0; $i -lt $cnt; $i++) {
                    $off = 6 + $i * 16
                    $w = [int]$bytes[$off]
                    if ($w -eq 0) { $w = 256 }
                    $imgOff = [BitConverter]::ToUInt32($bytes, $off + 12)
                    if ($imgOff + 8 -le $bytes.Length -and $bytes[$imgOff] -eq 0x89 -and $bytes[$imgOff+1] -eq 0x50) {
                        if ($w -gt $bestW) { $bestW = $w; $bestIdx = $i }
                    }
                }
                if ($bestIdx -ge 0) {
                    $off = 6 + $bestIdx * 16
                    $imgOff = [BitConverter]::ToUInt32($bytes, $off + 12)
                    $imgLen = [BitConverter]::ToUInt32($bytes, $off + 8)
                    if ($imgOff + $imgLen -le $bytes.Length) {
                        return [Convert]::ToBase64String($bytes, $imgOff, $imgLen)
                    }
                }
            }
        } catch {}

        try {
            $ico = New-Object System.Drawing.Icon($filePath, 256, 256)
            $bmp = $ico.ToBitmap()
            $ms = New-Object IO.MemoryStream
            $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
            $res = [Convert]::ToBase64String($ms.ToArray())
            $ms.Dispose(); $bmp.Dispose(); $ico.Dispose()
            return $res
        } catch {}

        try {
            $bmp = [System.Drawing.Bitmap]::FromFile($filePath)
            $ms = New-Object IO.MemoryStream
            $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
            $res = [Convert]::ToBase64String($ms.ToArray())
            $ms.Dispose(); $bmp.Dispose()
            return $res
        } catch {}
    }

    # Two routes disagree for some apps. The shell returns the app's real 256px
    # frame, which a few ship with the artwork drawn tiny in the middle (Sandboxie's
    # SandMan.exe), while PrivateExtractIcons upscales a full-bleed frame. So try
    # the route that usually wins for this kind, and take the other one as well
    # when the first does not fill the frame: a small glyph in a big square reads
    # as a minimised tile next to the other icons.
    $order = if ($filePath -match '\.(exe|dll|ocx|scr|cpl|msi|sys|com)$') { @('big', 'shell') } else { @('shell', 'big') }
    $best = $null
    $bestSpan = -1
    foreach ($route in $order) {
        $cand = if ($route -eq 'big') { Get-BigIconB64 $filePath } else { Get-ShellImageB64 $filePath }
        if ([string]::IsNullOrEmpty($cand)) { continue }
        $span = [IconFetch]::ArtSpanPercent($cand)
        if ($span -ge 60) { return $cand }
        if ($span -gt $bestSpan) { $bestSpan = $span; $best = $cand }
    }
    if (-not [string]::IsNullOrEmpty($best)) { return $best }

    # no icon inside the file itself: ask the shell which icon the file *type*
    # uses and pull that one at 256px. Explorer's per-user choice comes first:
    # the machine-wide ProgID often has no DefaultIcon (that is why .pdf fell
    # back to a 32x32 icon even though the shell shows a crisp one).
    try {
        $ext = [System.IO.Path]::GetExtension($filePath)
        if (-not [string]::IsNullOrEmpty($ext)) {
            $candidates = New-Object System.Collections.ArrayList
            $choice = (Get-ItemProperty -LiteralPath ('Registry::HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\' + $ext + '\UserChoice') -ErrorAction SilentlyContinue).ProgId
            if (-not [string]::IsNullOrEmpty($choice)) { [void]$candidates.Add($choice) }
            $progDefault = (Get-ItemProperty -LiteralPath ('Registry::HKEY_CLASSES_ROOT\' + $ext) -ErrorAction SilentlyContinue).'(default)'
            if (-not [string]::IsNullOrEmpty($progDefault)) { [void]$candidates.Add($progDefault) }
            foreach ($cand in $candidates) {
                $specs = New-Object System.Collections.ArrayList
                $defIcon = (Get-ItemProperty -LiteralPath ('Registry::HKEY_CLASSES_ROOT\' + $cand + '\DefaultIcon') -ErrorAction SilentlyContinue).'(default)'
                if (-not [string]::IsNullOrEmpty($defIcon) -and $defIcon -notmatch '^@\{') { [void]$specs.Add($defIcon) }
                if ($cand -match '^Applications\(.+\.exe)$') { [void]$specs.Add($matches[1]) }
                elseif ($cand -match '\.exe$') { [void]$specs.Add($cand) }
                foreach ($spec in $specs) {
                    $parts = $spec -split ','
                    $iconPath = [System.Environment]::ExpandEnvironmentVariables($parts[0].Trim('"', ' '))
                    $iconIdx = 0
                    if ($parts.Count -gt 1) { [void][int]::TryParse($parts[1].Trim(), [ref]$iconIdx) }
                    if (Test-Path -LiteralPath $iconPath) {
                        $typed = Get-BigIconB64($iconPath + ',' + $iconIdx)
                        if ([string]::IsNullOrEmpty($typed)) { $typed = Get-BigIconB64 $iconPath }
                        if (-not [string]::IsNullOrEmpty($typed)) { return $typed }
                    }
                }
            }
        }
    } catch {}

    try {
        $ico = [System.Drawing.Icon]::ExtractAssociatedIcon($filePath)
        if ($null -ne $ico) {
            $ms = New-Object IO.MemoryStream
            $bmp = $ico.ToBitmap()
            $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
            $res = [Convert]::ToBase64String($ms.ToArray())
            $ms.Dispose(); $bmp.Dispose(); $ico.Dispose()
            return $res
        }
    } catch {}

    return $null
}

$sh = $null
while ($true) {
    $p = $reader.ReadLine()
    if ($null -eq $p) { break }
    $p = $p.Trim()
    if ([string]::IsNullOrEmpty($p) -or -not (Test-Path -LiteralPath $p)) { continue }
    $b64 = $null

    if ($p.EndsWith('.url', [StringComparison]::OrdinalIgnoreCase)) {
        try {
            $txt = [System.IO.File]::ReadAllText($p)
            if ($txt -match 'IconFile=([^\r\n]+)') {
                $icoPath = $matches[1].Trim()
                $b64 = Get-IconB64 $icoPath
            }
        } catch {}
    }

    if ([string]::IsNullOrEmpty($b64) -and $p.EndsWith('.lnk', [StringComparison]::OrdinalIgnoreCase)) {
        try {
            if ($null -eq $sh) { $sh = New-Object -ComObject WScript.Shell }
            $sc = $sh.CreateShortcut($p)
            if (-not [string]::IsNullOrEmpty($sc.IconLocation)) {
                $loc = ($sc.IconLocation -split ',')[0].Trim()
                if (-not [string]::IsNullOrEmpty($loc) -and [System.IO.File]::Exists($loc)) {
                    $b64 = Get-IconB64 $loc
                }
            }
            if ([string]::IsNullOrEmpty($b64) -and -not [string]::IsNullOrEmpty($sc.TargetPath)) {
                if ([System.IO.File]::Exists($sc.TargetPath)) {
                    $b64 = Get-IconB64 $sc.TargetPath
                }
            }
        } catch {}
    }

    if ([string]::IsNullOrEmpty($b64)) {
        $b64 = Get-IconB64 $p
    }

    if (-not [string]::IsNullOrEmpty($b64)) {
        $writer.WriteLine($p + '|' + $b64)
    } else {
        $writer.WriteLine($p + '|none')
    }
}
$writer.Flush()
"#;

        if let Ok(mut child) = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .creation_flags(NO_WINDOW)
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                for p in &still_needed {
                    let _ = writeln!(stdin, "{}", p);
                }
            }
            if let Ok(output) = child.wait_with_output() {
                if output.status.success() {
                    let raw = String::from_utf8_lossy(&output.stdout);
                    for line in raw.lines() {
                        let trimmed = line.trim();
                        if let Some((p, b64)) = trimmed.split_once('|') {
                            let val = if b64 == "none" {
                                "none".to_string()
                            } else {
                                icons::store_b64_or_inline(b64)
                            };
                            results.insert(p.to_string(), val.clone());
                            cache_icon(p, &val);
                        }
                    }
                }
            }
        }
    }

    results
}

pub(crate) fn resolve_icon_url(lnk_path: &str) -> Option<String> {
    // resolve_icons_batch consults ICON_CACHE (with its file stamp) for us.
    let map = resolve_icons_batch(&[lnk_path.to_string()], false);
    let res = map.get(lnk_path)?;
    if res == "none" {
        None
    } else {
        Some(res.clone())
    }
}

#[tauri::command]
pub(crate) async fn floaty_icon(id: String, app: AppHandle) -> Result<String, String> {
    // serve cached icon only if it's already high resolution
    {
        let state = app.state::<AppState>();
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        if let Some(r) = guard.widgets.get(&id) {
            if let Some(s) = r.data.get("icon").and_then(|v| v.as_str()) {
                // Serve whatever we already have: only a *missing* icon is worth
                // another PowerShell round-trip (size-based "low-res" is the
                // background upgrade pass's business, not every mount's).
                if !icons::is_missing(s) {
                    return Ok(s.to_string());
                }
            }
        }
    }
    let target = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        guard
            .widgets
            .get(&id)
            .filter(|r| is_path_kind(&r.kind))
            .and_then(|r| {
                r.data
                    .get("target")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .ok_or("launcher not found")?
    };
    if target.trim().is_empty() {
        return Err("launcher has no target".into());
    }
    let target_clone = target.clone();
    let icon_url = tauri::async_runtime::spawn_blocking(move || resolve_icon_url(&target_clone))
        .await
        .map_err(|e| e.to_string())?
        .unwrap_or_else(|| "none".to_string());
    {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        if let Some(r) = guard.widgets.get_mut(&id) {
            // a re-resolution must never downgrade an icon that is already good:
            // remounts used to overwrite crisp 256px icons with 32px ones
            let existing = r
                .data
                .get("icon")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let keep_existing = !existing.is_empty()
                && existing != "none"
                && !icons::is_low_res(&existing)
                && icons::is_low_res(&icon_url);
            if !keep_existing {
                if let Some(obj) = r.data.as_object_mut() {
                    obj.insert(
                        "icon".to_string(),
                        serde_json::Value::String(icon_url.clone()),
                    );
                }
            }
        }
    }
    persist(&app);
    Ok(icon_url)
}

/// Background task that automatically detects any legacy 32x32 icons in saved app launchers
/// or folders and upgrades them to crisp native high-res icons (up to 256x256).
pub(crate) async fn upgrade_low_res_icons(app: &AppHandle) {
    // A pipeline bump invalidates every stored icon once. Without this, icons an
    // older resolver produced (black-background squares, 32px blanks) look
    // "good enough" to the size test and would never be replaced.
    let migrate = load_settings(app).icon_pipeline < ICON_PIPELINE;

    let to_upgrade: Vec<(String, String)> = {
        let state = app.state::<AppState>();
        let Ok(guard) = state.0.lock() else { return };
        guard
            .widgets
            .iter()
            .filter(|(_, r)| is_path_kind(&r.kind))
            .filter_map(|(id, r)| {
                let target = r.data.get("target")?.as_str()?.to_string();
                let icon = r.data.get("icon").and_then(|v| v.as_str()).unwrap_or("");
                if (migrate || icons::is_low_res(icon)) && !target.trim().is_empty() {
                    Some((id.clone(), target))
                } else {
                    None
                }
            })
            .collect()
    };

    let folders_to_check: Vec<(String, Vec<String>)> = {
        let state = app.state::<AppState>();
        let Ok(guard) = state.0.lock() else { return };
        guard
            .widgets
            .iter()
            .filter(|(_, r)| r.kind == "folder")
            .filter_map(|(id, r)| {
                let needed: Vec<String> = folder_items(r)
                    .into_iter()
                    .filter(|it| {
                        // directories included: the shell has a jumbo folder glyph,
                        // and skipping them left every subfolder blank
                        (migrate || icons::is_low_res(&it.icon)) && !it.target.trim().is_empty()
                    })
                    .map(|it| it.target)
                    .collect();
                if needed.is_empty() {
                    None
                } else {
                    Some((id.clone(), needed))
                }
            })
            .collect()
    };

    let stamp_pipeline = || {
        let mut settings = load_settings(app);
        settings.icon_pipeline = ICON_PIPELINE;
        if let Ok(json) = serde_json::to_string_pretty(&settings) {
            write_text_atomic(&settings_file(app), &json);
        }
        log_line(app, &format!("icon pipeline: stored icons re-resolved to v{ICON_PIPELINE}"));
    };

    if to_upgrade.is_empty() && folders_to_check.is_empty() {
        if migrate {
            stamp_pipeline();
        }
        return;
    }

    let mut all_paths: Vec<String> = to_upgrade.iter().map(|(_, t)| t.clone()).collect();
    for (_, needed) in &folders_to_check {
        all_paths.extend(needed.iter().cloned());
    }
    all_paths.sort();
    all_paths.dedup();

    log_line(app, &format!("upgrade_low_res_icons: batch resolving {} icons", all_paths.len()));

    let icon_map =
        tauri::async_runtime::spawn_blocking(move || resolve_icons_batch(&all_paths, true))
            .await
            .unwrap_or_default();

    let mut changed = false;
    {
        let state = app.state::<AppState>();
        let Ok(mut guard) = state.0.lock() else { return };
        for (id, target) in &to_upgrade {
            if let Some(hi_res) = icon_map.get(target) {
                if hi_res != "none" {
                    if let Some(r) = guard.widgets.get_mut(id) {
                        let cur = r.data.get("icon").and_then(|v| v.as_str()).unwrap_or("");
                        if cur != hi_res {
                            if let Some(obj) = r.data.as_object_mut() {
                                obj.insert("icon".to_string(), serde_json::Value::String(hi_res.clone()));
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
        for (folder_id, needed) in &folders_to_check {
            if let Some(r) = guard.widgets.get_mut(folder_id) {
                let mut items = folder_items(r);
                let mut folder_changed = false;
                for target in needed {
                    // matched by target, not by index: the resolve can take a
                    // second, and the user can add or remove items meanwhile
                    let Some(hi_res) = icon_map.get(target) else { continue };
                    if hi_res == "none" {
                        continue;
                    }
                    for it in items.iter_mut() {
                        if it.target == *target && it.icon != *hi_res {
                            it.icon = hi_res.clone();
                            folder_changed = true;
                            changed = true;
                        }
                    }
                }
                if folder_changed {
                    set_folder_items(r, &items);
                }
            }
        }
    }

    if changed {
        persist(app);
        for (id, _) in &to_upgrade {
            app.emit("floaty-icon-refreshed", id).ok();
        }
        for (folder_id, _) in &folders_to_check {
            app.emit("floaty-folder-changed", folder_id).ok();
        }
        log_line(app, "upgrade_low_res_icons: batch upgrade complete");
    }

    if migrate {
        stamp_pipeline();
    }
}

#[tauri::command]
pub(crate) async fn floaty_resolve_folder_icons(folder_id: String, app: AppHandle) -> Result<(), String> {
    let needed: Vec<String> = {
        let state = app.state::<AppState>();
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        let rec = guard.widgets.get(&folder_id).ok_or("folder not found")?;
        let items = folder_items(rec);
        items
            .into_iter()
            // directories included: skipping them left every subfolder inside a
            // folder widget blank (only the startup pass ever filled those in)
            .filter(|it| icons::is_missing(&it.icon) && !it.target.trim().is_empty())
            .map(|it| it.target)
            .collect()
    };

    if needed.is_empty() {
        return Ok(());
    }

    let icon_map =
        tauri::async_runtime::spawn_blocking(move || resolve_icons_batch(&needed, false))
            .await
            .map_err(|e| e.to_string())?;

    let changed = {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        if let Some(rec) = guard.widgets.get_mut(&folder_id) {
            let mut items = folder_items(rec);
            let mut modified = false;
            for it in &mut items {
                if let Some(icon) = icon_map.get(&it.target) {
                    if icon != "none" && it.icon != *icon {
                        it.icon = icon.clone();
                        modified = true;
                    }
                }
            }
            if modified {
                set_folder_items(rec, &items);
                true
            } else {
                false
            }
        } else {
            false
        }
    };

    if changed {
        persist(&app);
        app.emit("floaty-folder-changed", &folder_id).ok();
    }

    Ok(())
}
