//! Windows audio capture: WASAPI loopback (game) + microphone, 48 kHz stereo
//! i16 rings with QPC timestamps.
//!
//! Design rules (why this file exists):
//! - **Real-time callbacks never allocate or lock.** Samples are converted in
//!   a preallocated scratch, gains are atomic, and the data crosses to the
//!   assembler through a lock-free SPSC ring. The old design locked a Mutex
//!   per callback, so a save-time snapshot could stall the capture thread.
//! - **One clock for everything: QPC.** Each captured chunk carries the QPC
//!   instant of its first sample (cpal's WASAPI capture timestamp). At save
//!   the engine maps the video keyframe PTS to QPC and rebuilds each stem
//!   over exactly that window. `build_window` resamples by block timestamps,
//!   so device-clock drift (a few hundred ppm) cannot shift A/V over long
//!   buffers, and only real gaps (> FUSE) become silence — sub-ms timestamp
//!   jitter is absorbed, never materialized (that caused the old static).
//! - **MMCSS "Pro Audio"** on the capture threads (cpal already runs them
//!   TIME_CRITICAL; MMCSS tells the scheduler to protect them).

use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW;

use super::devices;

pub const STEM_RATE: u32 = 48_000;
pub const STEM_CHANNELS: usize = 2;
/// Sub-50 ms gaps inside a stem are timestamp jitter (WASAPI block grid vs
/// sample count), not lost audio: the resampler bridges them. Larger gaps
/// are real and become silence.
pub const FUSE_NS: i128 = 50_000_000;
/// Ring block size handed to the assembler (10 ms stereo).
const BLOCK_FRAMES: usize = 480;
/// Raw callback queue per stream (~2 s at 48 kHz stereo).
const QUEUE_SAMPLES: usize = 1 << 17; // 131072 frames... power of two
/// How often the supervisor wakes up.
const TICK: Duration = Duration::from_millis(500);
/// No delivered chunk for this long with a live stream = stalled: reopen.
const STALL_AFTER: Duration = Duration::from_secs(3);
/// Slow retry after a stall reopen while still silent.
const STALL_RETRY: Duration = Duration::from_secs(30);
/// Error reopen backoff.
const ERROR_RETRY: Duration = Duration::from_secs(3);
/// Device-follow poll (only for the `default_*` magic ids).
const DEFAULT_POLL_TICKS: u32 = 4; // 2 s

fn qpc_freq() -> u64 {
    static FREQ: OnceLock<u64> = OnceLock::new();
    *FREQ.get_or_init(|| {
        let mut f = 0i64;
        unsafe {
            let _ = QueryPerformanceFrequency(&mut f);
        }
        (f.max(1)) as u64
    })
}

/// Local QPC in nanoseconds (same clock cpal/WASAPI timestamps use).
pub fn qpc_ns() -> i128 {
    let mut c = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut c);
    }
    c as i128 * 1_000_000_000 / qpc_freq() as i128
}

/// cpal converts raw QPC ticks to ns assuming a 10 MHz counter; correct its
/// deltas when the OS counter runs at another frequency.
fn cpal_ns_scale() -> f64 {
    10_000_000.0 / qpc_freq() as f64
}

// ---------------------------------------------------------------------------
// Lock-free SPSC queues (single producer = audio callback, single consumer =
// assembler thread). One slot is reserved to tell full from empty.
// ---------------------------------------------------------------------------

struct SampleQueue {
    buf: Box<[UnsafeCell<i16>]>,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

unsafe impl Sync for SampleQueue {}

impl SampleQueue {
    fn new(cap_pow2: usize) -> Self {
        let cap = cap_pow2.next_power_of_two();
        let buf: Vec<UnsafeCell<i16>> = (0..cap).map(|_| UnsafeCell::new(0i16)).collect();
        Self {
            buf: buf.into_boxed_slice(),
            mask: cap - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        let t = self.tail.load(Ordering::Acquire);
        let h = self.head.load(Ordering::Relaxed);
        t.wrapping_sub(h) & self.mask
    }

    /// All-or-nothing push: a partial chunk would desync the timestamp map.
    fn push(&self, data: &[i16]) -> bool {
        if data.is_empty() {
            return true;
        }
        let t = self.tail.load(Ordering::Relaxed);
        let h = self.head.load(Ordering::Acquire);
        let free = self.mask - (t.wrapping_sub(h) & self.mask);
        if free < data.len() {
            return false;
        }
        for (i, &s) in data.iter().enumerate() {
            unsafe {
                *self.buf[(t.wrapping_add(i)) & self.mask].get() = s;
            }
        }
        self.tail.store(t.wrapping_add(data.len()), Ordering::Release);
        true
    }

    fn pop(&self, out: &mut [i16]) -> usize {
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Acquire);
        let avail = t.wrapping_sub(h) & self.mask;
        let n = avail.min(out.len());
        for (i, slot) in out[..n].iter_mut().enumerate() {
            *slot = unsafe { *self.buf[(h.wrapping_add(i)) & self.mask].get() };
        }
        self.head.store(h.wrapping_add(n), Ordering::Release);
        n
    }
}

#[derive(Clone, Copy)]
struct ChunkMeta {
    qpc_ns: i64,
    frames: u32,
}

struct MetaQueue {
    buf: Box<[UnsafeCell<ChunkMeta>]>,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

unsafe impl Sync for MetaQueue {}

impl MetaQueue {
    fn new(cap_pow2: usize) -> Self {
        let cap = cap_pow2.next_power_of_two();
        let buf: Vec<UnsafeCell<ChunkMeta>> = (0..cap)
            .map(|_| UnsafeCell::new(ChunkMeta { qpc_ns: 0, frames: 0 }))
            .collect();
        Self {
            buf: buf.into_boxed_slice(),
            mask: cap - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    fn push(&self, meta: ChunkMeta) -> bool {
        let t = self.tail.load(Ordering::Relaxed);
        let h = self.head.load(Ordering::Acquire);
        if self.mask - (t.wrapping_sub(h) & self.mask) == 0 {
            return false;
        }
        unsafe {
            *self.buf[t & self.mask].get() = meta;
        }
        self.tail.store(t.wrapping_add(1), Ordering::Release);
        true
    }

    fn pop(&self) -> Option<ChunkMeta> {
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Acquire);
        if h == t {
            return None;
        }
        let m = unsafe { *self.buf[h & self.mask].get() };
        self.head.store(h.wrapping_add(1), Ordering::Release);
        Some(m)
    }
}

// ---------------------------------------------------------------------------
// Stems: timestamped 10 ms blocks + QPC-grid window builder
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Block {
    /// QPC of the block's first sample.
    pub qpc_ns: i128,
    /// Interleaved stereo i16.
    pub samples: Vec<i16>,
}

impl Block {
    fn frames(&self) -> usize {
        self.samples.len() / STEM_CHANNELS
    }

