//! System-monitor sampling for the `sysmon` widget: CPU, 3D GPU and RAM.
//!
//! Sampling is deliberately cheap and slow. The widget lives inside the
//! full-desktop transparent overlay, so every frame it paints is a full-screen
//! composite: this module samples at 1 Hz by default, keeps its thread alive
//! only while a sysmon widget is mounted, and the UI draws exactly one frame
//! per sample (no animation loop at all).
//!
//! GPU usage is the same number Task Manager reports for the 3D engine: the
//! `\GPU Engine(*)\Utilization Percentage` instances whose name carries
//! `engtype_3D`, summed across processes — the counter the PowerShell
//! `gpu3dsum.ps1` helper used while profiling this app, so the widget agrees
//! with the numbers we quote for floaty's own cost.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use tauri::{AppHandle, Emitter};

/// Sample period bounds (ms).
const MIN_INTERVAL_MS: u32 = 250;
const MAX_INTERVAL_MS: u32 = 10_000;
const DEFAULT_INTERVAL_MS: u32 = 1000;
/// History depth the widget keeps for its graph (samples, not seconds).
pub const HISTORY: usize = 60;

static CLIENTS: AtomicU32 = AtomicU32::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);
static INTERVAL_MS: AtomicU32 = AtomicU32::new(DEFAULT_INTERVAL_MS);

#[derive(Clone, PartialEq, serde::Serialize)]
pub struct SysmonSample {
    /// 0..100 whole-machine CPU busy time
    cpu: f32,
    /// 0..100 summed 3D-engine GPU utilisation
    gpu: f32,
    /// 0..100 physical memory in use
    ram: f32,
    mem_used_mb: u32,
    mem_total_mb: u32,
}

/// Ref-counted: the first sysmon widget turns sampling on, the last turns it off.
pub fn start(app: &AppHandle, interval_ms: Option<u32>) {
    if let Some(ms) = interval_ms {
        set_interval(ms);
    }
    CLIENTS.fetch_add(1, Ordering::SeqCst);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return; // a sampler is already up
    }
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = sample_loop(&app) {
            crate::log_line(&app, &format!("[sysmon] sampling stopped: {e}"));
        }
        RUNNING.store(false, Ordering::SeqCst);
    });
}

/// Release one widget's claim; sampling stops once nobody is watching.
pub fn stop() {
    let prev = CLIENTS.fetch_sub(1, Ordering::SeqCst);
    if prev <= 1 {
        CLIENTS.store(0, Ordering::SeqCst);
    }
}

/// Bring the sampler back if its thread died (a sleep can kill the PDH query)
/// without touching the ref-count: only revives when a widget is still watching.
pub fn ensure_running(app: &AppHandle, interval_ms: Option<u32>) {
    if RUNNING.load(Ordering::SeqCst) || CLIENTS.load(Ordering::SeqCst) == 0 {
        return;
    }
    start(app, interval_ms);
}

pub fn set_interval(ms: u32) {
    INTERVAL_MS.store(ms.clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS), Ordering::Relaxed);
}

pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

// ------------------------------------------------------------- pure helpers ---

