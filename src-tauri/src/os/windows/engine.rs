//! Windows engine: ffmpeg `gfxcapture` (Windows.Graphics.Capture, D3D11
//! zero-copy) -> hardware encoder -> MPEG-TS RAM ring.
//!
//! Replay semantics mirror Linux GSR (`-r` ring + remux on hotkey): encoded
//! video packets (MPEG-TS) and PCM stems live in RAM only — zero disk writes
//! while idle. `save_clip` cuts the ring at a keyframe and muxes 3×AAC. No
//! DLL injection anywhere (gfxcapture is the same OS API Xbox Game Bar uses —
//! anti-cheat safe). OS floor: Windows 10 1903+.
//!
//! Why ffmpeg captures for us: the previous design pulled every WGC frame to
//! the CPU, piped raw BGRA to ffmpeg and ran swscale — up to ~500 MB/s of
//! memcpy plus a software color conversion. Under a heavy game the pipe
//! stalled, frames were dropped and the CFR pacer re-emitted the last frame
//! for minutes (a "frozen" clip). The `gfxcapture` source keeps frames on the
//! GPU (D3D11) straight into NVENC/AMF/QSV, and ffmpeg's own muxer owns the
//! timestamps: one process, no pipes, no conversion. `ts.rs` indexes the
//! resulting TS (PTS + keyframes) and calibrates it against the local QPC
//! clock, which is what makes A/V sync a one-line mapping instead of a pile
//! of per-codec offsets.

use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, AtomicI64, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use tokio::process::{Child, Command};

use super::super::{CaptureConfig, CaptureEngine, SavePlan};
use super::audio::AudioCapture;
use super::detector::MonitorTarget;
use super::dsp;
use super::encode;
use super::mux;
use super::pts;
use super::ring::{CutPlan, TsRing};
use super::video::{self, CaptureSource};

/// Extra seconds kept beyond the configured buffer (ring headroom).
const RING_MARGIN_SECS: u64 = 8;
/// How long `start_buffer` waits for indexed video before giving up.
const FRAME_PROOF_MS: u64 = 2500;
const FRAME_PROOF_POLL_MS: u64 = 50;
/// Shortest clip the ring cut will produce after the keyframe.
const SAVE_MIN_CLIP_NS: i64 = 1_000_000_000;
/// No fresh encoded bytes for this long (with a live encoder) = source stalled
/// (e.g. true exclusive fullscreen, which WGC cannot capture). Surfaced in
/// `log_tail`, never fabricated into frames.
const STALL_AFTER: Duration = Duration::from_secs(3);
/// TS read chunk from the encoder pipe.
const TS_READ_BUF: usize = 64 * 1024;
/// Kernel buffer for the encoder's TS stdout pipe (see `big_pipe`).
const TS_PIPE_BYTES: u32 = 8 * 1024 * 1024;
/// Kernel buffer for the encoder's stderr pipe (showinfo clock ~25 KB/s).
const ERR_PIPE_BYTES: u32 = 1024 * 1024;
/// `signalstats` YAVG under which a probed frame counts as black. Limited
/// range puts pure black at 16; a real game frame sits far above 20.
const BLACK_YAVG: f64 = 20.0;

/// CPU priority class for the encoder child from `MOONCLIP_CAPTURE_CPU_PRIO`:
/// unset = HIGH (games run at HIGH; ABOVE_NORMAL loses the CPU under load),
/// `0`/`off`/`above` = the old ABOVE_NORMAL, `normal` = NORMAL.
fn cpu_priority_class(
    env: Option<&str>,
) -> windows::Win32::System::Threading::PROCESS_CREATION_FLAGS {
    use windows::Win32::System::Threading::{
        ABOVE_NORMAL_PRIORITY_CLASS, HIGH_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
    };
    match env.map(str::trim) {
        Some("0") | Some("off") | Some("false") | Some("above") => ABOVE_NORMAL_PRIORITY_CLASS,
        Some("normal") => NORMAL_PRIORITY_CLASS,
        _ => HIGH_PRIORITY_CLASS,
    }
}

/// GPU scheduler class for the encoder child from `MOONCLIP_CAPTURE_GPU_PRIO`:
/// unset = HIGH(4) (the same lever OBS raises under HAGS), `0`/`off`/`false`
/// disables it, `1` kept as the legacy alias for HIGH, `2..=4` an explicit
/// class. HIGH is requested by default because WGC capture + NVENC starve
/// behind a game that saturates the GPU (fresh frames stop arriving and the
/// CFR filter turns the gaps into duplicates).
fn gpu_priority_level(env: Option<&str>) -> Option<i32> {
    match env.map(str::trim) {
        None | Some("") => Some(4),
        Some("0") | Some("off") | Some("false") => None,
        Some("1") => Some(4),
        Some(v) => v.parse::<i32>().ok().filter(|l| (2..=4).contains(l)),
    }
}

// ---------------------------------------------------------------------------
// Launch plan (unit-tested)
// ---------------------------------------------------------------------------

/// Everything needed to (re)launch the capture child. Stored at start so the
/// WGC->DXGI fallback (black picture / legacy exclusive fullscreen) can
/// respawn without re-running discovery or the codec probe.
struct SpawnPlan {
    ffmpeg: PathBuf,
    vendor: String,
    codec: String,
    enc_name: &'static str,
    monitor: MonitorTarget,
    out_height: u32,
    bitrate_kbps: u32,
    fps: u32,
    nvenc_hq: bool,
    nvenc_preset: String,
    encoder_full: bool,
    env_source: String,
    env_window_exe: Option<String>,
    max_fps: u32,
}

impl SpawnPlan {
    /// CaptureSource for a given source kind (`gfxcapture`/`ddagrab`). Window
    /// selectors only apply to WGC; the DDA fallback always targets the
    /// monitor.
    fn source<'a>(&'a self, kind: &'a str) -> CaptureSource<'a> {
        let window_mode = self.env_source == "window" || self.env_window_exe.is_some();
        let (mut hwnd, mut window_exe) = (None, None);
        if window_mode && kind == "gfxcapture" {
            match self.env_window_exe.as_deref() {
                Some(exe) => window_exe = Some(exe),
                None => hwnd = video::foreground_window().map(|h| h as u64),
            }
        }
        if window_mode && hwnd.is_none() && window_exe.is_none() {
            eprintln!(
                "[moonclip] window capture requested but no target window found; falling back to monitor"
            );
        }
        CaptureSource {
            kind,
            hmonitor: self.monitor.hmonitor,
            monitor_idx: self.monitor.index,
            hwnd,
            window_exe,
            max_fps: self.max_fps,
        }
    }
}

/// Parse a `showinfo` frame line's PTS tick count (`pts: 12345`).
fn parse_showinfo_pts(line: &str) -> Option<i64> {
    let idx = line.find(" pts:")? + " pts:".len();
    let rest = line[idx..].trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '-'))
        .unwrap_or(rest.len());
    rest[..end].parse::<i64>().ok()
}

/// dBFS for the save stats line (-inf when the stem is digital silence).
fn dbfs(v: f32) -> String {
    if v <= 0.0 {
        "-inf".to_string()
    } else {
        format!("{:.1}dB", 20.0 * v.log10())
    }
}

// ---------------------------------------------------------------------------
// Legacy probe helpers (fallback for codecs without a keyframe scanner)
// ---------------------------------------------------------------------------

/// Resync a TS window: first offset where `0x47` starts an aligned pair.
fn cut_ts_window(ring: &[u8], keep_bytes: usize) -> &[u8] {
    let start = ring.len().saturating_sub(keep_bytes);
    let win = &ring[start..];
    let scan = win.len().min(188 * 8);
    for i in 0..scan {
        if win[i] == 0x47 && (i + 188 >= win.len() || win[i + 188] == 0x47) {
            return &win[i..];
        }
    }
    win
}

