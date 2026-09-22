//! System-audio capture for the audio-visualizer widget.
//!
//! Windows has no microphone-free "what are the speakers playing" API other than
//! WASAPI loopback on the default render endpoint, so that is what this does:
//! open the render device in loopback mode, pull packets, keep a sliding window
//! of mono samples, FFT it and publish log-spaced band magnitudes to the UI.
//!
//! Cost matters here: the visualizer lives inside the full-desktop overlay, so
//! every redraw it triggers is a full-screen composite. The capture thread is
//! therefore only alive while a visualizer widget exists, the emit rate is
//! throttled to the configured fps, and it drops to a trickle once the machine
//! has been silent for a moment so the widget can stop drawing entirely.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use tauri::{AppHandle, Emitter};

/// Published spectrum size (log-spaced bands).
pub const BANDS: usize = 28;
/// FFT window; at 48kHz that is ~21ms / 47 windows per second.
const FFT_N: usize = 1024;
/// Emit rate while audio is playing; silence emits at SILENT_FPS instead.
const SILENT_FPS: u32 = 6;
/// Band edges: 40Hz .. 15kHz, log spaced.
const LOW_HZ: f32 = 40.0;
const HIGH_HZ: f32 = 15_000.0;
/// Below this RMS the endpoint counts as silent.
const SILENCE_RMS: f32 = 2.0e-4;

static CLIENTS: AtomicU32 = AtomicU32::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);
static FPS: AtomicU32 = AtomicU32::new(30);

#[derive(Clone, serde::Serialize)]
pub struct AudioLevels {
    /// 0..1 band magnitudes, low frequency first
    pub(crate) bands: Vec<f32>,
    /// 0..1 broadband level (for the widget's glow / mirror effects)
    pub(crate) level: f32,
    /// true when nothing is playing (widget may idle instead of redrawing)
    pub(crate) silent: bool,
}

/// Ref-counted: the first visualizer turns capture on, the last one turns it off.
pub fn start(app: &AppHandle, fps: Option<u32>) {
    if let Some(f) = fps {
        FPS.store(f.clamp(5, 60), Ordering::Relaxed);
    }
    CLIENTS.fetch_add(1, Ordering::SeqCst);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return; // a capture thread is already up
    }
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = capture_loop(&app) {
            crate::log_line(&app, &format!("[audio] capture stopped: {e}"));
        }
        RUNNING.store(false, Ordering::SeqCst);
    });
}

/// Release one client's claim; capture stops once nobody is listening.
pub fn stop() {
    let prev = CLIENTS.fetch_sub(1, Ordering::SeqCst);
    if prev <= 1 {
        // never underflow if stop() is called more often than start()
        CLIENTS.store(0, Ordering::SeqCst);
    }
}

/// Bring the capture back if its thread died in the sleep, without touching the
/// ref-count: only revives when a visualizer is still watching.
pub fn ensure_running(app: &AppHandle, fps: Option<u32>) {
    if RUNNING.load(Ordering::SeqCst) || CLIENTS.load(Ordering::SeqCst) == 0 {
        return;
    }
    start(app, fps);
}

pub fn set_fps(fps: u32) {
    FPS.store(fps.clamp(5, 60), Ordering::Relaxed);
}

/// True while the capture thread holds an audio client.
pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------- capture ---

#[cfg(windows)]
unsafe fn init_wasapi_capture(
) -> Result<(windows::Win32::Media::Audio::IAudioClient, windows::Win32::Media::Audio::IAudioCaptureClient, AudioFormat), String> {
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .map_err(|e| format!("MMDeviceEnumerator: {e}"))?;
    let device = enumerator
        .GetDefaultAudioEndpoint(eRender, eConsole)
        .map_err(|e| format!("no default render endpoint: {e}"))?;
    let client: IAudioClient = device
        .Activate(CLSCTX_ALL, None)
        .map_err(|e| format!("activate IAudioClient: {e}"))?;
    let fmt_ptr = client
        .GetMixFormat()
        .map_err(|e| format!("GetMixFormat: {e}"))?;
    if fmt_ptr.is_null() {
        return Err("GetMixFormat returned null".into());
    }
    let fmt = AudioFormat::parse(&*fmt_ptr);
    let res = client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_LOOPBACK,
        0,
        0,
        fmt_ptr,
        None,
    );
    CoTaskMemFree(Some(fmt_ptr as *const _));
    let fmt = fmt.ok_or_else(|| "unsupported mix format".to_string())?;
    res.map_err(|e| format!("Initialize(loopback): {e}"))?;
    let capture: IAudioCaptureClient = client
        .GetService()
        .map_err(|e| format!("GetService(IAudioCaptureClient): {e}"))?;
    client.Start().map_err(|e| format!("Start: {e}"))?;
    Ok((client, capture, fmt))
}