/// Busy percentage between two (idle, total) CPU-time pairs.
fn cpu_percent(prev: (u64, u64), now: (u64, u64)) -> f32 {
    let idle = now.0.saturating_sub(prev.0);
    let total = now.1.saturating_sub(prev.1);
    if total == 0 {
        return 0.0;
    }
    let busy = total.saturating_sub(idle);
    ((busy as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as f32
}

/// True for the 3D engine rows of `\GPU Engine(*)`; other engine types
/// (video decode, copy, compute) are not what a GPU graph should show.
fn is_3d_engine(instance: &str) -> bool {
    instance.to_ascii_lowercase().contains("engtype_3d")
}

// ------------------------------------------------------------------- sampler ---

#[cfg(windows)]
fn sample_loop(app: &AppHandle) -> Result<(), String> {
    let mut cpu = CpuMeter::new();
    let mut gpu = GpuMeter::new()?;
    crate::log_line(
        app,
        &format!(
            "[sysmon] sampling every {}ms (cpu=GetSystemTimes, gpu=PDH 3D engine, ram=GlobalMemoryStatusEx)",
            INTERVAL_MS.load(Ordering::Relaxed)
        ),
    );

    let mut logged = false;
    while CLIENTS.load(Ordering::SeqCst) > 0 {
        // sleep in slices so removing the widget stops us promptly
        let period = INTERVAL_MS
            .load(Ordering::Relaxed)
            .clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS);
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(period as u64);
        while std::time::Instant::now() < deadline {
            if CLIENTS.load(Ordering::SeqCst) == 0 {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        let (ram, used_mb, total_mb) = mem_sample();
        let sample = SysmonSample {
            cpu: cpu.sample(),
            gpu: gpu.sample(),
            ram,
            mem_used_mb: used_mb,
            mem_total_mb: total_mb,
        };
        if !logged {
            logged = true;
            crate::log_line(
                app,
                &format!(
                    "[sysmon] first sample cpu={:.1}% gpu={:.1}% ram={:.1}% ({}/{} MB)",
                    sample.cpu, sample.gpu, sample.ram, sample.mem_used_mb, sample.mem_total_mb
                ),
            );
        }
        let _ = app.emit("floaty-sysmon", &sample);
    }
    Ok(())
}

#[cfg(not(windows))]
fn sample_loop(_app: &AppHandle) -> Result<(), String> {
    Err("system monitoring is windows-only".into())
}

// ------------------------------------------------------------------- meters ---

#[cfg(windows)]
struct CpuMeter {
    /// (idle, total) as 100ns units
    prev: (u64, u64),
}

#[cfg(windows)]
impl CpuMeter {
    fn new() -> Self {
        CpuMeter {
            prev: read_cpu_times(),
        }
    }

    fn sample(&mut self) -> f32 {
        let now = read_cpu_times();
        let pct = cpu_percent(self.prev, now);
        self.prev = now;
        pct
    }
}

#[cfg(windows)]
fn read_cpu_times() -> (u64, u64) {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetSystemTimes;

    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // kernel time already includes idle, so total = kernel + user
    let ok = unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.is_ok();
    if !ok {
        return (0, 0);
    }
    (
        filetime_u64(idle),
        filetime_u64(kernel) + filetime_u64(user),
    )
}

#[cfg(windows)]
fn filetime_u64(ft: windows::Win32::Foundation::FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

/// PDH query over `\GPU Engine(*)`: one handle per sampler, re-read per tick.
#[cfg(windows)]
struct GpuMeter {
    query: windows::Win32::System::Performance::PDH_HQUERY,
    counter: windows::Win32::System::Performance::PDH_HCOUNTER,
    /// reused instance buffer (PDH writes PDH_FMT_COUNTERVALUE_ITEM_W array)
    buf: Vec<u8>,
}

#[cfg(windows)]
impl GpuMeter {
    fn new() -> Result<Self, String> {
        use windows::core::w;
        use windows::Win32::System::Performance::{
            PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhOpenQueryW, PDH_HCOUNTER,
            PDH_HQUERY,
        };

        unsafe {
            let mut query = PDH_HQUERY::default();
            let st = PdhOpenQueryW(None, 0, &mut query);
            if st != 0 {
                return Err(format!("PdhOpenQuery: {st:#x}"));
            }
            let mut counter = PDH_HCOUNTER::default();
            let st = PdhAddEnglishCounterW(
                query,
                w!("\\GPU Engine(*)\\Utilization Percentage"),
                0,
                &mut counter,
            );
            if st != 0 {
                let _ = PdhCloseQuery(query);
                return Err(format!("PdhAddEnglishCounter(GPU Engine): {st:#x}"));
            }
            // first collect only establishes the baseline; values land on the next one
            let st = PdhCollectQueryData(query);
            if st != 0 {
                let _ = PdhCloseQuery(query);
                return Err(format!("PdhCollectQueryData: {st:#x}"));
            }
            Ok(GpuMeter {
                query,
                counter,
                buf: Vec::new(),
            })
        }
    }

    /// Summed 3D-engine utilisation, or -1.0 when this tick has no usable data.
    fn sample(&mut self) -> f32 {
        use windows::Win32::System::Performance::{
            PdhCollectQueryData, PdhGetFormattedCounterArrayW, PDH_CSTATUS_NEW_DATA,
            PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA,
        };

        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return -1.0;
            }
            let mut size = 0u32;
            let mut count = 0u32;
            let mut st = PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                None,
            );
            if st == PDH_MORE_DATA {
                if self.buf.len() < size as usize {
                    self.buf.resize(size as usize, 0);
                }
                st = PdhGetFormattedCounterArrayW(
                    self.counter,
                    PDH_FMT_DOUBLE,
                    &mut size,
                    &mut count,
                    Some(self.buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W),
                );
            } else if st == 0 && count == 0 {
                // no GPU instances this tick
                return 0.0;
            }
            if st != 0 || count == 0 {
                return -1.0;
            }

            let items = std::slice::from_raw_parts(
                self.buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W,
                count as usize,
            );
            let mut sum = 0.0f64;
            let mut seen = false;
            for it in items {
                let status = it.FmtValue.CStatus;
                if status != PDH_CSTATUS_VALID_DATA && status != PDH_CSTATUS_NEW_DATA {
                    continue;
                }
                let name = pwstr_to_string(it.szName);
                if !is_3d_engine(&name) {
                    continue;
                }
                seen = true;
                sum += it.FmtValue.Anonymous.doubleValue;
            }
            if !seen {
                return 0.0;
            }
            sum.clamp(0.0, 100.0) as f32
        }
    }
}

#[cfg(windows)]
impl Drop for GpuMeter {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::System::Performance::PdhCloseQuery(self.query);
        }
    }
}