/// Parse `pts_time:12.345` from an ffmpeg `showinfo` line.
fn parse_pts_time(line: &str) -> Option<f64> {
    let idx = line.find("pts_time:")? + "pts_time:".len();
    let rest = &line[idx..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(rest.len());
    rest[..end].parse::<f64>().ok()
}

/// Time of the first decodable (key)frame inside the cut TS, relative to its
/// start. Only used for codecs without a keyframe scanner (e.g. AV1).
async fn first_keyframe_secs(ffmpeg: &Path, cut_ts: &Path) -> Option<f64> {
    let out = tokio::process::Command::new(ffmpeg)
        .args(["-hide_banner", "-i"])
        .arg(cut_ts)
        .args(["-vf", "showinfo", "-frames:v", "1", "-f", "null", "-"])
        .output()
        .await
        .ok()?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    let line = stderr.lines().find(|l| l.contains("pts_time:"))?;
    parse_pts_time(line)
}

fn sanitize_start_trim(trim: Option<f64>, video_secs: f64) -> f64 {
    match trim {
        Some(t) if (0.05..=3.5).contains(&t) && video_secs - t >= 1.0 => t,
        _ => 0.0,
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

pub struct WindowsCaptureEngine {
    child: Option<Child>,
    /// Encoded MPEG-TS ring with the PES/keyframe index + QPC calibration.
    ring: Arc<Mutex<TsRing>>,
    video_dead: Arc<AtomicBool>,
    /// QPC ns of the last byte read from the encoder (stall detection).
    last_rx_qpc: Arc<AtomicI64>,
    stall_logged: Arc<AtomicBool>,
    /// Bounded tail of encoder stderr (drained live). Feeds `log_tail()`.
    stderr_ring: Arc<Mutex<VecDeque<String>>>,
    audio: Option<AudioCapture>,
    output_dir: PathBuf,
    audio_args: Vec<String>,
    audio_single_track: bool,
    /// `mp4` (default) or `mkv`, from settings.
    container: String,
    /// MP4 `+faststart` (off by default: doubles save I/O).
    faststart: bool,
    duration_secs: u32,
    bitrate_kbps: u32,
    fps: u32,
    ffmpeg: PathBuf,
    /// Ring capacity granted at start (for the legacy fallback window).
    ring_cap: usize,
    /// Launch parameters, kept so the WGC->DXGI fallback can respawn.
    plan: Option<SpawnPlan>,
    source_kind: String,
    fallback_done: bool,
    fallback_requested: bool,
}

impl WindowsCaptureEngine {
    pub fn new() -> Self {
        Self {
            child: None,
            ring: Arc::new(Mutex::new(TsRing::new(16 * 1024 * 1024, 60))),
            video_dead: Arc::new(AtomicBool::new(false)),
            last_rx_qpc: Arc::new(AtomicI64::new(0)),
            stall_logged: Arc::new(AtomicBool::new(false)),
            stderr_ring: Arc::new(Mutex::new(VecDeque::new())),
            audio: None,
            output_dir: PathBuf::new(),
            audio_args: Vec::new(),
            audio_single_track: false,
            container: "mp4".into(),
            faststart: false,
            duration_secs: 0,
            bitrate_kbps: 0,
            fps: 60,
            ffmpeg: PathBuf::new(),
            ring_cap: 16 * 1024 * 1024,
            plan: None,
            source_kind: "gfxcapture".into(),
            fallback_done: false,
            fallback_requested: false,
        }
    }

    /// Spawn `ffmpeg (capture -> encoder -> mpegts)` and wire the drain
    /// threads. The encoder process gets HIGH CPU priority, power throttling
    /// disabled and HIGH GPU scheduling priority, so a heavy game competing
    /// for the GPU does not starve the capture.
    fn spawn_encoder(&self, plan: &SpawnPlan, kind: &str) -> Result<Child, String> {
        let src = plan.source(kind);
        let filter = video::capture_filter(
            &plan.vendor,
            &plan.codec,
            &src,
            plan.monitor.width,
            plan.monitor.height,
            plan.out_height,
            plan.fps,
        );
        // Big kernel buffers for the encoder pipes. `Stdio::piped()` is a
        // 32 KB pipe (measured with PeekNamedPipe): at 20 Mbps that is ~13 ms
        // of slack, so any scheduling hiccup in our drains blocks ffmpeg
        // mid-capture and the CFR filter turns the gap into duplicated frames.
        // 8 MB ≈ 3 s of slack; stderr gets 1 MB (showinfo ~25 KB/s).
        let (ts_read, ts_write) = big_pipe(TS_PIPE_BYTES)?;
        let (err_read, err_write) = big_pipe(ERR_PIPE_BYTES)?;
        use std::os::windows::io::{FromRawHandle, IntoRawHandle};
        let ts_write_stdio = unsafe { Stdio::from_raw_handle(ts_write.into_raw_handle()) };
        let err_write_stdio = unsafe { Stdio::from_raw_handle(err_write.into_raw_handle()) };
        let mut cmd = Command::new(&plan.ffmpeg);
        // No `-progress`: it wrote ~600 lines/s to stderr and the TS index
        // already owns the save-time telemetry. stderr carries only the
        // showinfo frame clock (~60 lines/s).
        cmd.args([
            "-hide_banner",
            "-loglevel",
            "info",
            "-filter_complex",
            &filter,
            "-map",
            "[out]",
            "-an",
            "-c:v",
            plan.enc_name,
        ]);
        cmd.args(encode::live_encoder_args(
            plan.enc_name,
            &plan.codec,
            plan.bitrate_kbps,
            plan.fps,
            plan.out_height,
            plan.nvenc_hq,
            &plan.nvenc_preset,
            plan.encoder_full,
        ));
        // No `-flush_packets 1`: with an 8 MB pipe the per-packet flush buys
        // nothing and costs one syscall per packet.
        cmd.args([
            "-r",
            &plan.fps.to_string(),
            "-fps_mode",
            "cfr",
            "-muxdelay",
            "0",
            "-muxpreload",
            "0",
            "-f",
            "mpegts",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(ts_write_stdio)
        .stderr(err_write_stdio)
        .kill_on_drop(true);
        let child = cmd
            .spawn()
            .map_err(|e| format!("cannot launch capture encoder ({}): {e}", plan.ffmpeg.display()))?;
        // Our copies of the child's pipe write ends must go: keeping them open
        // would hide EOF when ffmpeg dies (the drains would never end).
        drop(cmd);
        tune_child_priority(&child);
        let stdout = ts_read;
        let stderr = err_read;
        // stderr drain: `-progress` key=value lines are parsed (telemetry),
        // `showinfo` frame lines feed the PTS<->QPC clock anchor, real errors
        // land in the bounded ring for `log_tail()`.
        // stderr drain. Hot-path rule: NEVER print per-frame lines — a blocked
        // console stalls this thread, the stderr pipe fills and ffmpeg blocks
        // mid-capture (that froze user clips under load). Only showinfo's
        // `pts_time` line is parsed (PTS<->QPC clock); every other showinfo
        // line is discarded; the bounded ring keeps the rest for `log_tail()`.
        let err_ring = self.stderr_ring.clone();
        let clock_ring = self.ring.clone();
        std::thread::Builder::new()
            .name("moonclip-ffmpeg-err".into())
            .spawn(move || {
                super::boost_current_thread();
                use std::io::BufRead;
                let mut reader = std::io::BufReader::new(stderr);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let trimmed = line.trim_end();
                            if trimmed.is_empty() {
                                continue;
                            }
                            // Pre-encoder frame clock sample (showinfo).
                            if trimmed.contains("Parsed_showinfo") {
                                if trimmed.contains(" pts_time:") {
                                    if let Some(pts) = parse_showinfo_pts(trimmed) {
                                        let qpc = pts::qpc_ns();
                                        if let Ok(mut ring) = clock_ring.lock() {
                                            ring.note_clock_sample(pts * 100, qpc);
                                        }
                                    }
                                }
                                continue;
                            }
                            let problem = [
                                "error", "Error", "failed", "Invalid", "Could not",
                                "warning", "Warning", "No such",
                            ]
                            .iter()
                            .any(|k| trimmed.contains(k));
                            if problem {
                                eprintln!("[moonclip] ffmpeg: {trimmed}");
                            }
                            if let Ok(mut ring) = err_ring.lock() {
                                ring.push_back(trimmed.to_string());
                                if ring.len() > 200 {
                                    ring.pop_front();
                                }
                            }
                        }
                    }
                }
            })
            .map_err(|e| format!("cannot spawn stderr drain: {e}"))?;
        // TS drain: encoder stdout -> indexed RAM ring. Never blocks ffmpeg:
        // reads always complete, the ring trims itself.
        let ring = self.ring.clone();
        let dead = self.video_dead.clone();
        let last_rx = self.last_rx_qpc.clone();
        let stall_logged = self.stall_logged.clone();
        std::thread::Builder::new()
            .name("moonclip-ts-drain".into())
            .spawn(move || {
                super::boost_current_thread();
                let mut out = stdout;
                let mut buf = [0u8; TS_READ_BUF];
                loop {
                    match out.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let qpc = pts::qpc_ns();
                            last_rx.store(qpc as i64, Ordering::Relaxed);
                            stall_logged.store(false, Ordering::Relaxed);
                            if let Ok(mut guard) = ring.lock() {
                                guard.push(&buf[..n], qpc);
                            }
                        }
                        Err(_) => break,
                    }
                }
                dead.store(true, Ordering::Relaxed);
            })
            .map_err(|e| format!("cannot spawn TS drain: {e}"))?;
        Ok(child)
    }

    /// Write the stems and mux the clip. `window_ns`/`qpc_start_ns` come from
    /// the cut; `ss_secs` > 0 only in the pre-roll fallback (no PSI parsed).
    ///
    /// Fast path (`ss_secs == 0`, the normal one): the cut TS goes to the
    /// muxer over **stdin** (no ~300 MB staging write+read) and only the two
    /// solo stems are WAVs — the Mix is `amix normalize=0` (the exact sample
    /// sum the Rust path used to write to a third WAV, ~92 MB of I/O saved).
    async fn deliver(
        &self,
        bytes: Vec<u8>,
        qpc_start_ns: i128,
        window_ns: i128,
        ss_secs: f64,
    ) -> Result<PathBuf, String> {
        let ffmpeg = self.ffmpeg.clone();
        let t_save = std::time::Instant::now();
        let samples = dsp::window_frames(window_ns);
        let t_snap = std::time::Instant::now();
        let (game, mic) = self
            .audio
            .as_ref()
            .map(|a| a.snapshot_window(qpc_start_ns, samples))
            .unwrap_or_default();
        let snap_elapsed = t_snap.elapsed();
        if let Some(a) = self.audio.as_ref() {
            let (ge, me) = a.stream_errors();
            if ge.is_some() || me.is_some() {
                eprintln!("[moonclip] audio stream errors: game={ge:?} mic={me:?}");
            }
            let (g, m) = a.peak_levels();
            eprintln!(
                "[moonclip] audio stats: game peak={} mic peak={} window={:.2}s",
                dbfs(g),
                dbfs(m),
                window_ns as f64 / 1e9
            );
        }
        let dest = mux::unique_dest(&self.output_dir, &self.container);
        let t_tmp = dest.with_extension("mux.tmp");
        let game_path = t_tmp.with_extension("game.wav");
        let mic_path = t_tmp.with_extension("mic.wav");
        // Only the legacy fallback needs a materialized Mix WAV; the fast path
        // lets ffmpeg sum the solos (`amix normalize=0`).
        let fallback_mix = if ss_secs > 0.001 {
            Some(mux::mix_i16(&game, &mic))
        } else {
            None
        };
        let t_wav = std::time::Instant::now();
        {
            let (gp, mp) = (game_path.clone(), mic_path.clone());
            let (gw, mw) = tokio::join!(
                tokio::task::spawn_blocking(move || mux::write_wav(&gp, &game)),
                tokio::task::spawn_blocking(move || mux::write_wav(&mp, &mic)),
            );
            gw.map_err(|e| format!("wav task failed: {e}"))?
                .map_err(|e| format!("cannot write game wav: {e}"))?;
            mw.map_err(|e| format!("wav task failed: {e}"))?
                .map_err(|e| format!("cannot write mic wav: {e}"))?;
        }
        let wav_elapsed = t_wav.elapsed();
        let single = self.audio_single_track;
        let t_mux = std::time::Instant::now();
        let res = match fallback_mix {
            Some(mix) => {
                // Legacy fallback: staged TS + per-input `-ss`; needs a Mix WAV.
                let mix_path = t_tmp.with_extension("mix.wav");
                mux::write_wav(&mix_path, &mix).map_err(|e| format!("cannot write mix wav: {e}"))?;
                let wavs: Vec<(PathBuf, &str)> = if single {
                    vec![(mix_path.clone(), "Mix")]
                } else {
                    vec![
                        (mix_path.clone(), "Mix"),
                        (game_path.clone(), "Game"),
                        (mic_path.clone(), "Mic"),
                    ]
                };
                let ts_path = std::env::temp_dir().join("moonclip-save-cut.ts");
                tokio::fs::write(&ts_path, &bytes)
                    .await
                    .map_err(|e| format!("cannot stage video window: {e}"))?;
                let r = mux::mux_clip(&ffmpeg, &ts_path, &wavs, &dest, ss_secs, single, self.faststart).await;
                let _ = tokio::fs::remove_file(&ts_path).await;
                let _ = tokio::fs::remove_file(&mix_path).await;
                r
            }
            None => {
                mux::mux_clip_pipe(
                    &ffmpeg,
                    bytes,
                    &game_path,
                    &mic_path,
                    single,
                    &dest,
                    &self.container,
                    self.faststart,
                )
                .await
            }
        };
        let _ = tokio::fs::remove_file(&game_path).await;
        let _ = tokio::fs::remove_file(&mic_path).await;
        res?;
        eprintln!(
            "[moonclip] mux: snapshot={snap_elapsed:?} wav={wav_elapsed:?} mux={:?}",
            t_mux.elapsed()
        );
        let t_patch = std::time::Instant::now();
        match mux::patch_audio_alternate_group(&dest, 1) {
            Ok(n) => eprintln!(
                "[moonclip] mp4: alternate_group=1 on {n} audio track(s) (patch {:?})",
                t_patch.elapsed()
            ),
            Err(e) => eprintln!("[moonclip] mp4: alternate_group patch skipped: {e}"),
        }
        eprintln!("[moonclip] save done in {:?}", t_save.elapsed());
        Ok(dest)
    }

    /// Fallback for codecs without a keyframe scanner (e.g. AV1): stage the
    /// tail by bytes, probe the first keyframe with ffmpeg and trim every
    /// input to it (the pre-rewrite path, kept as a safety net).
    async fn save_probe(&self) -> Result<PathBuf, String> {
        let ffmpeg = self.ffmpeg.clone();
        let keep = ((self.bitrate_kbps as usize
            * (self.duration_secs as usize + RING_MARGIN_SECS as usize))
            / 8)
            * 1024;
        let (cut, end_qpc, span_ns) = {
            let mut ring = self
                .ring
                .lock()
                .map_err(|_| "video ring poisoned".to_string())?;
            if ring.is_empty() {
                return Err("no video buffered yet".into());
            }
            ring.finalize();
            let end = ring
                .max_pts_ns()
                .ok_or_else(|| "no video timestamped yet".to_string())?;
            let calib = ring
                .calib_ns()
                .ok_or_else(|| "sync calibration pending".to_string())?;
            let bytes = ring.bytes_snapshot();
            let span = self.duration_secs as i64 * 1_000_000_000;
            let cut = cut_ts_window(&bytes, keep.max(1024 * 1024)).to_vec();
            (cut, calib + end as i128, span)
        };
        let ts_path = std::env::temp_dir().join("moonclip-save-cut.ts");
        tokio::fs::write(&ts_path, &cut)
            .await
            .map_err(|e| format!("cannot stage video window: {e}"))?;
        let video_ms = crate::editor::ffmpeg::probe_duration_ms(&ffmpeg, &ts_path)
            .await
            .unwrap_or(0);
        let window_ns: i128 = if video_ms > 0 {
            video_ms as i128 * 1_000_000
        } else {
            span_ns as i128
        };
        let window_ns = window_ns.min(span_ns as i128).max(1_000_000_000);
        let start_trim = sanitize_start_trim(
            first_keyframe_secs(&ffmpeg, &ts_path).await,
            window_ns as f64 / 1e9,
        );
        let qpc_start = end_qpc - window_ns;
        let samples = (window_ns * dsp::STEM_RATE as i128 / 1_000_000_000) as usize;
        let (game, mic) = self
            .audio
            .as_ref()
            .map(|a| a.snapshot_window(qpc_start, samples))
            .unwrap_or_default();
        let mix = mux::mix_i16(&game, &mic);
        let dest = mux::unique_dest(&self.output_dir, &self.container);
        // Legacy mux: every input is trimmed by the same keyframe time.
        let single = self.audio_single_track;
        let mut stems: Vec<(Vec<i16>, &str)> = if single {
            vec![(mix, mux::MIX_TITLE)]
        } else {
            vec![
                (mix, mux::MIX_TITLE),
                (game, mux::GAME_TITLE),
                (mic, mux::MIC_TITLE),
            ]
        };
        let tmp = dest.with_extension("mux.tmp");
        let mut wavs: Vec<(PathBuf, &str)> = Vec::new();
        for (i, (samples, title)) in stems.drain(..).enumerate() {
            let p = tmp.with_extension(format!("stem{i}.wav"));
            mux::write_wav(&p, &samples).map_err(|e| format!("cannot write {title} wav: {e}"))?;
            wavs.push((p, title));
        }
        let mut cmd = Command::new(&ffmpeg);
        cmd.args(["-y", "-hide_banner", "-loglevel", "error"]);
        if start_trim > 0.001 {
            cmd.args(["-ss", &format!("{:.3}", start_trim + 0.001)]);
        }
        cmd.arg("-i").arg(&ts_path);
        for (p, _) in &wavs {
            if start_trim > 0.001 {
                cmd.args(["-ss", &format!("{:.3}", start_trim + 0.001)]);
            }
            cmd.arg("-i").arg(p);
        }
        cmd.args(["-map", "0:v"]);
        for i in 0..wavs.len() {
            cmd.arg("-map").arg(format!("{}:a", i + 1));
        }
        cmd.args(["-c:v", "copy", "-c:a", "aac"]);
        for (i, (_, title)) in wavs.iter().enumerate() {
            cmd.args(["-b:a", if i == 2 { "192k" } else { "320k" }]);
            cmd.arg(format!("-metadata:s:a:{i}")).arg(format!("title={title}"));
        }
        if self.faststart {
            cmd.args(["-movflags", "+faststart"]);
        }
        cmd.arg("-shortest");
        let out = cmd
            .arg(&dest)
            .output()
            .await
            .map_err(|e| format!("save mux failed: {e}"))?;
        for (p, _) in &wavs {
            let _ = tokio::fs::remove_file(p).await;
        }
        let _ = tokio::fs::remove_file(&ts_path).await;
        if !out.status.success() {
            return Err(format!(
                "save mux failed (ffmpeg): {}",
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .last()
                    .unwrap_or("unknown")
            ));
        }
        match mux::patch_audio_alternate_group(&dest, 1) {
            Ok(n) => eprintln!("[moonclip] mp4: alternate_group=1 on {n} audio track(s)"),
            Err(e) => eprintln!("[moonclip] mp4: alternate_group patch skipped: {e}"),
        }
        Ok(dest)
    }

    /// Wait until the ring indexes its first frame, the encoder exits, or the
    /// proof deadline passes. Returns `(frames_seen, exit_error)`.
    async fn wait_first_frames(&mut self) -> (usize, Option<String>) {
        let deadline = std::time::Instant::now() + Duration::from_millis(FRAME_PROOF_MS);
        loop {
            if let Some(child) = self.child.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    let err = self
                        .stderr_ring
                        .lock()
                        .map(|ring| {
                            ring.iter()
                                .rev()
                                .take(5)
                                .rev()
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(" | ")
                        })
                        .unwrap_or_default();
                    self.child = None;
                    return (0, Some(format!("capture encoder exited ({status}): {err}")));
                }
            }
            let frames = self.ring.lock().map(|r| r.frame_count()).unwrap_or(0);
            if frames > 0 || std::time::Instant::now() >= deadline {
                return (frames, None);
            }
            tokio::time::sleep(Duration::from_millis(FRAME_PROOF_POLL_MS)).await;
        }
    }

    /// Kill the running child (if any), reset the ring/telemetry and respawn
    /// with `kind`. Used at start and by the source fallback.
    async fn relaunch_source(&mut self, kind: &str) -> Result<usize, String> {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        if let Ok(mut ring) = self.ring.lock() {
            ring.clear();
        }
        if let Ok(mut r) = self.stderr_ring.lock() {
            r.clear();
        }
        self.video_dead.store(false, Ordering::Relaxed);
        self.last_rx_qpc.store(pts::qpc_ns() as i64, Ordering::Relaxed);
        self.stall_logged.store(false, Ordering::Relaxed);
        let child = self.spawn_encoder(self.plan.as_ref().expect("spawn plan"), kind)?;
        self.child = Some(child);
        self.source_kind = kind.to_string();
        self.fallback_done = kind == "ddagrab";
        let (frames, exit_err) = self.wait_first_frames().await;
        if let Some(e) = exit_err {
            return Err(e);
        }
        Ok(frames)
    }

    /// Decode a few frames from the freshest ring window and report whether it
    /// is (near) all-black: WGC cannot see a true legacy exclusive-fullscreen
    /// swapchain and then emits black frames, while Desktop Duplication owns
    /// the display. `None` when the probe cannot run (no keyframe yet).
    async fn probe_black(&self) -> Option<bool> {
        let cut = {
            let mut ring = self.ring.lock().ok()?;
            ring.finalize();
            ring.cut(1_500_000_000, 0)?
        };
        let bytes = cut.bytes;
        let path = std::env::temp_dir().join(format!(
            "moonclip-black-probe-{}.ts",
            std::process::id()
        ));
        if tokio::fs::write(&path, &bytes).await.is_err() {
            return None;
        }
        let out = Command::new(&self.ffmpeg)
            .args(["-hide_banner", "-loglevel", "info", "-i"])
            .arg(&path)
            .args([
                "-vf",
                "signalstats,metadata=print",
                "-frames:v",
                "4",
                "-f",
                "null",
                "-",
            ])
            .output()
            .await
            .ok();
        let _ = tokio::fs::remove_file(&path).await;
        let out = out?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        let mut max_yavg = 0.0f64;
        let mut seen = false;
        for line in stderr.lines() {
            if let Some(rest) = line.split("YAVG=").nth(1) {
                if let Some(v) = rest
                    .split_whitespace()
                    .next()
                    .and_then(|x| x.parse::<f64>().ok())
                {
                    seen = true;
                    max_yavg = max_yavg.max(v);
                }
            }
        }
        if seen {
            Some(max_yavg < BLACK_YAVG)
        } else {
            None
        }
    }
}