#[cfg(windows)]
unsafe fn drain_audio_packets(
    capture: &windows::Win32::Media::Audio::IAudioCaptureClient,
    fmt: &AudioFormat,
    mut packet_frames: u32,
    scratch: &mut Vec<u8>,
    window: &mut Vec<f32>,
) -> bool {
    use windows::Win32::Media::Audio::AUDCLNT_BUFFERFLAGS_SILENT;
    let mut got_audio = false;
    while packet_frames > 0 {
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut frames: u32 = 0;
        let mut flags: u32 = 0;
        let ok = capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None).is_ok();
        if !ok || frames == 0 {
            break;
        }
        let silent_packet = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
        let bytes = fmt.bytes_for(frames as usize);
        scratch.resize(bytes, 0);
        if !silent_packet && !data.is_null() {
            std::ptr::copy_nonoverlapping(data, scratch.as_mut_ptr(), bytes);
            got_audio = true;
        }
        fmt.append_mono(scratch, frames as usize, window);
        let _ = capture.ReleaseBuffer(frames);
        let next = capture.GetNextPacketSize().unwrap_or(0);
        if next == 0 {
            break;
        }
        packet_frames = next;
    }
    got_audio
}

#[cfg(windows)]
fn process_fft_windows(window: &mut Vec<f32>, fft: &mut Fft, bands: &mut [f32]) -> (f32, bool) {
    let mut rms = 0.0f32;
    let mut analyzed = false;
    while window.len() >= FFT_N {
        let chunk: Vec<f32> = window.drain(..FFT_N).collect();
        let sum: f32 = chunk.iter().map(|s| s * s).sum();
        rms = (sum / FFT_N as f32).sqrt();
        fft.analyze(&chunk, bands);
        analyzed = true;
    }
    (rms, analyzed)
}