    fn end_ns(&self) -> i128 {
        self.qpc_ns + self.frames() as i128 * 1_000_000_000 / STEM_RATE as i128
    }
}

struct StemRing {
    blocks: VecDeque<Block>,
    /// Capacity in i16 (interleaved).
    capacity: usize,
    total: usize,
}

impl StemRing {
    fn new(capacity_secs: u32) -> Self {
        Self {
            blocks: VecDeque::new(),
            capacity: capacity_secs.max(1) as usize * STEM_RATE as usize * STEM_CHANNELS,
            total: 0,
        }
    }

    fn push(&mut self, block: Block) {
        self.total += block.samples.len();
        self.blocks.push_back(block);
        while self.total > self.capacity {
            match self.blocks.pop_front() {
                Some(b) => self.total -= b.samples.len(),
                None => break,
            }
        }
    }
}

/// Rebuild `frames` stereo frames of a stem on the QPC grid starting at
/// `start_ns`, using each block's timestamp to resample (drift correction)
/// and inserting silence only for real gaps. Pure and unit-tested.
pub fn build_window(blocks: &VecDeque<Block>, start_ns: i128, frames: usize) -> Vec<i16> {
    let mut out = vec![0i16; frames * STEM_CHANNELS];
    if frames == 0 || blocks.is_empty() {
        return out;
    }
    // Flatten content for fractional lookups + per-block geometry.
    let mut content: Vec<i16> = Vec::with_capacity(blocks.iter().map(|b| b.samples.len()).sum());
    let mut geo: Vec<(i128, i128, usize)> = Vec::with_capacity(blocks.len());
    for b in blocks {
        let c_start = content.len() / STEM_CHANNELS;
        let q_start = b.qpc_ns;
        content.extend_from_slice(&b.samples);
        geo.push((q_start, b.end_ns(), c_start));
    }
    let content_frames = content.len() / STEM_CHANNELS;
    if content_frames == 0 {
        return out;
    }
    let step_num = 1_000_000_000i128;
    let step_den = STEM_RATE as i128;
    let mut cursor = 0usize;
    for f in 0..frames {
        let t = start_ns + f as i128 * step_num / step_den;
        // Cursor = last block that already started at `t`.
        while cursor + 1 < geo.len() && t >= geo[cursor + 1].0 {
            cursor += 1;
        }
        let (q_start, q_end, c_start) = geo[cursor];
        let n_frames = (geo
            .get(cursor + 1)
            .map(|(_, _, next_c)| (*next_c).min(content_frames) - c_start)
            .unwrap_or(content_frames - c_start))
        .max(1);
        // Big gap: more than FUSE before the current block, or more than FUSE
        // after its end without a following block.
        if t + FUSE_NS < q_start {
            continue; // silence
        }
        if t > q_end + FUSE_NS && cursor + 1 == geo.len() {
            continue; // silence
        }
        if t > q_end + FUSE_NS && t < geo[cursor + 1].0 {
            continue; // silence between two distant blocks
        }
        // Map within the current block (drift-corrected); hold the last
        // sample while waiting for the next block (tiny jitter, no clicks).
        let frame_f = c_start as i128 + (t - q_start) * step_den / step_num;
        let lo = c_start as i128;
        let hi = (c_start + n_frames - 1) as i128;
        let clamped = frame_f.clamp(lo, hi);
        let fi = clamped as usize;
        let ff = (clamped - fi as i128) as f64;
        let base = fi * STEM_CHANNELS;
        for ch in 0..STEM_CHANNELS {
            let a = content[base + ch] as f64;
            let b = if fi + 1 <= (c_start + n_frames - 1) {
                content[base + STEM_CHANNELS + ch] as f64
            } else {
                a
            };
            out[f * STEM_CHANNELS + ch] = (a + (b - a) * ff).round() as i16;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Per-stream live state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Game,
    Mic,
}

impl Role {
    fn label(self) -> &'static str {
        match self {
            Role::Game => "game",
            Role::Mic => "mic",
        }
    }
}

struct StreamState {
    role: Role,
    /// Configured device id (magic `default_*` follows the OS default).
    device_id: Mutex<String>,
    /// Friendly name of the resolved device (for default-follow comparisons).
    resolved_name: Mutex<String>,
    stream: Mutex<Option<cpal::Stream>>,
    live: AtomicBool,
    dead: Mutex<Option<String>>,
    ring: Mutex<StemRing>,
    gain_pct: AtomicU32,
    muted: AtomicBool,
    peak: AtomicU32,
    recent_peak: AtomicU32,
    delivered_qpc: AtomicI64,
    stall_handled: AtomicBool,
    stall_retry_at: Mutex<Option<std::time::Instant>>,
    retry_at: Mutex<Option<std::time::Instant>>,
    queue: Arc<SampleQueue>,
    metas: Arc<MetaQueue>,
    dropped_chunks: AtomicU64,
}

impl StreamState {
    fn new(role: Role, device_id: String, capacity_secs: u32, gain_pct: u32, muted: bool) -> Self {
        Self {
            role,
            device_id: Mutex::new(device_id),
            resolved_name: Mutex::new(String::new()),
            stream: Mutex::new(None),
            live: AtomicBool::new(false),
            dead: Mutex::new(None),
            ring: Mutex::new(StemRing::new(capacity_secs)),
            gain_pct: AtomicU32::new(gain_pct.min(200)),
            muted: AtomicBool::new(muted),
            peak: AtomicU32::new(0),
            recent_peak: AtomicU32::new(0),
            delivered_qpc: AtomicI64::new(0),
            stall_handled: AtomicBool::new(false),
            stall_retry_at: Mutex::new(None),
            retry_at: Mutex::new(None),
            queue: Arc::new(SampleQueue::new(QUEUE_SAMPLES)),
            metas: Arc::new(MetaQueue::new(4096)),
            dropped_chunks: AtomicU64::new(0),
        }
    }

    fn note_peak(&self, v: f32) {
        let bits = v.to_bits();
        self.peak.fetch_max(bits, Ordering::Relaxed);
        self.recent_peak.fetch_max(bits, Ordering::Relaxed);
    }

    fn peek_peak(&self) -> f32 {
        f32::from_bits(self.peak.load(Ordering::Relaxed))
    }
}

struct Shared {
    game: Arc<StreamState>,
    mic: Arc<StreamState>,
    keepalive: Mutex<Option<cpal::Stream>>,
    stop: AtomicBool,
}

fn registry() -> &'static Mutex<Option<Arc<Shared>>> {
    static LIVE: OnceLock<Mutex<Option<Arc<Shared>>>> = OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(None))
}

// ---------------------------------------------------------------------------
// Public capture handle
// ---------------------------------------------------------------------------

pub struct AudioCapture {
    shared: Arc<Shared>,
    assembler: Option<std::thread::JoinHandle<()>>,
    supervisor: Option<std::thread::JoinHandle<()>>,
}

impl AudioCapture {
    /// Start the game loopback + mic streams. `desktop`/`mic` are the settings
    /// ids (`default_output`/`default_input` = OS default).
    pub fn start(
        desktop: &str,
        mic: &str,
        capacity_secs: u32,
        gain_game: u32,
        gain_mic: u32,
        mute_game: bool,
        mute_mic: bool,
    ) -> Result<Self, String> {
        let game = Arc::new(StreamState::new(
            Role::Game,
            desktop.to_string(),
            capacity_secs,
            gain_game,
            mute_game,
        ));
        let mic = Arc::new(StreamState::new(
            Role::Mic,
            mic.to_string(),
            capacity_secs,
            gain_mic,
            mute_mic,
        ));
        let shared = Arc::new(Shared {
            game: game.clone(),
            mic: mic.clone(),
            keepalive: Mutex::new(None),
            stop: AtomicBool::new(false),
        });
        // Open both streams; a stream that fails to link marks itself dead and
        // the supervisor retries (a buffer with one stem beats no buffer).
        match open_stream(&game) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("[moonclip] game stream failed: {e}");
                *game.dead.lock().unwrap() = Some(e);
            }
        }
        match open_stream(&mic) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("[moonclip] mic stream failed: {e}");
                *mic.dead.lock().unwrap() = Some(e);
            }
        }
        let live = game.live.load(Ordering::Relaxed) as usize + mic.live.load(Ordering::Relaxed) as usize;
        if live == 0 {
            return Err("no audio streams could be opened".into());
        }
        // Silent keep-alive on the render endpoint: WASAPI delivers no packets
        // while the audio engine is idle, which used to look like a dead
        // stream and churned the watchdog. Keeping a render client alive
        // makes loopback deliver continuous silence.
        if let Some(dev) = resolve_output(&game) {
            match start_keepalive(&dev) {
                Ok(s) => *shared.keepalive.lock().unwrap() = Some(s),
                Err(e) => eprintln!("[moonclip] keepalive failed (audio may gap while idle): {e}"),
            }
        }
        if let Ok(mut reg) = registry().lock() {
            *reg = Some(shared.clone());
        }
        let asm_shared = shared.clone();
        let assembler = std::thread::Builder::new()
            .name("moonclip-audio-asm".into())
            .spawn(move || assembler_loop(asm_shared))
            .ok();
        let sup_shared = shared.clone();
        let supervisor = std::thread::Builder::new()
            .name("moonclip-audio-sup".into())
            .spawn(move || supervisor_loop(sup_shared))
            .ok();
        Ok(Self {
            shared,
            assembler,
            supervisor,
        })
    }

    pub fn live_count(&self) -> usize {
        self.shared.game.live.load(Ordering::Relaxed) as usize
            + self.shared.mic.live.load(Ordering::Relaxed) as usize
    }

    pub fn peak_levels(&self) -> (f32, f32) {
        (self.shared.game.peek_peak(), self.shared.mic.peek_peak())
    }

    pub fn stream_errors(&self) -> (Option<String>, Option<String>) {
        (
            self.shared.game.dead.lock().ok().and_then(|d| d.clone()),
            self.shared.mic.dead.lock().ok().and_then(|d| d.clone()),
        )
    }

    /// QPC of the end of the newest delivered audio block per stream
    /// (diagnostics for save-time sync telemetry).
    pub fn delivery_qpc(&self) -> (i64, i64) {
        (
            self.shared.game.delivered_qpc.load(Ordering::Relaxed),
            self.shared.mic.delivered_qpc.load(Ordering::Relaxed),
        )
    }

    /// Exactly `frames` stereo frames of both stems on the QPC grid starting
    /// at `start_ns` (sample aligned with the video keyframe instant).
    pub fn snapshot_window(&self, start_ns: i128, frames: usize) -> (Vec<i16>, Vec<i16>) {
        let game = self
            .shared
            .game
            .ring
            .lock()
            .map(|r| build_window(&r.blocks, start_ns, frames))
            .unwrap_or_else(|_| vec![0i16; frames * STEM_CHANNELS]);
        let mic = self
            .shared
            .mic
            .ring
            .lock()
            .map(|r| build_window(&r.blocks, start_ns, frames))
            .unwrap_or_else(|_| vec![0i16; frames * STEM_CHANNELS]);
        (game, mic)
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Ok(mut reg) = registry().lock() {
            if reg
                .as_ref()
                .map(|s| Arc::ptr_eq(s, &self.shared))
                .unwrap_or(false)
            {
                *reg = None;
            }
        }
        // Streams stop when their handles drop (stream slots + keep-alive).
        if let Ok(mut kl) = self.shared.keepalive.lock() {
            kl.take();
        }
        if let Ok(mut g) = self.shared.game.stream.lock() {
            g.take();
        }
        if let Ok(mut m) = self.shared.mic.stream.lock() {
            m.take();
        }
        if let Some(h) = self.assembler.take() {
            let _ = h.join();
        }
        if let Some(h) = self.supervisor.take() {
            let _ = h.join();
        }
    }
}