/// Anonymous pipe with a real kernel buffer and an inheritable write end only.
/// `Stdio::piped()` is a 32 KB pipe (measured): ~13 ms of slack at 20 Mbps, so
/// any scheduling hiccup in our drain threads blocks ffmpeg mid-capture and
/// the CFR filter turns the gap into duplicated frames. 8 MB ≈ 3 s of slack.
fn big_pipe(bytes: u32) -> Result<(std::fs::File, std::os::windows::io::OwnedHandle), String> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::SECURITY_ATTRIBUTES;
    use windows::Win32::System::Pipes::CreatePipe;
    unsafe {
        let mut read = HANDLE(std::ptr::null_mut());
        let mut write = HANDLE(std::ptr::null_mut());
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: true.into(),
        };
        CreatePipe(&mut read, &mut write, Some(&sa), bytes)
            .map_err(|e| format!("cannot create encoder pipe: {e}"))?;
        // Only the write end may be inherited: an inherited read end in the
        // child would keep the pipe alive and hide EOF when ffmpeg dies.
        let _ = windows::Win32::Foundation::SetHandleInformation(
            read,
            1u32,
            windows::Win32::Foundation::HANDLE_FLAGS(0),
        );
        Ok((
            std::fs::File::from_raw_handle(read.0),
            OwnedHandle::from_raw_handle(write.0),
        ))
    }
}

