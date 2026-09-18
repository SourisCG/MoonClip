//! Windows audio capture: WASAPI loopback (game) + microphone, 48 kHz stereo
//! i16 rings with QPC timestamps.
//!
//! Design rules (why this file exists):
//! - **One clock for everything: QPC.** Each captured packet carries WASAPI's
//!   `QPCPosition` (`BufferInfo::timestamp`, raw counter ticks) and the engine
//!   maps the video keyframe PTS to QPC to rebuild each stem over exactly that
//!   window. `dsp::build_window` resamples by block timestamps, so
//!   device-clock drift (a few hundred ppm) cannot shift A/V over long
//!   buffers.
//! - **Capture threads never block on heavy locks.** The WASAPI read thread
//!   only converts + gains + pushes into a lock-free SPSC ring; the assembler
//!   thread builds 10 ms blocks into the stems.
//! - **Endpoint loopback** = render endpoint + `Direction::Capture` in shared
//!   mode (the crate sets `AUDCLNT_STREAMFLAGS_LOOPBACK`); a silent keep-alive
//!   render stream keeps WASAPI delivering packets while the game is quiet.
//! - **MMCSS "Pro Audio"** on the capture and keep-alive threads.

use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use wasapi::{
    AudioCaptureClient, AudioClient, Direction, Handle, SampleType, StreamMode, WaveFormat,
};
use windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW;

use super::devices;
use super::dsp::{self, build_window, Block, STEM_CHANNELS, STEM_RATE};
use super::pts;

/// Ring block size handed to the assembler (10 ms stereo) — from dsp.
const BLOCK_FRAMES: usize = dsp::BLOCK_FRAMES;
/// Raw callback queue per stream (~2 s at 48 kHz stereo).
const QUEUE_SAMPLES: usize = 1 << 17;
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
/// Stream setup must hand back a started client within this budget.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
/// Fallback device period (20 ms in 100 ns units) when the driver hides it.
const FALLBACK_PERIOD_HNS: i64 = 200_000;

// ---------------------------------------------------------------------------
// Lock-free SPSC queues (single producer = capture thread, single consumer =
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
// Stems: timestamped 10 ms blocks
// ---------------------------------------------------------------------------

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

/// Owns a capture/keep-alive thread: dropping it stops the thread and joins.
struct StreamHandle {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct StreamState {
    role: Role,
    /// Configured device id (magic `default_*` follows the OS default).
    device_id: Mutex<String>,
    /// Friendly name of the resolved device (for default-follow comparisons).
    resolved_name: Mutex<String>,
    stream: Mutex<Option<StreamHandle>>,
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