#[cfg(windows)]
fn capture_loop(app: &AppHandle) -> Result<(), String> {
    let (client, capture, fmt) = unsafe { init_wasapi_capture()? };

    crate::log_line(
        app,
        &format!(
            "[audio] loopback capture started: {}Hz {}ch {}bit float={}",
            fmt.rate, fmt.channels, fmt.bits, fmt.is_float
        ),
    );

    let mut fft = Fft::new(FFT_N);
    fft.ensure_edges(fmt.rate); // band edges depend on the device sample rate
    let mut window: Vec<f32> = Vec::with_capacity(FFT_N * 2);
    let mut bands = vec![0.0f32; BANDS];
    let mut level = 0.0f32;
    let mut silent = true;
    let mut silent_since = std::time::Instant::now();
    let mut last_emit = std::time::Instant::now();
    let mut scratch: Vec<u8> = Vec::new();

    while CLIENTS.load(Ordering::SeqCst) > 0 {
        let packet_frames: u32 = match unsafe { capture.GetNextPacketSize() } {
            Ok(n) => n,
            Err(e) => {
                crate::log_line(app, &format!("[audio] GetNextPacketSize failed: {e}"));
                silent = true;
                std::thread::sleep(std::time::Duration::from_millis(200));
                0
            }
        };

        let drained = unsafe { drain_audio_packets(&capture, &fmt, packet_frames, &mut scratch, &mut window) };
        let (rms, analyzed) = process_fft_windows(&mut window, &mut fft, &mut bands);
        let got_audio = drained || analyzed;

        // fast attack / slow release keeps the bars lively but not jittery
        level = if rms > level { rms } else { level * 0.82 + rms * 0.18 };
        let quiet = !got_audio || rms < SILENCE_RMS;
        if quiet {
            if !silent && silent_since.elapsed().as_millis() > 400 {
                silent = true;
            }
            if silent {
                for b in bands.iter_mut() {
                    *b *= 0.92;
                }
                level *= 0.92;
            }
        } else {
            // audio came back: start counting how long we have been audible again
            silent = false;
            silent_since = std::time::Instant::now();
        }

        let target_fps = if silent { SILENT_FPS } else { FPS.load(Ordering::Relaxed).max(5) };
        if last_emit.elapsed().as_millis() as u32 * target_fps >= 1000 {
            last_emit = std::time::Instant::now();
            let _ = app.emit(
                "floaty-audio-levels",
                AudioLevels {
                    bands: bands.clone(),
                    level: level.min(1.0),
                    silent,
                },
            );
        }

        if !got_audio {
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
    }

    unsafe {
        let _ = client.Stop();
    }
    crate::log_line(app, "[audio] loopback capture stopped");
    Ok(())
}

#[cfg(not(windows))]
fn capture_loop(_app: &AppHandle) -> Result<(), String> {
    Err("audio capture is windows-only".into())
}

// ------------------------------------------------------------------ format ---

#[cfg(windows)]
#[derive(Clone, Copy)]
struct AudioFormat {
    pub(crate) channels: usize,
    pub(crate) rate: f32,
    pub(crate) bits: u16,
    pub(crate) is_float: bool,
}

#[cfg(windows)]
impl AudioFormat {
    fn parse(f: &windows::Win32::Media::Audio::WAVEFORMATEX) -> Option<Self> {
        const WAVE_FORMAT_PCM: u16 = 1;
        const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
        const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
        // KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        const SUBTYPE_FLOAT: [u8; 16] = [
            0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38,
            0x9B, 0x71,
        ];
        let is_float = match f.wFormatTag {
            WAVE_FORMAT_IEEE_FLOAT => true,
            WAVE_FORMAT_EXTENSIBLE => {
                // the subformat GUID sits right after the 22-byte WAVEFORMATEX
                let ext = f as *const _ as *const u8;
                let sub = unsafe { std::slice::from_raw_parts(ext.add(24), 16) };
                sub == SUBTYPE_FLOAT
            }
            WAVE_FORMAT_PCM => false,
            _ => return None,
        };
        if f.nChannels == 0 || f.nSamplesPerSec == 0 {
            return None;
        }
        if !is_float && f.wBitsPerSample != 16 && f.wBitsPerSample != 32 {
            return None;
        }
        Some(AudioFormat {
            channels: f.nChannels as usize,
            rate: f.nSamplesPerSec as f32,
            bits: f.wBitsPerSample,
            is_float,
        })
    }

    fn bytes_for(&self, frames: usize) -> usize {
        frames * self.channels * (self.bits as usize / 8)
    }

    #[inline]
    fn decode_sample(bytes: &[u8], off: usize, is_float: bool, bits: u16) -> f32 {
        if is_float && bits == 32 {
            f32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
        } else if bits == 16 {
            i16::from_le_bytes([bytes[off], bytes[off + 1]]) as f32 / 32768.0
        } else {
            i32::from_le_bytes([
                bytes[off],
                bytes[off + 1],
                bytes[off + 2],
                bytes[off + 3],
            ]) as f32
                / 2147483648.0
        }
    }

    /// Downmixes a packet to mono and appends it to the analysis window.
    fn append_mono(&self, bytes: &[u8], frames: usize, out: &mut Vec<f32>) {
        let bytes_per_sample = self.bits as usize / 8;
        let stride = self.channels * bytes_per_sample;
        for i in 0..frames {
            let base = i * stride;
            if base + stride > bytes.len() {
                break;
            }
            let mut acc = 0.0f32;
            for c in 0..self.channels {
                let off = base + c * bytes_per_sample;
                acc += Self::decode_sample(bytes, off, self.is_float, self.bits);
            }
            out.push(acc / self.channels as f32);
        }
        // never let the backlog grow without bound
        let cap = FFT_N * 4;
        if out.len() > cap {
            let drop = out.len() - cap;
            out.drain(..drop);
        }
    }
}

// --------------------------------------------------------------------- fft ---

/// Iterative radix-2 FFT with a Hann window, sized for this one job.
struct Fft {
    pub(crate) n: usize,
    pub(crate) cos: Vec<f32>,
    pub(crate) sin: Vec<f32>,
    pub(crate) window: Vec<f32>,
    pub(crate) re: Vec<f32>,
    pub(crate) im: Vec<f32>,
    pub(crate) rev: Vec<usize>,
    /// band edges as FFT bin ranges, precomputed
    pub(crate) edges: Vec<(usize, usize)>,
    pub(crate) mag: Vec<f32>,
}