/// Best-effort process priority + power throttling for the encoder child.
///
/// CPU class and power throttling land immediately. The GPU scheduling class
/// cannot: dxgkrnl rejects `D3DKMTSetProcessSchedulingPriorityClass` with
/// `STATUS_INVALID_PARAMETER` while the target process has no live GPU/display
/// context (measured: fails right after spawn; the same call succeeds once
/// ffmpeg's D3D11/NVENC device is up, and never succeeds on GPU-less
/// processes like ping). So it is retried in the background until it lands.
fn tune_child_priority(child: &Child) {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Threading::{
        ProcessPowerThrottling, SetPriorityClass, SetProcessInformation,
        PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        PROCESS_POWER_THROTTLING_STATE,
    };
    let Some(raw) = child.raw_handle() else { return };
    let h = HANDLE(raw);
    unsafe {
        // CPU class HIGH by default (games run at HIGH; ABOVE_NORMAL loses the
        // CPU under a saturated game). `MOONCLIP_CAPTURE_CPU_PRIO=0` restores
        // the old ABOVE_NORMAL, `normal` drops to NORMAL.
        let class = cpu_priority_class(std::env::var("MOONCLIP_CAPTURE_CPU_PRIO").ok().as_deref());
        let _ = SetPriorityClass(h, class);
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: 0, // execution speed throttling disabled
        };
        let _ = SetProcessInformation(
            h,
            ProcessPowerThrottling,
            &state as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
    }
    // GPU scheduler priority HIGH by default (`gpu_priority_level`): without
    // it the WGC/NVENC chain starves behind a GPU-saturating game and the
    // clip fills with duplicated frames. `MOONCLIP_CAPTURE_GPU_PRIO=0`
    // disables it if a game ever shows contention stutter.
    if let Some(level) = gpu_priority_level(std::env::var("MOONCLIP_CAPTURE_GPU_PRIO").ok().as_deref())
    {
        let mut got: i32 = -1;
        let set = unsafe { D3DKMTSetProcessSchedulingPriorityClass(h, level) };
        let read = unsafe { D3DKMTGetProcessSchedulingPriorityClass(h, &mut got) };
        if set == 0 {
            eprintln!(
                "[moonclip] encoder GPU scheduling priority: {level} applied immediately (read_back={got}, get_status={read})"
            );
        } else {
            if let Some(pid) = child.id() {
                retry_gpu_priority(pid, level);
            }
        }
    }
}