    fn gain(&self) -> f32 {
        if self.muted.load(Ordering::Relaxed) {
            0.0
        } else {
            self.gain_pct.load(Ordering::Relaxed) as f32 / 100.0
        }
    }
}

struct Shared {
    game: Arc<StreamState>,
    mic: Arc<StreamState>,
    keepalive: Mutex<Option<StreamHandle>>,
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
        let live = game.live.load(Ordering::Relaxed) as usize
            + mic.live.load(Ordering::Relaxed) as usize;
        if live == 0 {
            return Err("no audio streams could be opened".into());
        }
        // Silent keep-alive on the render endpoint: WASAPI delivers no packets
        // while the audio engine is idle, which used to look like a dead
        // stream and churned the watchdog. Keeping a render client alive
        // makes loopback deliver continuous silence.
        {
            let id = game.device_id.lock().map(|d| d.clone()).unwrap_or_default();
            match start_keepalive(&id) {
                Ok(h) => *shared.keepalive.lock().unwrap() = Some(h),
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
        // Dropping each handle stops + joins its thread.
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
// Stream opening + capture threads
// ---------------------------------------------------------------------------

fn mark_dead(state: &StreamState, msg: String) {
    eprintln!("[moonclip] {} stream error: {msg}", state.role.label());
    if let Ok(mut d) = state.dead.lock() {
        *d = Some(msg);
    }
    state.live.store(false, Ordering::Relaxed);
}

/// Open a capture stream: spawn its thread, wait for the client to start, and
/// keep the handle. A setup failure is returned synchronously.
fn open_stream(state: &Arc<StreamState>) -> Result<(), String> {
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
    let stop = Arc::new(AtomicBool::new(false));
    let thread_state = state.clone();
    let thread_stop = stop.clone();
    let handle = std::thread::Builder::new()
        .name(format!("moonclip-wasapi-{}", state.role.label()))
        .spawn(move || capture_thread(thread_state, thread_stop, ready_tx))
        .map_err(|e| format!("cannot spawn capture thread: {e}"))?;
    match ready_rx.recv_timeout(OPEN_TIMEOUT) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err("audio stream did not start in 5 s".into()),
    }
    if let Ok(mut slot) = state.stream.lock() {
        *slot = Some(StreamHandle {
            stop,
            thread: Some(handle),
        });
    }
    state.live.store(true, Ordering::Relaxed);
    // Do NOT touch the stall episode here: opening a stream only proves it
    // linked, not that audio flows. The episode stays `handled` until a block
    // is actually delivered (`drain_meta`), or the watchdog reopens in a loop.
    *state.retry_at.lock().unwrap() = None;
    Ok(())
}

/// Wait for the next packet and drain it into the SPSC ring. Tight loop only
/// while packets are pending; otherwise parks on the WASAPI event.
fn capture_thread(
    state: Arc<StreamState>,
    stop: Arc<AtomicBool>,
    ready: SyncSender<Result<(), String>>,
) {
    let setup = setup_capture(&state);
    let (client, capture, event, channels) = match setup {
        Ok(v) => {
            ready.send(Ok(())).ok();
            v
        }
        Err(e) => {
            ready.send(Err(e)).ok();
            return;
        }
    };
    register_mmcss();
    let mut f32_buf: Vec<f32> = Vec::new();
    let mut scratch: Vec<i16> = Vec::with_capacity(8192);
    while !stop.load(Ordering::Relaxed) {
        // 50 ms timeout keeps the stop flag responsive without busy-waiting.
        if event.wait_for_event(50).is_err() {
            continue;
        }
        loop {
            match capture.get_next_packet_size() {
                Ok(Some(frames)) if frames > 0 => {
                    let need = frames as usize * channels;
                    if f32_buf.len() < need {
                        f32_buf.resize(need, 0.0);
                    }
                    // SAFETY: `f32_buf` is a `Vec<f32>`, so its backing store
                    // is 4-byte aligned and at least `need * 4` bytes long;
                    // the byte view is only used as the WASAPI destination.
                    let raw = unsafe {
                        std::slice::from_raw_parts_mut(
                            f32_buf.as_mut_ptr() as *mut u8,
                            need * std::mem::size_of::<f32>(),
                        )
                    };
                    match capture.read_from_device(raw) {
                        Ok((frames_read, info)) => {
                            let n = frames_read as usize * channels;
                            dsp::convert_f32(&f32_buf[..n], state.gain(), &mut scratch);
                            state.note_peak(dsp::peak_i16(&scratch));
                            let qpc = pts::qpc_ticks_to_ns(info.timestamp);
                            let frames_u32 = (scratch.len() / STEM_CHANNELS) as u32;
                            if frames_u32 > 0 {
                                if state.queue.push(&scratch) {
                                    if !state.metas.push(ChunkMeta {
                                        qpc_ns: qpc.clamp(i64::MIN as i128, i64::MAX as i128) as i64,
                                        frames: frames_u32,
                                    }) {
                                        state.dropped_chunks.fetch_add(1, Ordering::Relaxed);
                                    }
                                } else {
                                    state.dropped_chunks.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                        Err(e) => {
                            mark_dead(&state, e.to_string());
                            break;
                        }
                    }
                }
                Ok(_) => break,
                Err(e) => {
                    mark_dead(&state, e.to_string());
                    break;
                }
            }
        }
    }
    let _ = client.stop_stream();
}

/// Resolve the configured endpoint and start a shared-mode capture client at
/// 48 kHz stereo float (the engine converts formats for us).
fn setup_capture(
    state: &Arc<StreamState>,
) -> Result<(AudioClient, AudioCaptureClient, Handle, usize), String> {
    let id = state
        .device_id
        .lock()
        .map_err(|_| "device id lock poisoned".to_string())?
        .clone();
    let render = matches!(state.role, Role::Game);
    let device = devices::resolve_endpoint(&id, render)?;
    if let Ok(name) = device.get_friendlyname() {
        if let Ok(mut n) = state.resolved_name.lock() {
            *n = name;
        }
    }
    let mut client = device
        .get_iaudioclient()
        .map_err(|e| format!("{} client: {e}", state.role.label()))?;
    let min_time = client
        .get_device_period()
        .map(|(_, m)| m)
        .unwrap_or(FALLBACK_PERIOD_HNS);
    let format = WaveFormat::new(
        32,
        32,
        &SampleType::Float,
        STEM_RATE as usize,
        STEM_CHANNELS,
        None,
    );
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: min_time,
    };
    client
        .initialize_client(&format, &Direction::Capture, &mode)
        .map_err(|e| format!("{} init: {e}", state.role.label()))?;
    let event = client
        .set_get_eventhandle()
        .map_err(|e| format!("{} event: {e}", state.role.label()))?;
    let capture = client
        .get_audiocaptureclient()
        .map_err(|e| format!("{} capture client: {e}", state.role.label()))?;
    client
        .start_stream()
        .map_err(|e| format!("{} start: {e}", state.role.label()))?;
    Ok((client, capture, event, STEM_CHANNELS))
}

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
                let _ = AvSetMmThreadCharacteristicsW(windows::core::PCWSTR(task.as_ptr()), &mut idx);
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Keep-alive render stream (loopback goes silent when nothing renders)
// ---------------------------------------------------------------------------

fn start_keepalive(device_id: &str) -> Result<StreamHandle, String> {
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
    let stop = Arc::new(AtomicBool::new(false));
    let id = device_id.to_string();
    let thread_stop = stop.clone();
    let handle = std::thread::Builder::new()
        .name("moonclip-wasapi-keepalive".into())
        .spawn(move || keepalive_thread(id, thread_stop, ready_tx))
        .map_err(|e| format!("cannot spawn keepalive thread: {e}"))?;
    match ready_rx.recv_timeout(OPEN_TIMEOUT) {
        Ok(Ok(())) => Ok(StreamHandle {
            stop,
            thread: Some(handle),
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("keepalive stream did not start in 5 s".into()),
    }
}

fn keepalive_thread(id: String, stop: Arc<AtomicBool>, ready: SyncSender<Result<(), String>>) {
    let setup = (|| -> Result<(AudioClient, wasapi::AudioRenderClient, Handle, usize), String> {
        let device = devices::resolve_endpoint(&id, true)?;
        let mut client = device
            .get_iaudioclient()
            .map_err(|e| format!("keepalive client: {e}"))?;
        let min_time = client
            .get_device_period()
            .map(|(_, m)| m)
            .unwrap_or(FALLBACK_PERIOD_HNS);
        let format = WaveFormat::new(
            32,
            32,
            &SampleType::Float,
            STEM_RATE as usize,
            STEM_CHANNELS,
            None,
        );
        let mode = StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: min_time,
        };
        client
            .initialize_client(&format, &Direction::Render, &mode)
            .map_err(|e| format!("keepalive init: {e}"))?;
        let event = client
            .set_get_eventhandle()
            .map_err(|e| format!("keepalive event: {e}"))?;
        let render = client
            .get_audiorenderclient()
            .map_err(|e| format!("keepalive render client: {e}"))?;
        client
            .start_stream()
            .map_err(|e| format!("keepalive start: {e}"))?;
        let blockalign = format.get_blockalign() as usize;
        Ok((client, render, event, blockalign))
    })();
    let (client, render, event, blockalign) = match setup {
        Ok(v) => {
            ready.send(Ok(())).ok();
            v
        }
        Err(e) => {
            ready.send(Err(e)).ok();
            return;
        }
    };
    register_mmcss();
    let buffer_frames = client.get_buffer_size().unwrap_or(0);
    let silence = vec![0u8; (buffer_frames as usize).max(1) * blockalign];
    while !stop.load(Ordering::Relaxed) {
        if event.wait_for_event(100).is_err() {
            continue;
        }
        loop {
            match client.get_available_space_in_frames() {
                Ok(frames) if frames > 0 => {
                    let n = (frames as usize).min(buffer_frames as usize);
                    let bytes = &silence[..n * blockalign];
                    if render.write_to_device(n, bytes, None).is_err() {
                        break;
                    }
                }
                Ok(_) => break,
                Err(_) => break,
            }
        }
    }
    let _ = client.stop_stream();
}

/// Restart the keep-alive on the game endpoint's current device (called when
/// the default output device changes).
fn restart_keepalive(shared: &Arc<Shared>) {
    if let Ok(mut kl) = shared.keepalive.lock() {
        kl.take();
    }
    let id = shared
        .game
        .device_id
        .lock()
        .map(|d| d.clone())
        .unwrap_or_default();
    match start_keepalive(&id) {
        Ok(h) => {
            if let Ok(mut kl) = shared.keepalive.lock() {
                *kl = Some(h);
            }
        }
        Err(e) => eprintln!("[moonclip] keepalive restart failed: {e}"),
    }
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
        did_work |= drain_meta(&shared.game, &mut game_pending, &mut game_qpc, &mut buf);
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
        if ticks.is_multiple_of(DEFAULT_POLL_TICKS) {
            follow_default(&shared, true);
            follow_default(&shared, false);
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
            let age_ns = pts::qpc_ns() - last as i128;
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
        slot.take(); // dropping stops + joins the capture thread
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
fn follow_default(shared: &Arc<Shared>, render: bool) {
    let state = if render { &shared.game } else { &shared.mic };
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
        devices::resolve_endpoint("default_output", true).ok()
    } else {
        devices::resolve_endpoint("default_input", false).ok()
    }) else {
        return;
    };
    let new_name = dev.get_friendlyname().unwrap_or_default();
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
        if render {
            restart_keepalive(shared);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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
            assert!(q.push(ChunkMeta {
                qpc_ns: i,
                frames: 1
            }));
        }
        assert_eq!(q.pop().unwrap().qpc_ns, 0);
        assert_eq!(q.pop().unwrap().qpc_ns, 1);
        assert_eq!(q.pop().unwrap().qpc_ns, 2);
        assert!(q.pop().is_none());
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