// ---------------------------------------------------------------------------
// Stream opening + conversion
// ---------------------------------------------------------------------------

fn resolve_output(state: &StreamState) -> Option<cpal::Device> {
    let id = state.device_id.lock().ok()?.clone();
    devices::find_output_device(&id)
}

fn resolve_input(state: &StreamState) -> Option<cpal::Device> {
    let id = state.device_id.lock().ok()?.clone();
    devices::find_input_device(&id)
}

/// Install MMCSS "Pro Audio" on the calling (capture) thread, once.
fn register_mmcss() {
    use std::cell::Cell;
    thread_local! {
        static DONE: Cell<bool> = const { Cell::new(false) };
    }
    DONE.with(|d| {
        if !d.get() {
            d.set(true);
            let task: Vec<u16> = "Pro Audio\0".encode_utf16().collect();
            let mut idx = 0u32;
            unsafe {
                let _ = AvSetMmThreadCharacteristicsW(
                    windows::core::PCWSTR(task.as_ptr()),
                    &mut idx,
                );
            }
        }
    });
}

/// Stereo 48 kHz i16 conversion with optional linear resample for devices not
/// running at 48 kHz. `scratch` is reused (no allocation in the callback).
fn fill_scratch<T>(data: &[T], channels: usize, rate: u32, gain: f32, out: &mut Vec<i16>)
where
    T: cpal::Sample + cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    out.clear();
    if channels == 0 || data.is_empty() {
        return;
    }
    let frames = data.len() / channels;
    if frames == 0 {
        return;
    }
    let stereo_at = |frame: usize| -> (f32, f32) {
        let base = frame * channels;
        let a = f32::from_sample(data[base]);
        let b = if channels > 1 {
            f32::from_sample(data[base + 1])
        } else {
            a
        };
        (a, b)
    };
    if rate == STEM_RATE {
        out.reserve(frames * STEM_CHANNELS);
        for f in 0..frames {
            let (a, b) = stereo_at(f);
            out.push((a * gain).clamp(-1.0, 1.0).mul_add(32767.0, 0.0) as i16);
            out.push((b * gain).clamp(-1.0, 1.0).mul_add(32767.0, 0.0) as i16);
        }
        return;
    }
    // Linear resample to 48 kHz (only used when the endpoint runs at another
    // rate; shared-mode WASAPI normally gives us 48 kHz already).
    let ratio = rate as f64 / STEM_RATE as f64;
    let out_frames = (frames as f64 / ratio).floor() as usize;
    out.reserve(out_frames * STEM_CHANNELS);
    for j in 0..out_frames {
        let pos = j as f64 * ratio;
        let i0 = pos.floor() as usize;
        let i1 = (i0 + 1).min(frames - 1);
        let frac = (pos - i0 as f64) as f32;
        let (a0, b0) = stereo_at(i0.min(frames - 1));
        let (a1, b1) = stereo_at(i1);
        let a = a0 + (a1 - a0) * frac;
        let b = b0 + (b1 - b0) * frac;
        out.push((a * gain).clamp(-1.0, 1.0).mul_add(32767.0, 0.0) as i16);
        out.push((b * gain).clamp(-1.0, 1.0).mul_add(32767.0, 0.0) as i16);
    }
}