/// Retry the GPU scheduling class until ffmpeg has a GPU context (or exits).
/// Opens its own handle per attempt: the child's raw handle may be closed by
/// the engine at any moment, and reusing a closed numeric handle is unsafe.
fn retry_gpu_priority(pid: u32, level: i32) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    };
    const ATTEMPTS: u32 = 40; // ~10 s at 250 ms
    std::thread::Builder::new()
        .name("moonclip-gpu-prio".into())
        .spawn(move || {
            for attempt in 1..=ATTEMPTS {
                std::thread::sleep(Duration::from_millis(250));
                let Ok(h) = (unsafe {
                    OpenProcess(
                        PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
                        false,
                        pid,
                    )
                }) else {
                    return; // child exited: nothing to raise
                };
                let mut got: i32 = -1;
                let set = unsafe { D3DKMTSetProcessSchedulingPriorityClass(h, level) };
                let read = unsafe { D3DKMTGetProcessSchedulingPriorityClass(h, &mut got) };
                let _ = unsafe { CloseHandle(h) };
                if set == 0 {
                    eprintln!(
                        "[moonclip] encoder GPU scheduling priority: {level} applied after {}ms (read_back={got}, get_status={read})",
                        attempt * 250
                    );
                    return;
                }
                if attempt == ATTEMPTS {
                    eprintln!(
                        "[moonclip] encoder GPU scheduling priority: not applied after {}s (set_status={set:#X}); capture continues",
                        ATTEMPTS / 4
                    );
                }
            }
        })
        .ok();
}

// gdi32 exports, declared locally to avoid pulling the WDK feature surface
// (D3DKMT_SCHEDULINGPRIORITYCLASS_HIGH = 4).
#[link(name = "gdi32")]
extern "system" {
    fn D3DKMTSetProcessSchedulingPriorityClass(
        handle: windows::Win32::Foundation::HANDLE,
        priority: i32,
    ) -> i32;
    fn D3DKMTGetProcessSchedulingPriorityClass(
        handle: windows::Win32::Foundation::HANDLE,
        priority: *mut i32,
    ) -> i32;
}

impl CaptureEngine for WindowsCaptureEngine {
    async fn start_buffer(&mut self, config: CaptureConfig) -> Result<(), String> {
        if self.child.is_some() {
            return Err("recorder already running".into());
        }
        std::fs::create_dir_all(&config.output_dir)
            .map_err(|e| format!("cannot create clips dir: {e}"))?;
        let vendor = video::vendor(Path::new("")).await;
        let enc_name = video::capture_encoder_name(&vendor, &config.codec).ok_or_else(|| {
            format!(
                "codec '{}' cannot encode on '{}' (stale setting?)",
                config.codec, vendor
            )
        })?;
        let monitor = video::resolve_monitor(&config.source)
            .await
            .ok_or_else(|| "no monitor available (Windows 10 1903+ required)".to_string())?;
        let fps = if config.fps == 30 { 30 } else { 60 };
        // With live GPU scaling the buffer runs at the DELIVERED height:
        // `save_height` is the user's choice when the old plan asked for a
        // source-resolution buffer + save transcode. No save transcode here.
        let out_height = if config.save_height > 0 {
            config.save_height
        } else {
            config.out_height
        };
        // Bitrate: the caller pairs `save_bitrate_kbps` with `save_height`.
        let bitrate_kbps = if config.save_bitrate_kbps > 0 && out_height == config.save_height {
            config.save_bitrate_kbps
        } else {
            config.bitrate_kbps
        };
        // A/B override (`MOONCLIP_CAPTURE_BITRATE_KBPS`, e.g. 10000): swaps
        // the CBR target without touching the shared Medal ladder. The ring
        // capacity follows it.
        let bitrate_kbps = encode::capture_bitrate_override(
            std::env::var("MOONCLIP_CAPTURE_BITRATE_KBPS").ok().as_deref(),
        )
        .unwrap_or(bitrate_kbps);
        self.ring_cap = (((bitrate_kbps as usize) * ((config.duration_seconds as usize) + RING_MARGIN_SECS as usize))
            / 8)
            * 1024;
        self.ring_cap = self.ring_cap.max(16 * 1024 * 1024);
        if let Ok(mut ring) = self.ring.lock() {
            *ring = TsRing::new(self.ring_cap, fps);
        }
        self.video_dead.store(false, Ordering::Relaxed);
        self.last_rx_qpc.store(pts::qpc_ns() as i64, Ordering::Relaxed);
        self.stall_logged.store(false, Ordering::Relaxed);
        if let Ok(mut r) = self.stderr_ring.lock() {
            r.clear();
        }
        let ffmpeg = config
            .ffmpeg_bin
            .clone()
            .unwrap_or_else(super::video::capture_ffmpeg);
        // A force-killed session leaves its ffmpeg capturing + encoding (the
        // Drop guard never runs); stacked leftovers are a GPU/NVENC tax that
        // only shows under game load. Sweep ours before spawning a fresh one.
        super::kill_orphan_ffmpeg(&ffmpeg);
        let nvenc_hq = config.nvenc_opts.is_some();
        let nvenc_preset =
            encode::nvenc_preset(std::env::var("MOONCLIP_NVENC_PRESET").ok().as_deref());
        let encoder_full =
            encode::encoder_hq_full(std::env::var("MOONCLIP_ENCODER_HQ_FULL").ok().as_deref());
        // Capture source. Default: WGC monitor (`gfxcapture`). Opt-in A/B:
        //   MOONCLIP_CAPTURE_SOURCE=ddagrab  -> Desktop Duplication (monitor)
        //   MOONCLIP_CAPTURE_SOURCE=window   -> foreground window (WGC hwnd)
        //   MOONCLIP_CAPTURE_WINDOW_EXE=^cod.exe$ -> game window by regex
        // `source_override` comes from the mid-session fallback path and wins
        // over the environment.
        let env_source = config
            .source_override
            .clone()
            .unwrap_or_else(|| std::env::var("MOONCLIP_CAPTURE_SOURCE").unwrap_or_default());
        let env_window_exe = std::env::var("MOONCLIP_CAPTURE_WINDOW_EXE")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        // Max capture rate = 2x the output rate (see `capture_max_fps`), never
        // the full refresh: under a game, full-refresh WGC copies + filtering
        // steal GPU time from NVENC and the CFR filter then discards those
        // frames anyway. ffmpeg owns the exact CFR with `-r fps -fps_mode cfr`.
        let max_fps = std::env::var("MOONCLIP_CAPTURE_MAX_FPS")
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .map(|v| v.max(fps))
            .or_else(|| (config.capture_max_fps > 0).then_some(config.capture_max_fps.max(fps)))
            .unwrap_or_else(|| video::capture_max_fps(monitor.refresh_hz, fps));
        let initial_kind = if env_source == "ddagrab" { "ddagrab" } else { "gfxcapture" };
        self.ffmpeg = ffmpeg.clone();
        self.plan = Some(SpawnPlan {
            ffmpeg,
            vendor,
            codec: config.codec.clone(),
            enc_name,
            monitor,
            out_height,
            bitrate_kbps,
            fps,
            nvenc_hq,
            nvenc_preset,
            encoder_full,
            env_source,
            env_window_exe,
            max_fps,
        });
        // Prove the chain before committing: a bad combo (missing HW block,
        // unsupported filter option) exits within milliseconds, and indexed
        // frames prove capture+encode+mux actually flow. A source that yields
        // no frames — or an all-black picture on WGC (legacy exclusive
        // fullscreen bypasses DWM) — is respawned once with DXGI Duplication.
        let mut frames = self.relaunch_source(initial_kind).await?;
        let black = if frames > 0 { self.probe_black().await } else { None };
        if let Some(next) = video::source_fallback(self.source_kind.as_str(), frames > 0, black) {
            eprintln!(
                "[moonclip] source fallback: {} unusable (frames={frames}, black={black:?}); respawning with {next}",
                self.source_kind
            );
            frames = self.relaunch_source(next).await?;
        }
        if frames == 0 {
            eprintln!(
                "[moonclip] warning: no video frames indexed after {FRAME_PROOF_MS}ms (static screen or uncapturable source)"
            );
        }
        // Audio: loopback + mic. Gains are applied right after start by the
        // settings task (same as Linux).
        match AudioCapture::start(
            &config.desktop_device,
            &config.mic_device,
            config.duration_seconds + RING_MARGIN_SECS as u32,
            100,
            100,
            false,
            false,
        ) {
            Ok(a) => {
                if a.live_count() < 2 {
                    eprintln!("[moonclip] audio degraded: {}/2 streams live", a.live_count());
                }
                self.audio = Some(a);
            }
            Err(e) => {
                eprintln!("[moonclip] audio failed, aborting start: {e}");
                let _ = self.stop_buffer().await;
                return Err(e);
            }
        }
        self.output_dir = config.output_dir;
        self.audio_args = vec![
            format!("{}|{}", config.desktop_device, config.mic_device),
            config.desktop_device.clone(),
            config.mic_device.clone(),
        ];
        self.audio_single_track = config.audio_single_track;
        self.container = if config.container == "mkv" {
            "mkv".into()
        } else {
            "mp4".into()
        };
        self.faststart = config.faststart;
        self.duration_secs = config.duration_seconds;
        self.bitrate_kbps = bitrate_kbps;
        self.fps = fps;
        {
            let kind = self.source_kind.clone();
            let p = self.plan.as_ref().expect("spawn plan");
            eprintln!(
                "[moonclip] capture buffer: {}x{}@{} ({}, max {}) {} ({}) -> {}p{}kbps, audio {}/2",
                p.monitor.width,
                p.monitor.height,
                p.fps,
                p.source(&kind).describe(),
                p.max_fps,
                p.enc_name,
                p.vendor,
                p.out_height,
                p.bitrate_kbps,
                self.audio.as_ref().map(|a| a.live_count()).unwrap_or(0)
            );
        }
        Ok(())
    }