impl Fft {
    fn new(n: usize) -> Self {
        let mut cos = vec![0.0f32; n / 2];
        let mut sin = vec![0.0f32; n / 2];
        for i in 0..n / 2 {
            let a = -2.0 * std::f32::consts::PI * i as f32 / n as f32;
            cos[i] = a.cos();
            sin[i] = a.sin();
        }
        let window = (0..n)
            .map(|i| {
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (n as f32 - 1.0)).cos()
            })
            .collect();
        let mut rev = vec![0usize; n];
        let bits = n.trailing_zeros();
        for (i, slot) in rev.iter_mut().enumerate() {
            *slot = ((i as u32).reverse_bits() >> (32 - bits)) as usize;
        }
        Fft {
            n,
            cos,
            sin,
            window,
            re: vec![0.0; n],
            im: vec![0.0; n],
            rev,
            edges: Vec::new(),
            mag: vec![0.0; n / 2],
        }
    }

    /// Band edges need the sample rate, so they are built lazily on first use.
    fn ensure_edges(&mut self, rate: f32) {
        if !self.edges.is_empty() {
            return;
        }
        let bin_hz = rate / self.n as f32;
        let ratio = (HIGH_HZ / LOW_HZ).powf(1.0 / BANDS as f32);
        let mut edges = Vec::with_capacity(BANDS);
        for b in 0..BANDS {
            let lo = LOW_HZ * ratio.powi(b as i32);
            let hi = LOW_HZ * ratio.powi(b as i32 + 1);
            let lo_bin = ((lo / bin_hz).floor() as usize).clamp(1, self.n / 2 - 1);
            let hi_bin = ((hi / bin_hz).ceil() as usize).clamp(lo_bin + 1, self.n / 2);
            edges.push((lo_bin, hi_bin));
        }
        self.edges = edges;
    }

    /// Fills `out` (BANDS entries) with 0..1 magnitudes for this window.
    fn analyze(&mut self, samples: &[f32], out: &mut [f32]) {
        let n = self.n;
        for i in 0..n {
            let s = samples.get(i).copied().unwrap_or(0.0) * self.window[i];
            self.re[self.rev[i]] = s;
            self.im[self.rev[i]] = 0.0;
        }
        // butterflies
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            let mut i = 0;
            while i < n {
                let mut j = i;
                let mut k = 0;
                while j < i + half {
                    let tre = self.re[j + half] * self.cos[k] - self.im[j + half] * self.sin[k];
                    let tim = self.re[j + half] * self.sin[k] + self.im[j + half] * self.cos[k];
                    self.re[j + half] = self.re[j] - tre;
                    self.im[j + half] = self.im[j] - tim;
                    self.re[j] += tre;
                    self.im[j] += tim;
                    j += 1;
                    k += step;
                }
                i += len;
            }
            len *= 2;
        }
        for i in 0..n / 2 {
            self.mag[i] = (self.re[i] * self.re[i] + self.im[i] * self.im[i]).sqrt() / (n as f32 * 0.25);
        }
        // dB -> 0..1 with a -70dB floor, then smooth attack/release per band
        for (b, (lo, hi)) in self.edges.clone().into_iter().enumerate() {
            let mut sum = 0.0;
            let count = (hi - lo).max(1) as f32;
            for i in lo..hi {
                sum += self.mag[i];
            }
            let amp = sum / count;
            let db = 20.0 * (amp + 1e-9).log10();
            let norm = ((db + 70.0) / 70.0).clamp(0.0, 1.0);
            let prev = out[b];
            out[b] = if norm > prev { norm } else { prev * 0.80 + norm * 0.20 };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, rate: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate).sin() * 0.5)
            .collect()
    }

    #[test]
    fn fft_finds_the_tone_bin() {
        let rate = 48_000.0;
        let mut fft = Fft::new(FFT_N);
        fft.ensure_edges(rate);
        let mut out = vec![0.0f32; BANDS];
        let samples = tone(1000.0, rate, FFT_N);
        fft.analyze(&samples, &mut out);
        // the loudest raw bin should sit at 1000Hz +/- one bin
        let peak = fft
            .mag
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();
        let hz = peak as f32 * rate / FFT_N as f32;
        assert!((hz - 1000.0).abs() < 60.0, "peak bin was {hz}Hz");
    }

    #[test]
    fn bands_are_ordered_and_react_to_a_tone() {
        let rate = 48_000.0;
        let mut fft = Fft::new(FFT_N);
        fft.ensure_edges(rate);
        assert_eq!(fft.edges.len(), BANDS);
        for w in fft.edges.windows(2) {
            assert!(w[0].0 <= w[1].0 && w[0].1 <= w[1].1, "band edges must ascend");
            assert!(w[1].0 >= w[0].1 - 1, "bands must not invert");
        }
        let mut quiet = vec![0.0f32; BANDS];
        fft.analyze(&vec![0.0f32; FFT_N], &mut quiet);
        let mut loud = vec![0.0f32; BANDS];
        fft.analyze(&tone(1000.0, rate, FFT_N), &mut loud);
        let q: f32 = quiet.iter().sum();
        let l: f32 = loud.iter().sum();
        assert!(l > q, "a tone must raise total band energy ({l} vs {q})");
        assert!(loud.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn stereo_float_is_downmixed_to_mono() {
        #[cfg(windows)]
        {
            let fmt = AudioFormat {
                channels: 2,
                rate: 48_000.0,
                bits: 32,
                is_float: true,
            };
            // two frames: (0.5, 0.5) and (-1.0, 0.0)
            let mut bytes = Vec::new();
            for v in [0.5f32, 0.5, -1.0, 0.0] {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            let mut out = Vec::new();
            fmt.append_mono(&bytes, 2, &mut out);
            assert_eq!(out.len(), 2);
            assert!((out[0] - 0.5).abs() < 1e-6);
            assert!((out[1] + 0.5).abs() < 1e-6);
        }
    }
}