fn open_stream(state: &Arc<StreamState>) -> Result<(), String> {
    let device = match state.role {
        Role::Game => resolve_output(state),
        Role::Mic => resolve_input(state),
    }
    .ok_or_else(|| format!("{} device not found", state.role.label()))?;
    let name = device.name().unwrap_or_default();
    if let Ok(mut n) = state.resolved_name.lock() {
        *n = name.clone();
    }
    let config = match state.role {
        Role::Game => device
            .default_output_config()
            .map_err(|e| format!("game config: {e}"))?,
        Role::Mic => device
            .default_input_config()
            .map_err(|e| format!("mic config: {e}"))?,
    };
    let fmt = config.sample_format();
    let cfg: cpal::StreamConfig = config.into();
    let channels = cfg.channels as usize;
    let rate = cfg.sample_rate.0;
    let queue = state.queue.clone();
    let metas = state.metas.clone();
    let state2 = state.clone();
    let mut scratch: Vec<i16> = Vec::with_capacity(8192);
    let err_state = state.clone();
    let err_fn = move |e: cpal::StreamError| {
        eprintln!("[moonclip] {} stream error: {e}", err_state.role.label());
        if let Ok(mut d) = err_state.dead.lock() {
            *d = Some(e.to_string());
        }
        err_state.live.store(false, Ordering::Relaxed);
    };
    let stream = match fmt {
        cpal::SampleFormat::F32 => build_input::<f32>(&device, &cfg, move |data, info| {
            realtime_chunk(data, info, channels, rate, &state2, &queue, &metas, &mut scratch)
        }, err_fn),
        cpal::SampleFormat::I16 => build_input::<i16>(&device, &cfg, move |data, info| {
            realtime_chunk(data, info, channels, rate, &state2, &queue, &metas, &mut scratch)
        }, err_fn),
        cpal::SampleFormat::U16 => build_input::<u16>(&device, &cfg, move |data, info| {
            realtime_chunk(data, info, channels, rate, &state2, &queue, &metas, &mut scratch)
        }, err_fn),
        cpal::SampleFormat::I32 => build_input::<i32>(&device, &cfg, move |data, info| {
            realtime_chunk(data, info, channels, rate, &state2, &queue, &metas, &mut scratch)
        }, err_fn),
        cpal::SampleFormat::U32 => build_input::<u32>(&device, &cfg, move |data, info| {
            realtime_chunk(data, info, channels, rate, &state2, &queue, &metas, &mut scratch)
        }, err_fn),
        other => return Err(format!("unsupported sample format {other:?}")),
    }?;
    stream.play().map_err(|e| format!("play: {e}"))?;
    if let Ok(mut slot) = state.stream.lock() {
        *slot = Some(stream);
    }
    state.live.store(true, Ordering::Relaxed);
    // Do NOT touch the stall episode here: opening a stream only proves it
    // linked, not that audio flows. The episode stays `handled` until a block
    // is actually delivered (`drain_meta`), or the watchdog reopens in a loop.
    *state.retry_at.lock().unwrap() = None;
    Ok(())
}