    async fn save_clip(&mut self) -> Result<PathBuf, String> {
        if self.child.is_none() {
            return Err("recorder not running".into());
        }
        if self.video_dead.load(Ordering::Relaxed) {
            let frames = self.ring.lock().map(|r| r.frame_count()).unwrap_or(0);
            if frames == 0 {
                return Err("video encoder died and no footage is buffered".into());
            }
        }
        let t_save = std::time::Instant::now();
        let span = self.duration_secs as i64 * 1_000_000_000;
        let cut: Option<CutPlan> = {
            let mut ring = self
                .ring
                .lock()
                .map_err(|_| "video ring poisoned".to_string())?;
            if ring.is_empty() {
                return Err("no video buffered yet".into());
            }
            ring.finalize();
            // Snapshot only: the ~300 MB copy runs after the lock is released
            // (`assemble` below), so the encoder pipe never stalls behind a save.
            ring.cut_plan(span, SAVE_MIN_CLIP_NS)
        };
        let Some(cut) = cut else {
            // No scanner for this codec (AV1) or a too-short ring: probe path.
            let (frames, keys, calib) = self
                .ring
                .lock()
                .map(|r| (r.frame_count(), r.key_count(), r.calib_ns()))
                .unwrap_or((0, 0, None));
            eprintln!(
                "[moonclip] save: indexed cut unavailable (frames={frames} keys={keys} calib={calib:?}), using probe fallback"
            );
            return self.save_probe().await;
        };
        eprintln!(
            "[moonclip] save: key={:.2}s span={:.2}s clip={:.2}s frames={}",
            cut.key_pts_ns as f64 / 1e9,
            (cut.end_pts_ns - cut.key_pts_ns) as f64 / 1e9,
            cut.window_ns as f64 / 1e9,
            self.ring.lock().map(|r| r.frame_count()).unwrap_or(0),
        );
        // Real capture rate from the pre-encoder clock samples: the number to
        // watch under game load (PES counts include ffmpeg's CFR duplicates).
        let (crate_frames, crate_span, crate_fps) = self
            .ring
            .lock()
            .map(|r| r.capture_rate())
            .unwrap_or((0, 0.0, 0.0));
        eprintln!(
            "[moonclip] capture: {crate_frames} real frames in {crate_span:.1}s = {crate_fps:.1} fps"
        );
        let now = pts::qpc_ns();
        let end_qpc = cut.qpc_start_ns + cut.window_ns;
        let (g, m) = self
            .audio
            .as_ref()
            .map(|a| a.delivery_qpc())
            .unwrap_or((0, 0));
        let lag_ms = (now - end_qpc) as f64 / 1e6;
        eprintln!(
            "[moonclip] sync: key_qpc={:.3}s end_qpc={:.3}s video_lag={:.0}ms audio_end game={:.0}ms mic={:.0}ms calib={:.3}s",
            cut.qpc_start_ns as f64 / 1e9,
            end_qpc as f64 / 1e9,
            lag_ms,
            (g as i128 - end_qpc) as f64 / 1e6,
            (m as i128 - end_qpc) as f64 / 1e6,
            self.ring
                .lock()
                .map(|r| r.calib_ns().map(|c| c as f64 / 1e9).unwrap_or(0.0))
                .unwrap_or(0.0),
        );
        // SPEC §9: sustained encoder lag with a slow preset means the GPU is
        // saturated; recommend the documented P4 Medium step-down (applied by
        // the settings path with a restart notice, never silently).
        if let Some(next) = self
            .plan
            .as_ref()
            .and_then(|p| encode::preset_step_down(&p.nvenc_preset, lag_ms))
        {
            eprintln!(
                "[moonclip] encoder lag {lag_ms:.0}ms > {:.0}ms: preset step-down {next} recommended",
                encode::LAG_STEP_DOWN_MS
            );
        }
        let bytes = cut.assemble();
        let bytes_len = bytes.len();
        let dest = self
            .deliver(bytes, cut.qpc_start_ns, cut.window_ns, cut.ss_secs)
            .await?;
        eprintln!(
            "[moonclip] engine save done in {:?} ({} MB staged)",
            t_save.elapsed(),
            bytes_len / 1024 / 1024
        );
        Ok(dest)
    }

    async fn stop_buffer(&mut self) -> Result<(), String> {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
        }
        self.audio = None;
        self.audio_args.clear();
        if let Ok(mut ring) = self.ring.lock() {
            ring.clear();
        }
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "gfxcapture"
    }

    fn audio_args(&self) -> Vec<String> {
        self.audio_args.clone()
    }

    /// Live scaling + copy-only saves: no save-time transcode on Windows.
    fn save_plan(&self) -> Option<SavePlan> {
        None
    }

    /// A closed encoder pipe (`video_dead`) or an exited child is a dead
    /// engine. Source stalls (no fresh PES for `STALL_AFTER`) keep the engine
    /// alive but are surfaced once in `log_tail`.
    fn check_alive(&mut self) -> bool {
        if self.video_dead.load(Ordering::Relaxed) {
            return false;
        }
        let alive = match self.child.as_mut() {
            Some(child) => !matches!(child.try_wait(), Ok(Some(_))),
            None => false,
        };
        if alive {
            let last = self.last_rx_qpc.load(Ordering::Relaxed);
            let age_ns = pts::qpc_ns() - last as i128;
            let frames = self.ring.lock().map(|r| r.frame_count()).unwrap_or(0);
            if frames > 0
                && age_ns > STALL_AFTER.as_nanos() as i128
                && !self.stall_logged.swap(true, Ordering::Relaxed)
            {
                eprintln!(
                    "[moonclip] video source stalled ({:.1}s without new frames; exclusive fullscreen?)",
                    age_ns as f64 / 1e9
                );
                // Mid-session degradation (WGC lost DWM, e.g. windowed ->
                // legacy FSE): surface a one-shot source-fallback request for
                // the owner to respawn with DXGI.
                if self.source_kind == "gfxcapture" && !self.fallback_done {
                    self.fallback_requested = true;
                }
            }
        }
        alive
    }

    fn log_tail(&self) -> Vec<String> {
        let mut out = self
            .stderr_ring
            .lock()
            .map(|ring| ring.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        if self.stall_logged.load(Ordering::Relaxed) {
            out.push("video source stalled: no new frames (exclusive fullscreen?)".into());
        }
        out
    }

    fn source_fallback_request(&mut self) -> Option<String> {
        if self.fallback_requested && !self.fallback_done {
            self.fallback_requested = false;
            Some("ddagrab".into())
        } else {
            None
        }
    }
}

impl Drop for WindowsCaptureEngine {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio_track_headers(path: &std::path::Path) -> Vec<(u8, u32, u16)> {
        let bytes = std::fs::read(path).unwrap();
        let (_, start, len) = super::mux::box_children(&bytes, 0, bytes.len())
            .into_iter()
            .find(|(t, _, _)| t == b"moov")
            .expect("moov box");
        let moov = &bytes[start..start + len];
        let mut out = Vec::new();
        for (t, p, l) in super::mux::box_children(moov, 0, moov.len()) {
            if &t != b"trak" {
                continue;
            }
            let mut tkhd = None;
            let mut audio = false;
            for (t2, p2, l2) in super::mux::box_children(moov, p, l) {
                if &t2 == b"tkhd" {
                    tkhd = Some(p2);
                } else if &t2 == b"mdia" {
                    for (t3, p3, l3) in super::mux::box_children(moov, p2, l2) {
                        if &t3 == b"hdlr" && l3 >= 12 && &moov[p3 + 8..p3 + 12] == b"soun" {
                            audio = true;
                        }
                    }
                }
            }
            if !audio {
                continue;
            }
            let Some(t) = tkhd else { continue };
            let ver = moov[t];
            let flags = u32::from_be_bytes([0, moov[t + 1], moov[t + 2], moov[t + 3]]);
            let rel = super::mux::tkhd_alternate_group_off(ver).expect("tkhd version");
            let alt = u16::from_be_bytes([moov[t + rel], moov[t + rel + 1]]);
            out.push((ver, flags, alt));
        }
        out
    }