/// Bounded UTF-16 read of a PDH instance name.
#[cfg(windows)]
fn pwstr_to_string(p: windows::core::PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe {
        let mut len = 0usize;
        while len < 512 && *p.0.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p.0, len))
    }
}

#[cfg(windows)]
fn mem_sample() -> (f32, u32, u32) {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut st = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    if unsafe { GlobalMemoryStatusEx(&mut st) }.is_err() {
        return (0.0, 0, 0);
    }
    let mb = |bytes: u64| (bytes / (1024 * 1024)) as u32;
    let total = mb(st.ullTotalPhys);
    let used = mb(st.ullTotalPhys.saturating_sub(st.ullAvailPhys));
    (st.dwMemoryLoad as f32, used, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_percent_matches_hand_maths() {
        // half the delta was busy
        assert!((cpu_percent((0, 0), (500, 1000)) - 50.0).abs() < 1e-3);
        // fully idle
        assert!(cpu_percent((0, 0), (1000, 1000)).abs() < 1e-3);
        // no elapsed time must not divide by zero
        assert_eq!(cpu_percent((10, 20), (10, 20)), 0.0);
        // counters that jump backwards (impossible, but must not panic)
        assert_eq!(cpu_percent((100, 200), (1, 1)), 0.0);
    }

    #[test]
    fn only_3d_engine_instances_count() {
        assert!(is_3d_engine("pid_1234_luid_0x00000000_0x0000C3B1_phys_0_eng_0_engtype_3D"));
        assert!(is_3d_engine("pid_9_engtype_3d"));
        assert!(!is_3d_engine("pid_1234_luid_0x0_0x0_phys_0_eng_0_engtype_VideoDecode"));
        assert!(!is_3d_engine("pid_1234_luid_0x0_0x0_phys_0_eng_1_engtype_Copy"));
        assert!(!is_3d_engine(""));
    }

    #[test]
    fn interval_is_clamped() {
        set_interval(1);
        assert_eq!(INTERVAL_MS.load(Ordering::Relaxed), MIN_INTERVAL_MS);
        set_interval(999_999);
        assert_eq!(INTERVAL_MS.load(Ordering::Relaxed), MAX_INTERVAL_MS);
        set_interval(DEFAULT_INTERVAL_MS);
        assert_eq!(INTERVAL_MS.load(Ordering::Relaxed), DEFAULT_INTERVAL_MS);
    }

    #[test]
    fn refcount_never_underflows() {
        // callers may release more often than they claim
        stop();
        stop();
        assert_eq!(CLIENTS.load(Ordering::SeqCst), 0);
        // nothing has claimed a sampler in this test binary
        assert!(!is_running());
    }
}