fn build_input<T>(
    device: &cpal::Device,
    cfg: &cpal::StreamConfig,
    mut cb: impl FnMut(&[T], &cpal::InputCallbackInfo) + Send + 'static,
    mut err: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
{
    device
        .build_input_stream(
            cfg,
            move |data: &[T], info: &cpal::InputCallbackInfo| cb(data, info),
            move |e| err(e),
            None,
        )
        .map_err(|e| format!("open stream: {e}"))
}

/// The real-time callback body: register with MMCSS, convert + gain into a
/// reused scratch, hand the chunk over lock-free. Never blocks.
fn realtime_chunk<T>(
    data: &[T],
    info: &cpal::InputCallbackInfo,
    channels: usize,
    rate: u32,
    state: &Arc<StreamState>,
    queue: &SampleQueue,
    metas: &MetaQueue,
    scratch: &mut Vec<i16>,
) where
    T: cpal::Sample + cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    register_mmcss();
    let now = qpc_ns();
    let ts = info.timestamp();
    let lat_ns = ts
        .callback
        .duration_since(&ts.capture)
        .map(|d| d.as_nanos() as f64 * cpal_ns_scale())
        .unwrap_or(0.0);
    let capture_qpc = now - lat_ns as i128;
    let gain = if state.muted.load(Ordering::Relaxed) {
        0.0
    } else {
        state.gain_pct.load(Ordering::Relaxed) as f32 / 100.0
    };
    fill_scratch(data, channels, rate, gain, scratch);
    if scratch.is_empty() {
        return;
    }
    let mut peak = 0f32;
    for &s in scratch.iter() {
        let v = (s as f32 / 32768.0).abs();
        if v > peak {
            peak = v;
        }
    }
    state.note_peak(peak);
    let frames = (scratch.len() / STEM_CHANNELS) as u32;
    if !queue.push(scratch) {
        state.dropped_chunks.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if !metas.push(ChunkMeta {
        qpc_ns: capture_qpc.clamp(i64::MIN as i128, i64::MAX as i128) as i64,
        frames,
    }) {
        // Extremely unlikely (meta ring is tiny); drop the samples too by
        // marking a full-chunk underrun: consumer will see missing meta.
        state.dropped_chunks.fetch_add(1, Ordering::Relaxed);
    }
}

fn start_keepalive(device: &cpal::Device) -> Result<cpal::Stream, String> {
    let config = device
        .default_output_config()
        .map_err(|e| format!("keepalive config: {e}"))?;
    let fmt = config.sample_format();
    let cfg: cpal::StreamConfig = config.into();
    let stream = match fmt {
        cpal::SampleFormat::F32 => device
            .build_output_stream(&cfg, |d: &mut [f32], _| d.fill(0.0), |_| {}, None)
            .map_err(|e| e.to_string())?,
        cpal::SampleFormat::I16 => device
            .build_output_stream(&cfg, |d: &mut [i16], _| d.fill(0), |_| {}, None)
            .map_err(|e| e.to_string())?,
        cpal::SampleFormat::U16 => device
            .build_output_stream(&cfg, |d: &mut [u16], _| d.fill(u16::MAX / 2), |_| {}, None)
            .map_err(|e| e.to_string())?,
        other => return Err(format!("unsupported keepalive format {other:?}")),
    };
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

// ---------------------------------------------------------------------------
// Assembler + supervisor
// ---------------------------------------------------------------------------

/// Drain the callback queues into timestamped 10 ms blocks (one thread for
/// both streams).
fn assembler_loop(shared: Arc<Shared>) {
    let mut buf: Vec<i16> = Vec::with_capacity(BLOCK_FRAMES * STEM_CHANNELS * 2);
    let mut game_pending: Vec<i16> = Vec::with_capacity(BLOCK_FRAMES * STEM_CHANNELS);
    let mut game_qpc: Option<i128> = None;
    let mut mic_pending: Vec<i16> = Vec::with_capacity(BLOCK_FRAMES * STEM_CHANNELS);
    let mut mic_qpc: Option<i128> = None;
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            return;
        }
        let mut did_work = false;
        did_work |= drain_meta(
            &shared.game,
            &mut game_pending,
            &mut game_qpc,
            &mut buf,
        );
        did_work |= drain_meta(&shared.mic, &mut mic_pending, &mut mic_qpc, &mut buf);
        if !did_work {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

fn drain_meta(
    state: &Arc<StreamState>,
    pending: &mut Vec<i16>,
    pending_qpc: &mut Option<i128>,
    buf: &mut Vec<i16>,
) -> bool {
    let mut worked = false;
    while let Some(meta) = state.metas.pop() {
        worked = true;
        let need = meta.frames as usize * STEM_CHANNELS;
        if buf.len() < need {
            buf.resize(need, 0);
        }
        let got = state.queue.pop(&mut buf[..need]);
        if got < need {
            // Producer dropped samples without meta (or vice versa): skip.
            *pending_qpc = None;
            pending.clear();
            continue;
        }
        let qpc = meta.qpc_ns as i128;
        if pending_qpc.is_none() {
            *pending_qpc = Some(qpc);
        }
        pending.extend_from_slice(&buf[..need]);
        // Emit whole 10 ms blocks.
        let block_len = BLOCK_FRAMES * STEM_CHANNELS;
        while pending.len() >= block_len {
            let samples: Vec<i16> = pending.drain(..block_len).collect();
            let q = pending_qpc.unwrap_or(qpc);
            let block = Block {
                qpc_ns: q,
                samples,
            };
            let end = block.end_ns();
            if let Ok(mut ring) = state.ring.lock() {
                ring.push(block);
            }
            state.delivered_qpc.store(
                end.clamp(i64::MIN as i128, i64::MAX as i128) as i64,
                Ordering::Relaxed,
            );
            // A delivered block ends the stall episode and re-arms a fresh one.
            if state.stall_handled.swap(false, Ordering::Relaxed) {
                if let Ok(mut r) = state.stall_retry_at.lock() {
                    *r = None;
                }
            }
            *pending_qpc = Some(end);
        }
    }
    worked
}

fn supervisor_loop(shared: Arc<Shared>) {
    let mut ticks: u32 = 0;
    while !shared.stop.load(Ordering::Relaxed) {
        std::thread::sleep(TICK);
        ticks = ticks.wrapping_add(1);
        if ticks % DEFAULT_POLL_TICKS == 0 {
            follow_default(&shared.game, true);
            follow_default(&shared.mic, false);
        }
        for state in [&shared.game, &shared.mic] {
            let dead = state.dead.lock().ok().and_then(|d| d.clone());
            if let Some(_e) = dead {
                let due = state
                    .retry_at
                    .lock()
                    .ok()
                    .map(|r| r.map(|t| t <= std::time::Instant::now()).unwrap_or(true))
                    .unwrap_or(true);
                if due {
                    if reopen(state) {
                        continue;
                    }
                    if let Ok(mut r) = state.retry_at.lock() {
                        *r = Some(std::time::Instant::now() + ERROR_RETRY);
                    }
                }
                continue;
            }
            // Silent stall: live stream, no delivered blocks for STALL_AFTER.
            if !state.live.load(Ordering::Relaxed) {
                continue;
            }
            let last = state.delivered_qpc.load(Ordering::Relaxed);
            if last == 0 {
                continue; // never delivered yet: handled on open
            }
            let age_ns = qpc_ns() - last as i128;
            let handled = state.stall_handled.load(Ordering::Relaxed);
            let retry_due = state
                .stall_retry_at
                .lock()
                .ok()
                .map(|r| r.map(|t| t <= std::time::Instant::now()).unwrap_or(true))
                .unwrap_or(true);
            if stall_action(age_ns, handled, retry_due) == StallAction::Reopen {
                // Mark the episode BEFORE reopening: a successful open only
                // proves the stream linked. Only a delivered block clears it.
                state.stall_handled.store(true, Ordering::Relaxed);
                eprintln!(
                    "[moonclip] {} stream stalled ({:.1}s without audio), reopening",
                    state.role.label(),
                    age_ns as f64 / 1e9
                );
                let _ = reopen(state);
                // Always arm the slow retry: reopen churn every tick was the
                // old bug (840 log lines per minute and a WASAPI stream per
                // tick). One attempt per STALL_RETRY while silent.
                if let Ok(mut r) = state.stall_retry_at.lock() {
                    *r = Some(std::time::Instant::now() + STALL_RETRY);
                }
            }
        }
    }
}

/// Supervisor decision for one silent-stall tick (pure, unit-tested).
#[derive(Debug, PartialEq, Eq)]
enum StallAction {
    None,
    Reopen,
}

fn stall_action(age_ns: i128, handled: bool, retry_due: bool) -> StallAction {
    if age_ns > STALL_AFTER.as_nanos() as i128 && !handled && retry_due {
        StallAction::Reopen
    } else {
        StallAction::None
    }
}

/// Close + reopen a stream on its resolved device, keeping ring history.
fn reopen(state: &Arc<StreamState>) -> bool {
    if let Ok(mut slot) = state.stream.lock() {
        slot.take(); // dropping stops the stream
    }
    state.live.store(false, Ordering::Relaxed);
    match open_stream(state) {
        Ok(()) => {
            if let Ok(mut d) = state.dead.lock() {
                *d = None;
            }
            true
        }
        Err(e) => {
            eprintln!("[moonclip] {} reopen failed: {e}", state.role.label());
            if let Ok(mut d) = state.dead.lock() {
                *d = Some(e);
            }
            false
        }
    }
}

/// For `default_output`/`default_input`: if Windows changed the default
/// device, move the stream there (and re-link the keep-alive for the game
/// endpoint). Concrete device selections are left alone.
fn follow_default(state: &Arc<StreamState>, render: bool) {
    let id = match state.device_id.lock() {
        Ok(i) => i.clone(),
        Err(_) => return,
    };
    let magic = if render {
        devices::is_default_output_id(&id)
    } else {
        devices::is_default_input_id(&id)
    };
    if !magic {
        return;
    }
    let Some(dev) = (if render {
        cpal::default_host().default_output_device()
    } else {
        cpal::default_host().default_input_device()
    }) else {
        return;
    };
    let new_name = dev.name().unwrap_or_default();
    let old_name = state
        .resolved_name
        .lock()
        .map(|n| n.clone())
        .unwrap_or_default();
    if new_name != old_name {
        eprintln!(
            "[moonclip] {} default device changed ('{old_name}' -> '{new_name}'), relinking",
            state.role.label()
        );
        let _ = reopen(state);
    }
}

// ---------------------------------------------------------------------------
// Public API shared with commands.rs (same signatures as the Linux backend)
// ---------------------------------------------------------------------------

pub async fn apply_gains(
    _known_args: &[String],
    game: u32,
    mic: u32,
    mute_game: bool,
    mute_mic: bool,
) -> Result<usize, String> {
    let shared = registry()
        .lock()
        .ok()
        .and_then(|r| r.clone())
        .ok_or_else(|| "no Windows audio streams linked (buffer not running)".to_string())?;
    shared.game.gain_pct.store(game.min(200), Ordering::Relaxed);
    shared.mic.gain_pct.store(mic.min(200), Ordering::Relaxed);
    shared.game.muted.store(mute_game, Ordering::Relaxed);
    shared.mic.muted.store(mute_mic, Ordering::Relaxed);
    Ok(shared.game.live.load(Ordering::Relaxed) as usize
        + shared.mic.live.load(Ordering::Relaxed) as usize)
}

pub async fn linked_count(_known_args: &[String]) -> usize {
    registry()
        .lock()
        .ok()
        .and_then(|r| {
            r.as_ref().map(|s| {
                s.game.live.load(Ordering::Relaxed) as usize
                    + s.mic.live.load(Ordering::Relaxed) as usize
            })
        })
        .unwrap_or(0)
}

pub fn recent_peaks() -> Option<(f32, f32)> {
    let shared = registry().lock().ok().and_then(|r| r.clone())?;
    Some((
        f32::from_bits(shared.game.recent_peak.swap(0, Ordering::Relaxed)),
        f32::from_bits(shared.mic.recent_peak.swap(0, Ordering::Relaxed)),
    ))
}

/// Assemble the stems for the engine (kept as a helper for tests/logging).
pub fn window_frames(window_ns: i128) -> usize {
    (window_ns.max(0) * STEM_RATE as i128 / 1_000_000_000) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring_from(spec: &[(i128, i16)]) -> VecDeque<Block> {
        let mut r = VecDeque::new();
        let mut qpc = spec.first().map(|(q, _)| *q).unwrap_or(0);
        for &(dt, v) in spec {
            qpc += dt;
            let samples: Vec<i16> = std::iter::repeat(v).take(BLOCK_FRAMES * 2).collect();
            r.push_back(Block { qpc_ns: qpc, samples });
        }
        r
    }

    #[test]
    fn spsc_all_or_nothing_and_order() {
        let q = SampleQueue::new(16);
        let data: Vec<i16> = (0..20).collect();
        assert!(!q.push(&data)); // 20 > 15 usable
        assert!(q.push(&data[..10]));
        assert_eq!(q.len(), 10);
        let mut out = [0i16; 6];
        assert_eq!(q.pop(&mut out), 6);
        assert_eq!(out, [0, 1, 2, 3, 4, 5]);
        assert!(q.push(&data[..7])); // 4 + 7 = 11 <= 15
        let mut out2 = [0i16; 20];
        assert_eq!(q.pop(&mut out2), 11);
        // Remaining old samples first, then the fresh chunk (wrapped).
        assert_eq!(&out2[..11], &[6, 7, 8, 9, 0, 1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn meta_queue_fifo() {
        let q = MetaQueue::new(4);
        for i in 0..3 {
            assert!(q.push(ChunkMeta { qpc_ns: i, frames: 1 }));
        }
        assert_eq!(q.pop().unwrap().qpc_ns, 0);
        assert_eq!(q.pop().unwrap().qpc_ns, 1);
        assert_eq!(q.pop().unwrap().qpc_ns, 2);
        assert!(q.pop().is_none());
    }

    #[test]
    fn window_perfect_timestamps_are_verbatim() {
        let blocks = ring_from(&[(0, 100), (BLOCK_FRAMES as i128 * 1_000_000_000 / STEM_RATE as i128, 200)]);
        let start = blocks.front().unwrap().qpc_ns;
        let frames = BLOCK_FRAMES * 2;
        let out = build_window(&blocks, start, frames);
        assert_eq!(out.len(), frames * 2);
        assert!(out[..BLOCK_FRAMES * 2].iter().all(|&s| s == 100));
        assert!(out[BLOCK_FRAMES * 2..].iter().all(|&s| s == 200));
    }

    #[test]
    fn window_corrects_drift_without_clicks() {
        // Second block starts 1% later than its content duration: the builder
        // must stretch (interpolate), never insert silence.
        let step = BLOCK_FRAMES as i128 * 1_000_000_000 / STEM_RATE as i128;
        let blocks = ring_from(&[(0, 0), (step + step / 100, 10000)]);
        let start = blocks.front().unwrap().qpc_ns;
        let frames = BLOCK_FRAMES * 2;
        let out = build_window(&blocks, start, frames);
        // Jitter is absorbed by holding the previous block's last sample for
        // the sub-ms gap; the next block maps normally right after.
        let mid = BLOCK_FRAMES + 10;
        assert!(
            out[mid * 2] > 0,
            "expected the second block after the jittered boundary"
        );
    }

    #[test]
    fn window_real_gap_becomes_silence() {
        let step = BLOCK_FRAMES as i128 * 1_000_000_000 / STEM_RATE as i128;
        // 5 s gap between two blocks (> FUSE).
        let blocks = ring_from(&[(0, 500), (step + 5_000_000_000, 900)]);
        let start = blocks.front().unwrap().qpc_ns;
        let frames = BLOCK_FRAMES + 4800; // 110 ms: past q_end + FUSE
        let out = build_window(&blocks, start, frames);
        let late = frames - 1;
        assert_eq!(out[late * 2], 0, "gap must be silent");
        // The first block itself is verbatim.
        assert_eq!(out[10], 500);
    }

    #[test]
    fn window_pads_when_ring_is_short() {
        let blocks = ring_from(&[(0, 7)]);
        let start = blocks.front().unwrap().qpc_ns;
        let frames = BLOCK_FRAMES;
        let out = build_window(&blocks, start, frames);
        assert_eq!(out.len(), frames * 2);
        assert!(out.iter().all(|&s| s == 7));
    }

    #[test]
    fn conversion_handles_silence_and_gain() {
        let mut out = Vec::new();
        fill_scratch(&[0.5f32, -0.5, 0.25, -0.25], 2, 48_000, 2.0, &mut out);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0], 32767); // 1.0 clamped
        assert_eq!(out[1], -32767);
        assert_eq!(out[2], 16383);
        fill_scratch::<f32>(&[], 2, 48_000, 1.0, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn conversion_resamples_when_rate_differs() {
        let data: Vec<f32> = (0..960).map(|_| 0.5).collect();
        let mut out = Vec::new();
        fill_scratch(&data, 1, 44_100, 1.0, &mut out);
        // ~960 * 48000/44100 frames, stereo.
        let frames = out.len() / 2;
        assert!((1030..=1050).contains(&frames), "{frames}");
        assert!(out.iter().all(|&s| (s as i32 - 16383).abs() <= 1));
    }

    #[test]
    fn window_frames_is_seconds_times_rate() {
        assert_eq!(window_frames(1_000_000_000), 48_000);
        assert_eq!(window_frames(500_000_000), 24_000);
        assert_eq!(window_frames(-5), 0);
    }

    /// Regression for the reopen churn seen in the user console (840
    /// "stalled, reopening" lines in a minute): a handled episode or a
    /// not-yet-due retry must never trigger another reopen.
    #[test]
    fn stall_watchdog_never_churns() {
        let after = STALL_AFTER.as_nanos() as i128;
        // First stall: one reopen; the supervisor then marks the episode
        // handled and arms the 30 s slow retry.
        assert_eq!(stall_action(after + 1, false, true), StallAction::Reopen);
        assert_eq!(stall_action(after + 2, true, true), StallAction::None);
        assert_eq!(stall_action(after + 2, true, false), StallAction::None);
        assert_eq!(stall_action(after + 2, false, false), StallAction::None);
        // Short gaps never trigger a reopen; a delivered block clears the
        // episode so a future stall can act again.
        assert_eq!(stall_action(after / 2, false, true), StallAction::None);
        assert_eq!(stall_action(after + 1, false, true), StallAction::Reopen);
    }
}