    #[test]
    fn gpu_priority_defaults_high_with_env_overrides() {        assert_eq!(gpu_priority_level(None), Some(4));
        assert_eq!(gpu_priority_level(Some("")), Some(4));
        assert_eq!(gpu_priority_level(Some("0")), None);
        assert_eq!(gpu_priority_level(Some("off")), None);
        assert_eq!(gpu_priority_level(Some("false")), None);
        // Legacy opt-in value stays a HIGH alias.
        assert_eq!(gpu_priority_level(Some("1")), Some(4));
        assert_eq!(gpu_priority_level(Some("3")), Some(3));
        assert_eq!(gpu_priority_level(Some("9")), None);
        assert_eq!(gpu_priority_level(Some("junk")), None);
    }

    #[test]
    fn cpu_priority_defaults_high_with_env_overrides() {
        use windows::Win32::System::Threading::{
            ABOVE_NORMAL_PRIORITY_CLASS, HIGH_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
        };
        assert_eq!(cpu_priority_class(None).0, HIGH_PRIORITY_CLASS.0);
        assert_eq!(cpu_priority_class(Some("")).0, HIGH_PRIORITY_CLASS.0);
        assert_eq!(cpu_priority_class(Some("0")).0, ABOVE_NORMAL_PRIORITY_CLASS.0);
        assert_eq!(cpu_priority_class(Some("above")).0, ABOVE_NORMAL_PRIORITY_CLASS.0);
        assert_eq!(cpu_priority_class(Some("normal")).0, NORMAL_PRIORITY_CLASS.0);
        assert_eq!(cpu_priority_class(Some("junk")).0, HIGH_PRIORITY_CLASS.0);
    }

    #[test]
    fn pts_time_parse() {
        let line = "[Parsed_showinfo_0 @ 000001] n:0 pts:141000 pts_time:1.566667 duration:1500 fmt:yuv420p";
        assert_eq!(parse_pts_time(line), Some(1.566667));
        assert_eq!(parse_pts_time("no pts here"), None);
    }

    #[test]
    fn start_trim_sanitized() {
        assert_eq!(sanitize_start_trim(Some(1.5), 31.0), 1.5);
        assert_eq!(sanitize_start_trim(Some(0.01), 31.0), 0.0);
        assert_eq!(sanitize_start_trim(Some(4.9), 31.0), 0.0);
        assert_eq!(sanitize_start_trim(None, 31.0), 0.0);
    }

    #[test]
    fn ts_cut_resyncs() {
        let mut ring = vec![0xAAu8; 100];
        for _ in 0..4 {
            ring.push(0x47);
            ring.extend(std::iter::repeat(0x00).take(187));
        }
        let cut = cut_ts_window(&ring, 600);
        assert_eq!(cut[0], 0x47);
        assert_eq!(cut[188], 0x47);
        assert!(cut.len() <= 600);
        let plain = vec![0x11u8; 500];
        assert_eq!(cut_ts_window(&plain, 600).len(), 500);
    }

    // -----------------------------------------------------------------------
    // Live tests (ignored by default: need a real desktop + NVENC + WASAPI).
    // Run: cargo test --target x86_64-pc-windows-msvc live_ -- --ignored
    // -----------------------------------------------------------------------

    fn cfg(dir: PathBuf, duration: u32, fps: u32) -> CaptureConfig {
        CaptureConfig {
            duration_seconds: duration,
            fps,
            output_dir: dir,
            gsr_bin: None,
            desktop_device: String::new(),
            mic_device: String::new(),
            source: String::new(),
            codec: "h264".into(),
            out_height: 0,
            bitrate_kbps: 20000,
            save_height: 0,
            save_bitrate_kbps: 20000,
            save_encoder: Some(super::super::super::TranscodeEncoder::Nvenc),
            nvenc_opts: None,
            ffmpeg_bin: None,
            audio_single_track: false,
            source_override: None,
            capture_max_fps: 0,
            faststart: false,
            container: "mp4".into(),
        }
    }

    /// Buffers N seconds, saves, and asserts h264 video + 3×AAC, keyframe
    /// start (no late video), 3 audio tracks in alternate group 1.
    #[tokio::test]
    #[ignore]
    async fn live_buffer_and_save() {
        use super::super::super::CaptureEngine;

        let dir = std::env::temp_dir().join("moonclip-e2e");
        std::fs::create_dir_all(&dir).unwrap();
        let mut eng = super::WindowsCaptureEngine::new();
        eng.start_buffer(cfg(dir, 10, 60)).await.expect("start");
        // Make the desktop change so WGC delivers frames.
        tokio::time::sleep(std::time::Duration::from_secs(6)).await;
        let path = eng.save_clip().await.expect("save");
        assert!(path.exists(), "clip missing");
        let size = std::fs::metadata(&path).unwrap().len();
        assert!(size > 50_000, "clip suspiciously small: {size} B");
        let probe = tokio::process::Command::new(super::super::video::capture_ffmpeg())
            .args(["-hide_banner", "-i", &path.to_string_lossy()])
            .output()
            .await
            .expect("probe");
        let err = String::from_utf8_lossy(&probe.stderr);
        assert!(err.contains("Video: h264"), "no h264 video:\n{err}");
        assert_eq!(err.matches("Audio: aac").count(), 3, "want 3xAAC:\n{err}");
        let video_line = err
            .lines()
            .find(|l| l.contains("Video: h264"))
            .unwrap_or_default();
        assert!(
            !video_line.contains("start "),
            "video track starts late (keyframe cut broken):\n{video_line}"
        );
        let tracks = audio_track_headers(&path);
        assert_eq!(tracks.len(), 3, "audio tracks: {tracks:?}");
        assert!(tracks.iter().all(|(_, _, alt)| *alt == 1), "{tracks:?}");
        let dur_ms = crate::editor::ffmpeg::probe_duration_ms(
            &super::super::video::capture_ffmpeg(),
            &path,
        )
        .await
        .expect("duration");
        // 6 s run, 10 s buffer: the clip is ~6 s (latest keyframe ≤ target).
        assert!(
            (4000..=9500).contains(&dur_ms),
            "unexpected duration: {dur_ms} ms"
        );
        eng.stop_buffer().await.expect("stop");
        let _ = std::fs::remove_file(&path);
    }

    /// Compatibility mode: the clip carries a single audio track (the Mix).
    #[tokio::test]
    #[ignore]
    async fn live_buffer_and_save_single_track() {
        use super::super::super::CaptureEngine;

        let dir = std::env::temp_dir().join("moonclip-e2e-one");
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = cfg(dir, 6, 60);
        c.audio_single_track = true;
        let mut eng = super::WindowsCaptureEngine::new();
        eng.start_buffer(c).await.expect("start");
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        let path = eng.save_clip().await.expect("save");
        let probe = tokio::process::Command::new(super::super::video::capture_ffmpeg())
            .args(["-hide_banner", "-i", &path.to_string_lossy()])
            .output()
            .await
            .expect("probe");
        let err = String::from_utf8_lossy(&probe.stderr);
        assert_eq!(err.matches("Audio: aac").count(), 1, "want 1xAAC:\n{err}");
        let tracks = audio_track_headers(&path);
        assert_eq!(tracks.len(), 1, "{tracks:?}");
        assert_eq!(tracks[0].2, 1, "alternate_group missing: {tracks:?}");
        eng.stop_buffer().await.expect("stop");
        let _ = std::fs::remove_file(&path);
    }

    /// Source override (the settings mid-session fallback path): force Desktop
    /// Duplication and prove capture + save flow.
    #[tokio::test]
    #[ignore]
    async fn live_source_override_ddagrab() {
        use super::super::super::CaptureEngine;

        let dir = std::env::temp_dir().join("moonclip-e2e-dda");
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = cfg(dir, 6, 60);
        c.source_override = Some("ddagrab".into());
        let mut eng = super::WindowsCaptureEngine::new();
        eng.start_buffer(c).await.expect("start");
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        let path = eng.save_clip().await.expect("save");
        let probe = tokio::process::Command::new(super::super::video::capture_ffmpeg())
            .args(["-hide_banner", "-i", &path.to_string_lossy()])
            .output()
            .await
            .expect("probe");
        let err = String::from_utf8_lossy(&probe.stderr);
        assert!(err.contains("Video: h264"), "no h264 video:\n{err}");
        assert_eq!(err.matches("Audio: aac").count(), 3, "want 3xAAC:\n{err}");
        eng.stop_buffer().await.expect("stop");
        let _ = std::fs::remove_file(&path);
    }

    /// CPU (x264) fallback smoke: the buffer must save h264 + 3×AAC.
    #[tokio::test]
    #[ignore]
    async fn live_buffer_and_save_x264() {
        use super::super::super::CaptureEngine;

        let dir = std::env::temp_dir().join("moonclip-e2e-x264");
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = cfg(dir, 6, 30);
        c.codec = "x264".into();
        c.nvenc_opts = None;
        let mut eng = super::WindowsCaptureEngine::new();
        eng.start_buffer(c).await.expect("start");
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let path = eng.save_clip().await.expect("save");
        let probe = tokio::process::Command::new(super::super::video::capture_ffmpeg())
            .args(["-hide_banner", "-i", &path.to_string_lossy()])
            .output()
            .await
            .expect("probe");
        let err = String::from_utf8_lossy(&probe.stderr);
        assert!(err.contains("Video: h264"), "no h264 video:\n{err}");
        assert_eq!(err.matches("Audio: aac").count(), 3, "want 3xAAC:\n{err}");
        eng.stop_buffer().await.expect("stop");
        let _ = std::fs::remove_file(&path);
    }

    /// Gold save test (SPEC §14): a full 120 s buffer must save copy-only in
    /// under 3 s. Keep the screen changing (mouse-mover rig in PROGRESS).
    #[tokio::test]
    #[ignore]
    async fn live_save_120s_timing() {
        use super::super::super::CaptureEngine;

        let dir = std::env::temp_dir().join("moonclip-e2e-120");
        std::fs::create_dir_all(&dir).unwrap();
        let mut eng = super::WindowsCaptureEngine::new();
        eng.start_buffer(cfg(dir, 120, 60)).await.expect("start");
        tokio::time::sleep(std::time::Duration::from_secs(125)).await;
        let t0 = std::time::Instant::now();
        let path = eng.save_clip().await.expect("save");
        let elapsed = t0.elapsed();
        let size = std::fs::metadata(&path).unwrap().len();
        let dur_ms = crate::editor::ffmpeg::probe_duration_ms(
            &super::super::video::capture_ffmpeg(),
            &path,
        )
        .await
        .unwrap_or(0);
        eprintln!(
            "[moonclip-test] 120s save: {:?} size={}MB duration={}ms",
            elapsed,
            size / 1024 / 1024,
            dur_ms
        );
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "save took {elapsed:?} (spec ceiling: 5 s on SATA/HDD; <3 s is the NVMe target)"
        );
        if elapsed >= std::time::Duration::from_secs(3) {
            eprintln!(
                "[moonclip-test] note: {:?} > 3 s NVMe target, within the SATA/HDD ceiling",
                elapsed
            );
        }
        assert!((110_000..=125_000).contains(&dur_ms), "duration {dur_ms} ms");
        eng.stop_buffer().await.expect("stop");
        let _ = std::fs::remove_file(&path);
    }

    /// Long-buffer drift rig: saves at ~1 min and at the configured end
    /// (`MOONCLIP_DRIFT_SECS`, default 1800 = 30 min) into
    /// `%TEMP%\moonclip-e2e-drift`, so `analyze_av.py` /
    /// `analyze_interaudio.py` can measure both ends of a long session.
    #[tokio::test]
    #[ignore]
    async fn live_drift_capture() {
        use super::super::super::CaptureEngine;

        let total: u64 = std::env::var("MOONCLIP_DRIFT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1800);
        let dir = std::env::temp_dir().join("moonclip-e2e-drift");
        std::fs::create_dir_all(&dir).unwrap();
        let mut eng = super::WindowsCaptureEngine::new();
        eng.start_buffer(cfg(dir, 5, 60)).await.expect("start");
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        let p1 = eng.save_clip().await.expect("save min1");
        eprintln!("[moonclip-test] drift min1: {}", p1.display());
        if total > 60 {
            tokio::time::sleep(std::time::Duration::from_secs(total - 60)).await;
            let p2 = eng.save_clip().await.expect("save end");
            eprintln!("[moonclip-test] drift end: {}", p2.display());
        }
        eng.stop_buffer().await.expect("stop");
    }

    /// Stop/drop + fresh start in the same process (settings-change path).
    #[tokio::test]
    #[ignore]
    async fn live_engine_restart() {
        use super::super::super::CaptureEngine;

        let dir = std::env::temp_dir().join("moonclip-e2e-restart");
        std::fs::create_dir_all(&dir).unwrap();
        let mut eng = super::WindowsCaptureEngine::new();
        eng.start_buffer(cfg(dir.clone(), 5, 60)).await.expect("start 1");
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        eng.stop_buffer().await.expect("stop 1");
        drop(eng);
        let mut eng = super::WindowsCaptureEngine::new();
        eng.start_buffer(cfg(dir, 5, 60)).await.expect("start 2");
        tokio::time::sleep(std::time::Duration::from_secs(14)).await;
        let path = eng.save_clip().await.expect("save 2");
        let probe = tokio::process::Command::new(super::super::video::capture_ffmpeg())
            .args(["-hide_banner", "-i", &path.to_string_lossy()])
            .output()
            .await
            .expect("probe");
        let err = String::from_utf8_lossy(&probe.stderr);
        assert_eq!(err.matches("Audio: aac").count(), 3, "want 3xAAC:\n{err}");
        let video_line = err
            .lines()
            .find(|l| l.contains("Video: h264"))
            .unwrap_or_default();
        assert!(!video_line.contains("start "), "{video_line}");
        // 5 s buffer on a 14 s run: latest key ≤ target → 5-8 s clip.
        let dur_ms = crate::editor::ffmpeg::probe_duration_ms(
            &super::super::video::capture_ffmpeg(),
            &path,
        )
        .await
        .expect("duration");
        assert!(
            (4500..=8500).contains(&dur_ms),
            "unexpected duration after keyframe cut: {dur_ms} ms"
        );
        eng.stop_buffer().await.expect("stop 2");
        let _ = std::fs::remove_file(&path);
    }

    /// Capture with the flash+beep reference for offline A/V analysis
    /// (`%TEMP%\moonclip-e2e-av`), run by `analyze_av.py`.
    #[tokio::test]
    #[ignore]
    async fn live_av_offset_capture() {
        use super::super::super::CaptureEngine;

        let codec = std::env::var("MOONCLIP_AVTEST_CODEC").unwrap_or_else(|_| "h264".into());
        let run_secs: u64 = std::env::var("MOONCLIP_AVTEST_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(14);
        let dir = std::env::temp_dir().join("moonclip-e2e-av");
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = cfg(dir, 5, 60);
        c.codec = codec.clone();
        c.nvenc_opts = Some(crate::video_quality::nvenc_hq_opts(&codec));
        let mut eng = super::WindowsCaptureEngine::new();
        eng.start_buffer(c).await.expect("start");
        tokio::time::sleep(std::time::Duration::from_secs(run_secs)).await;
        let path = eng.save_clip().await.expect("save");
        eprintln!("[moonclip-test] av clip: {}", path.display());
        eng.stop_buffer().await.expect("stop");
    }

    /// Tone coverage: 100% of the game stem must carry the tone while it
    /// plays (loopback reliability).
    #[tokio::test]
    #[ignore]
    async fn live_tone_coverage() {
        use super::super::super::CaptureEngine;
        use rodio::Source as _;

        let stream = rodio::OutputStreamBuilder::open_default_stream()
            .expect("open default tone stream");
        let sink = rodio::Sink::connect_new(stream.mixer());
        let dir = std::env::temp_dir().join("moonclip-e2e-tone");
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = cfg(dir, 10, 60);
        c.desktop_device = "default_output".into();
        let mut eng = super::WindowsCaptureEngine::new();
        eng.start_buffer(c).await.expect("start");
        sink.append(
            rodio::source::SineWave::new(1000.0)
                .take_duration(std::time::Duration::from_secs(20))
                .amplify(0.25),
        );
        tokio::time::sleep(std::time::Duration::from_secs(18)).await;
        let path = eng.save_clip().await.expect("save");
        drop(sink);
        drop(stream);
        let ff = super::super::video::capture_ffmpeg();
        let pcm = tokio::process::Command::new(&ff)
            .args([
                "-v", "error", "-i", &path.to_string_lossy(), "-map", "0:a:1", "-ac", "1",
                "-ar", "48000", "-f", "s16le", "-",
            ])
            .output()
            .await
            .expect("decode game stem");
        let samples: Vec<i16> = pcm
            .stdout
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        let win = 4800usize;
        let (mut total, mut active) = (0usize, 0usize);
        for chunk in samples.chunks(win) {
            let sum: f64 = chunk.iter().map(|&s| (s as f64) * (s as f64)).sum();
            let rms = (sum / chunk.len() as f64).sqrt() / 32768.0;
            total += 1;
            if rms > 0.01 {
                active += 1;
            }
        }
        let (dur, act) = (total as f64 * 0.1, active as f64 * 0.1);
        eprintln!("[moonclip-test] tone coverage: clip={dur:.2}s tone-active={act:.2}s");
        assert!(dur > 5.0, "clip too short: {dur:.2}s");
        assert!(act >= dur - 1.0, "tone missing for {:.2}s", dur - act);
        eng.stop_buffer().await.expect("stop");
        let _ = std::fs::remove_file(&path);
    }
}
