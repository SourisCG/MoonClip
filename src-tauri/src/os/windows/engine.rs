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
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, AtomicI64, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use tokio::process::{Child, Command};

use super::super::{CaptureConfig, CaptureEngine, SavePlan};
use super::audio::{self, AudioCapture};
use super::ts::{CutPlan, TsRing};
use super::video;

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
const ERR_PIPE_BYTES: u32 = 1 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested)
// ---------------------------------------------------------------------------

/// ffmpeg args for the live encoder **after** `-c:v <enc>`. CBR ladder +
/// 2 s GOP everywhere (Medal parity); HQ knobs only where valid
/// (NVIDIA + h264/hevc).
pub fn live_encoder_args(
    enc_name: &str,
    codec: &str,
    bitrate_kbps: u32,
    fps: u32,
    nvenc_hq: bool,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut flag = |k: &str, v: &str| {
        out.push(k.to_string());
        out.push(v.to_string());
    };
    if enc_name.ends_with("_nvenc") {
        if nvenc_hq {
            // HQ recipe: preset + HQ + profile + BF2 + Spatial AQ +
            // single-pass. Default **p5**, the preset OBS's own Auto
            // Configuration Wizard picks for this hardware. Measured on an
            // RTX 3060 under COD (real zero-copy chain): p7 pulls only
            // 19.6 fps and emits 852 dups / 20 s (NVENC ~0.5x realtime under
            // game load), p6 57.8 fps / 126 dups, p5 50 fps / 207. The
            // remaining wall is the game's own fps (a 60 fps CFR clip of a
            // ~58 fps game has ~2 dups/s; OBS is identical).
            // `MOONCLIP_NVENC_PRESET` swaps the preset (`p4` for more
            // headroom, `p7` = Linux parity) without rebuilding.
            let preset = std::env::var("MOONCLIP_NVENC_PRESET").unwrap_or_else(|_| "p5".into());
            flag("-preset", &preset);
            flag("-tune", "hq");
            flag("-profile:v", if codec == "hevc" { "main" } else { "high" });
            flag("-bf", "2");
            flag("-spatial-aq", "1");
            flag("-multipass", "disabled");
        }
        flag("-rc", "cbr");
    } else if enc_name.ends_with("_amf") {
        flag("-rc", "cbr");
        flag("-quality", "quality");
    } else if enc_name.ends_with("_qsv") {
        flag("-preset", "fast");
    } else if enc_name == "libx264" {
        flag("-preset", "veryfast");
        flag("-tune", "zerolatency");
        flag("-profile:v", "high");
        flag("-bf", "2");
    }
    let gop = (fps.max(1) * 2).to_string();
    flag("-b:v", &format!("{bitrate_kbps}k"));
    flag("-maxrate", &format!("{bitrate_kbps}k"));
    flag("-bufsize", &format!("{bitrate_kbps}k"));
    flag("-g", &gop);
    out
}

/// True when the GPU chain can take D3D11 frames straight from `gfxcapture`
/// (validated: NVENC). AMD/Intel run the hwdownload fallback until their
/// zero-copy chain is validated on real hardware.
fn gpu_zero_copy(vendor: &str, codec: &str) -> bool {
    vendor == "nvidia" && codec != "x264"
}

/// Even output width for a target height, preserving the source aspect.
fn scaled_width(mw: u32, mh: u32, out_height: u32) -> u32 {
    if mh == 0 {
        return mw;
    }
    let w = ((mw as f64 * out_height as f64 / mh as f64).round() as u32).max(2);
    w & !1
}

/// Capture rate cap for WGC. Two candidates per CFR slot (2x the output rate,
/// min 90; fallback 120 when the refresh is unknown) so a suppressed WGC
/// present becomes a dropped excess frame instead of a duplicate. Never the
/// full 164 Hz refresh: that triples the WGC copy/filter work under a game
/// for frames the CFR filter then throws away. `MOONCLIP_CAPTURE_MAX_FPS`
/// overrides it (clamped to >= fps).
pub fn capture_max_fps(refresh_hz: u32, fps: u32) -> u32 {
    let headroom = (fps * 2).max(90);
    let cap = if refresh_hz >= 30 {
        refresh_hz.min(headroom)
    } else {
        (fps * 2).max(120)
    };
    cap.max(fps)
}

/// CBR target override from `MOONCLIP_CAPTURE_BITRATE_KBPS` (test/A-B only):
/// swaps the ladder bitrate without rebuilding. Values below 500 kbps are
/// ignored (a typo must not produce an unusable clip).
fn capture_bitrate_override(env: Option<&str>) -> Option<u32> {
    env?.trim().parse::<u32>().ok().filter(|v| *v >= 500)
}

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

/// What to capture and at what maximum rate. Monitor selection is by native
/// HMONITOR (cross-process safe, validated) so the Rust-side monitor list is
/// authoritative — no index-order guessing. `max_fps` is the capture cap
/// (`capture_max_fps`: 2x the output rate, clamped to the panel refresh —
/// `MOONCLIP_CAPTURE_MAX_FPS` overrides). Window capture is opt-in
/// (`MOONCLIP_CAPTURE_SOURCE=window` or `MOONCLIP_CAPTURE_WINDOW_EXE=regex`).
#[derive(Debug, Clone)]
pub struct CaptureSource<'a> {
    /// `gfxcapture` (WGC: monitors and windows) or `ddagrab` (DDA, monitor).
    pub kind: &'a str,
    pub hmonitor: isize,
    pub monitor_idx: u32,
    /// WGC window selectors; `hwnd` wins over `window_exe`.
    pub hwnd: Option<u64>,
    pub window_exe: Option<&'a str>,
    pub max_fps: u32,
}

impl CaptureSource<'_> {
    fn is_window(&self) -> bool {
        self.kind == "gfxcapture" && (self.hwnd.is_some() || self.window_exe.is_some())
    }
    fn describe(&self) -> String {
        if let Some(h) = self.hwnd {
            format!("window hwnd={h}")
        } else if let Some(exe) = self.window_exe {
            format!("window exe~'{exe}'")
        } else if self.kind == "ddagrab" {
            format!("ddagrab output {}", self.monitor_idx)
        } else {
            format!("monitor hmonitor={}", self.hmonitor)
        }
    }
}

/// The capture filtergraph. Scaling happens inside the filter on the GPU
/// (bicubic) for the zero-copy chain; the download fallback uses lanczos.
/// `showinfo` runs pre-encoder and feeds the PTS<->QPC clock anchor.
pub fn capture_filter(
    vendor: &str,
    codec: &str,
    src: &CaptureSource<'_>,
    mw: u32,
    mh: u32,
    out_height: u32,
    fps: u32,
) -> String {
    let scale = out_height > 0 && out_height < mh;
    // ddagrab has no working GPU resizer here (`scale_d3d11` refuses its
    // frames); window capture scales through the download path too (its
    // canvas is the window, not the monitor).
    let zero = gpu_zero_copy(vendor, codec) && !(src.kind == "ddagrab" && scale) && !(src.is_window() && scale);
    let mut f = if src.kind == "ddagrab" {
        // `dup_frames=0`: deliver on change only. Duplicates are ffmpeg's job
        // (`-r fps -fps_mode cfr`); paying the OS to fabricate them here would
        // multiply the capture cost for nothing.
        format!(
            "ddagrab=output_idx={}:framerate={}:dup_frames=0",
            src.monitor_idx, src.max_fps
        )
    } else if let Some(h) = src.hwnd {
        format!(
            "gfxcapture=hwnd={h}:max_framerate={}:capture_cursor=1",
            src.max_fps
        )
    } else if let Some(exe) = src.window_exe {
        format!(
            "gfxcapture=window_exe='{exe}':max_framerate={}:capture_cursor=1",
            src.max_fps
        )
    } else {
        format!(
            "gfxcapture=hmonitor={}:max_framerate={}:capture_cursor=1",
            src.hmonitor, src.max_fps
        )
    };
    if zero {
        if scale {
            let tw = scaled_width(mw, mh, out_height);
            f.push_str(&format!(
                ":width={tw}:height={out_height}:resize_mode=scale_aspect:scale_mode=bicubic"
            ));
        }
    } else {
        f.push_str(",hwdownload,format=bgra");
        if scale {
            f.push_str(&format!(",scale=-2:{out_height}:flags=lanczos"));
        }
        f.push_str(",format=yuv420p");
    }
    // showinfo runs pre-encoder and logs each frame's PTS (100 ns) on stderr:
    // the engine uses the log arrival QPC as the PTS<->QPC clock anchor, so
    // encoder lookahead can never shift A/V (see `note_clock_sample`).
    // `MOONCLIP_NO_SHOWINFO=1` drops it for load A/B tests (A/V falls back to
    // the coarser PES-arrival calibration).
    if std::env::var("MOONCLIP_NO_SHOWINFO").as_deref() != Ok("1") {
        f.push_str(",showinfo");
    }
    f.push_str("[out]");
    let _ = fps;
    f
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

/// GSR-style clip name with a free suffix when the second already exists:
/// the muxer runs `-y`, so a same-second save must never target a live file.
pub fn unique_dest(dir: &Path, base: &str) -> PathBuf {
    let stem = base.strip_suffix(".mp4").unwrap_or(base);
    let mut cand = dir.join(base);
    let mut n = 2u32;
    while cand.exists() {
        cand = dir.join(format!("{stem}_{n}.mp4"));
        n += 1;
    }
    cand
}

fn replay_filename() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    format!(
        "replay_{:04}-{:02}-{:02}_{:02}-{:02}-{:02}.mp4",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

/// dBFS for the save stats line (-inf when the stem is digital silence).
fn dbfs(v: f32) -> String {
    if v <= 0.0 {
        "-inf".to_string()
    } else {
        format!("{:.1}dB", 20.0 * v.log10())
    }
}

/// MIX track from the two solo stems (sample sum, clamped). Stems share the
/// 48 kHz stereo grid and the same window, so they are sample aligned.
pub fn mix_i16(game: &[i16], mic: &[i16]) -> Vec<i16> {
    let n = game.len().min(mic.len());
    game[..n]
        .iter()
        .zip(&mic[..n])
        .map(|(&g, &m)| (g as i32 + m as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16)
        .collect()
}

/// Minimal PCM-16 WAV writer (stereo 48 kHz).
pub fn write_wav(path: &Path, samples: &[i16]) -> std::io::Result<()> {
    use std::io::BufWriter;
    let data_bytes = (samples.len() * 2) as u32;
    let mut hdr = [0u8; 44];
    hdr[0..4].copy_from_slice(b"RIFF");
    hdr[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    hdr[8..12].copy_from_slice(b"WAVE");
    hdr[12..16].copy_from_slice(b"fmt ");
    hdr[16..20].copy_from_slice(&16u32.to_le_bytes());
    hdr[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    hdr[22..24].copy_from_slice(&2u16.to_le_bytes()); // stereo
    hdr[24..28].copy_from_slice(&48_000u32.to_le_bytes());
    hdr[28..32].copy_from_slice(&(48_000u32 * 2 * 2).to_le_bytes());
    hdr[32..34].copy_from_slice(&4u16.to_le_bytes());
    hdr[34..36].copy_from_slice(&16u16.to_le_bytes());
    hdr[36..40].copy_from_slice(b"data");
    hdr[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    let f = std::fs::File::create(path)?;
    let mut w = BufWriter::with_capacity(1024 * 1024, f);
    w.write_all(&hdr)?;
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    w.write_all(&bytes)?;
    w.flush()
}

// ---------------------------------------------------------------------------
// MP4 default-track hardening
// ---------------------------------------------------------------------------

/// Offset of the `alternate_group` u16 inside a `tkhd` payload.
fn tkhd_alternate_group_off(version: u8) -> Option<usize> {
    match version {
        0 => Some(34),
        1 => Some(46),
        _ => None,
    }
}

fn box_at(buf: &[u8], off: usize) -> Option<([u8; 4], usize, usize, usize)> {
    if off + 8 > buf.len() {
        return None;
    }
    let size = u32::from_be_bytes(buf[off..off + 4].try_into().ok()?) as u64;
    let mut typ = [0u8; 4];
    typ.copy_from_slice(&buf[off + 4..off + 8]);
    let (header, total) = match size {
        0 => (8u64, (buf.len() - off) as u64),
        1 => {
            if off + 16 > buf.len() {
                return None;
            }
            let large = u64::from_be_bytes(buf[off + 8..off + 16].try_into().ok()?);
            (16, large)
        }
        n => (8, n),
    };
    let total = total as usize;
    if total < header as usize || off + total > buf.len() {
        return None;
    }
    Some((typ, off + header as usize, total - header as usize, off + total))
}

fn box_children(buf: &[u8], start: usize, len: usize) -> Vec<([u8; 4], usize, usize)> {
    let mut out = Vec::new();
    let end = start.saturating_add(len).min(buf.len());
    let mut off = start;
    while off < end {
        let Some((typ, payload, plen, next)) = box_at(buf, off) else {
            break;
        };
        if next <= off {
            break;
        }
        out.push((typ, payload, plen));
        off = next;
        if out.len() > 4096 {
            break;
        }
    }
    out
}

/// `alternate_group` offsets (relative to the `moov` payload) of every audio
/// track. A track counts as audio when its `mdia/hdlr` handler is `soun`.
fn audio_tkhd_alt_group_offsets(moov: &[u8]) -> Vec<usize> {
    let mut offs = Vec::new();
    for (typ, pstart, plen) in box_children(moov, 0, moov.len()) {
        if &typ != b"trak" {
            continue;
        }
        let mut tkhd: Option<usize> = None;
        let mut is_audio = false;
        for (t2, p2, l2) in box_children(moov, pstart, plen) {
            if &t2 == b"tkhd" {
                tkhd = Some(p2);
            } else if &t2 == b"mdia" {
                for (t3, p3, l3) in box_children(moov, p2, l2) {
                    if &t3 == b"hdlr" && l3 >= 12 && &moov[p3 + 8..p3 + 12] == b"soun" {
                        is_audio = true;
                    }
                }
            }
        }
        if !is_audio {
            continue;
        }
        let Some(t) = tkhd else { continue };
        let Some(ver) = moov.get(t).copied() else {
            continue;
        };
        let Some(rel) = tkhd_alternate_group_off(ver) else {
            continue;
        };
        if t + rel + 2 <= moov.len() {
            offs.push(t + rel);
        }
    }
    offs
}

/// Put every audio track of `path` into alternate group `group`: the standard
/// "these tracks are alternatives, use the enabled/default one" signal. Only
/// the 2-byte fields are rewritten in place. Best effort by design.
pub fn patch_audio_alternate_group(path: &Path, group: u16) -> Result<usize, String> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    let file_len = f.metadata().map_err(|e| format!("stat: {e}"))?.len();
    let mut off = 0u64;
    let mut moov: Option<(u64, usize)> = None;
    while off + 8 <= file_len {
        f.seek(SeekFrom::Start(off))
            .map_err(|e| format!("seek: {e}"))?;
        let mut hdr = [0u8; 16];
        if f.read(&mut hdr[..8]).map_err(|e| format!("read: {e}"))? < 8 {
            break;
        }
        let size = u32::from_be_bytes(hdr[0..4].try_into().unwrap()) as u64;
        let typ = [hdr[4], hdr[5], hdr[6], hdr[7]];
        let (header, total) = match size {
            0 => (8u64, file_len - off),
            1 => {
                f.read_exact(&mut hdr[8..16])
                    .map_err(|e| format!("read largesize: {e}"))?;
                (16u64, u64::from_be_bytes(hdr[8..16].try_into().unwrap()))
            }
            n => (8, n),
        };
        if total < header || off + total > file_len {
            return Err("corrupt mp4 (box size out of range)".into());
        }
        if &typ == b"moov" {
            moov = Some((off + header, (total - header) as usize));
            break;
        }
        off += total;
    }
    let Some((p_off, p_len)) = moov else {
        return Ok(0);
    };
    let mut buf = vec![0u8; p_len];
    f.seek(SeekFrom::Start(p_off))
        .map_err(|e| format!("seek moov: {e}"))?;
    f.read_exact(&mut buf)
        .map_err(|e| format!("read moov: {e}"))?;
    let offsets = audio_tkhd_alt_group_offsets(&buf);
    for rel in &offsets {
        f.seek(SeekFrom::Start(p_off + *rel as u64))
            .map_err(|e| format!("seek field: {e}"))?;
        f.write_all(&group.to_be_bytes())
            .map_err(|e| format!("write field: {e}"))?;
    }
    f.sync_all().map_err(|e| format!("sync: {e}"))?;
    Ok(offsets.len())
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
    duration_secs: u32,
    bitrate_kbps: u32,
    fps: u32,
    ffmpeg: PathBuf,
    /// Ring capacity granted at start (for the legacy fallback window).
    ring_cap: usize,
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
            duration_secs: 0,
            bitrate_kbps: 0,
            fps: 60,
            ffmpeg: PathBuf::new(),
            ring_cap: 16 * 1024 * 1024,
        }
    }

    /// Spawn `ffmpeg (capture -> encoder -> mpegts)` and wire the drain
    /// threads. The encoder process gets above-normal CPU priority, power
    /// throttling disabled and HIGH GPU scheduling priority, so a heavy game
    /// competing for the GPU does not starve the capture.
    fn spawn_encoder(
        &self,
        ffmpeg: &Path,
        vendor: &str,
        codec: &str,
        enc_name: &str,
        src: &CaptureSource<'_>,
        mw: u32,
        mh: u32,
        out_height: u32,
        bitrate_kbps: u32,
        fps: u32,
        nvenc_hq: bool,
    ) -> Result<Child, String> {
        let filter = capture_filter(vendor, codec, src, mw, mh, out_height, fps);
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
        let mut cmd = Command::new(ffmpeg);
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
            enc_name,
        ]);
        cmd.args(live_encoder_args(enc_name, codec, bitrate_kbps, fps, nvenc_hq));
        // No `-flush_packets 1`: with an 8 MB pipe the per-packet flush buys
        // nothing and costs one syscall per packet.
        cmd.args([
            "-r",
            &fps.to_string(),
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
            .map_err(|e| format!("cannot launch capture encoder ({}): {e}", ffmpeg.display()))?;
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
                                        let qpc = audio::qpc_ns();
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
                            let qpc = audio::qpc_ns();
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

    /// Mux the staged TS + stems into the final clip. The audio stems are
    /// already exactly `window_ns` long starting at the keyframe, so only the
    /// video input is seeked (`-ss`) onto that same keyframe: both streams
    /// start at 0 aligned, no per-input trims, no re-encode of video.
    async fn mux_clip(
        ffmpeg: &Path,
        cut_ts: &Path,
        wavs: &[(PathBuf, &str)],
        dest: &Path,
        ss_secs: f64,
    ) -> Result<(), String> {
        let mut cmd = Command::new(ffmpeg);
        cmd.args(["-y", "-hide_banner", "-loglevel", "error"]);
        // A hair before the keyframe (input seek drops PTS < target); only
        // applied when the stage actually has pre-roll before the keyframe.
        if ss_secs > 0.001 {
            cmd.args(["-ss", &format!("{ss_secs:.3}")]);
        }
        cmd.arg("-i").arg(cut_ts);
        for (p, _) in wavs {
            cmd.arg("-i").arg(p);
        }
        cmd.args(["-map", "0:v"]);
        for i in 0..wavs.len() {
            cmd.arg("-map").arg(format!("{}:a", i + 1));
        }
        cmd.args(["-c:v", "copy", "-c:a", "aac", "-b:a", "160k"]);
        for (i, (_, title)) in wavs.iter().enumerate() {
            cmd.arg(format!("-metadata:s:a:{i}")).arg(format!("title={title}"));
        }
        cmd.arg("-shortest");
        let out = cmd
            .arg(dest)
            .output()
            .await
            .map_err(|e| format!("save mux failed: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let tail = err
                .lines()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" | ");
            return Err(format!("save mux failed (ffmpeg): {tail}"));
        }
        Ok(())
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
        let samples = audio::window_frames(window_ns);
        let (game, mic) = self
            .audio
            .as_ref()
            .map(|a| a.snapshot_window(qpc_start_ns, samples))
            .unwrap_or_default();
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
        let dest = unique_dest(&self.output_dir, &replay_filename());
        let t_tmp = dest.with_extension("mux.tmp");
        let game_path = t_tmp.with_extension("game.wav");
        let mic_path = t_tmp.with_extension("mic.wav");
        // Only the legacy fallback needs a materialized Mix WAV; the fast path
        // lets ffmpeg sum the solos (`amix normalize=0`).
        let fallback_mix = if ss_secs > 0.001 {
            Some(mix_i16(&game, &mic))
        } else {
            None
        };
        let t_wav = std::time::Instant::now();
        {
            let (gp, mp) = (game_path.clone(), mic_path.clone());
            let (gw, mw) = tokio::join!(
                tokio::task::spawn_blocking(move || write_wav(&gp, &game)),
                tokio::task::spawn_blocking(move || write_wav(&mp, &mic)),
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
                write_wav(&mix_path, &mix).map_err(|e| format!("cannot write mix wav: {e}"))?;
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
                let r = Self::mux_clip(&ffmpeg, &ts_path, &wavs, &dest, ss_secs).await;
                let _ = tokio::fs::remove_file(&ts_path).await;
                let _ = tokio::fs::remove_file(&mix_path).await;
                r
            }
            None => Self::mux_clip_pipe(&ffmpeg, bytes, &game_path, &mic_path, single, &dest).await,
        };
        let _ = tokio::fs::remove_file(&game_path).await;
        let _ = tokio::fs::remove_file(&mic_path).await;
        res?;
        eprintln!(
            "[moonclip] mux: wav={wav_elapsed:?} mux={:?}",
            t_mux.elapsed()
        );
        match patch_audio_alternate_group(&dest, 1) {
            Ok(n) => eprintln!("[moonclip] mp4: alternate_group=1 on {n} audio track(s)"),
            Err(e) => eprintln!("[moonclip] mp4: alternate_group patch skipped: {e}"),
        }
        eprintln!("[moonclip] save done in {:?}", t_save.elapsed());
        Ok(dest)
    }

    /// Fast mux: TS over stdin, solo stems as WAVs, Mix via `amix`.
    async fn mux_clip_pipe(
        ffmpeg: &Path,
        ts: Vec<u8>,
        game: &Path,
        mic: &Path,
        single: bool,
        dest: &Path,
    ) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;
        let mut cmd = Command::new(ffmpeg);
        cmd.args([
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "mpegts", "-i", "pipe:0",
        ]);
        cmd.arg("-i").arg(game).arg("-i").arg(mic);
        cmd.args([
            "-filter_complex",
            "[1:a][2:a]amix=inputs=2:normalize=0:dropout_transition=0[mix]",
            "-map", "0:v", "-map", "[mix]",
        ]);
        if !single {
            cmd.args(["-map", "1:a", "-map", "2:a"]);
        }
        cmd.args(["-c:v", "copy", "-c:a", "aac", "-b:a", "160k"]);
        cmd.args(["-metadata:s:a:0", "title=Mix"]);
        if !single {
            cmd.args([
                "-metadata:s:a:1", "title=Game",
                "-metadata:s:a:2", "title=Mic",
            ]);
        }
        cmd.arg("-shortest")
            .arg(dest)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| format!("save mux failed: {e}"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "mux stdin unavailable".to_string())?;
        let writer = tokio::spawn(async move {
            let _ = stdin.write_all(&ts).await;
            let _ = stdin.shutdown().await;
        });
        let out = child
            .wait_with_output()
            .await
            .map_err(|e| format!("save mux failed: {e}"))?;
        let _ = writer.await;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let tail = err
                .lines()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" | ");
            return Err(format!("save mux failed (ffmpeg): {tail}"));
        }
        Ok(())
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
        let samples = (window_ns * audio::STEM_RATE as i128 / 1_000_000_000) as usize;
        let (game, mic) = self
            .audio
            .as_ref()
            .map(|a| a.snapshot_window(qpc_start, samples))
            .unwrap_or_default();
        let mix = mix_i16(&game, &mic);
        let dest = unique_dest(&self.output_dir, &replay_filename());
        // Legacy mux: every input is trimmed by the same keyframe time.
        let single = self.audio_single_track;
        let mut stems: Vec<(Vec<i16>, &str)> = if single {
            vec![(mix, "Mix")]
        } else {
            vec![(mix, "Mix"), (game, "Game"), (mic, "Mic")]
        };
        let tmp = dest.with_extension("mux.tmp");
        let mut wavs: Vec<(PathBuf, &str)> = Vec::new();
        for (i, (samples, title)) in stems.drain(..).enumerate() {
            let p = tmp.with_extension(format!("stem{i}.wav"));
            write_wav(&p, &samples).map_err(|e| format!("cannot write {title} wav: {e}"))?;
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
        cmd.args(["-c:v", "copy", "-c:a", "aac", "-b:a", "160k"]);
        for (i, (_, title)) in wavs.iter().enumerate() {
            cmd.arg(format!("-metadata:s:a:{i}")).arg(format!("title={title}"));
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
        match patch_audio_alternate_group(&dest, 1) {
            Ok(n) => eprintln!("[moonclip] mp4: alternate_group=1 on {n} audio track(s)"),
            Err(e) => eprintln!("[moonclip] mp4: alternate_group patch skipped: {e}"),
        }
        Ok(dest)
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
    let h = HANDLE(raw as *mut core::ffi::c_void);
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
        let bitrate_kbps = capture_bitrate_override(
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
        self.last_rx_qpc.store(audio::qpc_ns() as i64, Ordering::Relaxed);
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
        // Capture source. Default: WGC monitor (`gfxcapture`). Opt-in A/B:
        //   MOONCLIP_CAPTURE_SOURCE=ddagrab  -> Desktop Duplication (monitor)
        //   MOONCLIP_CAPTURE_SOURCE=window   -> foreground window (WGC hwnd)
        //   MOONCLIP_CAPTURE_WINDOW_EXE=^cod.exe$ -> game window by regex
        let env_source = std::env::var("MOONCLIP_CAPTURE_SOURCE").unwrap_or_default();
        let env_window_exe = std::env::var("MOONCLIP_CAPTURE_WINDOW_EXE")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let window_mode = env_source == "window" || env_window_exe.is_some();
        let kind = if env_source == "ddagrab" { "ddagrab" } else { "gfxcapture" };
        let (mut hwnd, mut window_exe): (Option<u64>, Option<String>) = (None, None);
        if window_mode {
            match env_window_exe.as_deref() {
                Some(exe) => window_exe = Some(exe.to_string()),
                None => hwnd = video::foreground_window().map(|h| h as u64),
            }
        }
        if window_mode && hwnd.is_none() && window_exe.is_none() {
            eprintln!(
                "[moonclip] window capture requested but no target window found; falling back to monitor"
            );
        }
        // Max capture rate = 2x the output rate (see `capture_max_fps`), never
        // the full refresh: under a game, full-refresh WGC copies + filtering
        // steal GPU time from NVENC and the CFR filter then discards those
        // frames anyway. ffmpeg owns the exact CFR with `-r fps -fps_mode cfr`.
        let max_fps = std::env::var("MOONCLIP_CAPTURE_MAX_FPS")
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .map(|v| v.max(fps))
            .unwrap_or_else(|| capture_max_fps(monitor.refresh_hz, fps));
        let source = CaptureSource {
            kind,
            hmonitor: monitor.hmonitor,
            monitor_idx: monitor.index,
            hwnd,
            window_exe: window_exe.as_deref(),
            max_fps,
        };
        let child = self.spawn_encoder(
            &ffmpeg,
            &vendor,
            &config.codec,
            enc_name,
            &source,
            monitor.width,
            monitor.height,
            out_height,
            bitrate_kbps,
            fps,
            nvenc_hq,
        )?;
        self.child = Some(child);
        // Prove the chain before committing: a bad combo (missing HW block,
        // unsupported filter option) exits within milliseconds, and indexed
        // frames prove capture+encode+mux actually flow.
        let deadline = std::time::Instant::now() + Duration::from_millis(FRAME_PROOF_MS);
        let mut frames = 0usize;
        while std::time::Instant::now() < deadline {
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
                    return Err(format!("capture encoder exited ({status}): {err}"));
                }
            }
            frames = self.ring.lock().map(|r| r.frame_count()).unwrap_or(0);
            if frames > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(FRAME_PROOF_POLL_MS)).await;
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
        self.duration_secs = config.duration_seconds;
        self.bitrate_kbps = bitrate_kbps;
        self.fps = fps;
        self.ffmpeg = ffmpeg;
        eprintln!(
            "[moonclip] capture buffer: {}x{}@{} ({}, max {}) {} ({}) -> {}p{}kbps, audio {}/2",
            monitor.width,
            monitor.height,
            fps,
            source.describe(),
            source.max_fps,
            enc_name,
            vendor,
            out_height,
            bitrate_kbps,
            self.audio.as_ref().map(|a| a.live_count()).unwrap_or(0)
        );
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
        let now = audio::qpc_ns();
        let end_qpc = cut.qpc_start_ns + cut.window_ns;
        let (g, m) = self
            .audio
            .as_ref()
            .map(|a| a.delivery_qpc())
            .unwrap_or((0, 0));
        eprintln!(
            "[moonclip] sync: key_qpc={:.3}s end_qpc={:.3}s video_lag={:.0}ms audio_end game={:.0}ms mic={:.0}ms calib={:.3}s",
            cut.qpc_start_ns as f64 / 1e9,
            end_qpc as f64 / 1e9,
            (now - end_qpc) as f64 / 1e6,
            (g as i128 - end_qpc) as f64 / 1e6,
            (m as i128 - end_qpc) as f64 / 1e6,
            self.ring
                .lock()
                .map(|r| r.calib_ns().map(|c| c as f64 / 1e9).unwrap_or(0.0))
                .unwrap_or(0.0),
        );
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
            let age_ns = audio::qpc_ns() - last as i128;
            let frames = self.ring.lock().map(|r| r.frame_count()).unwrap_or(0);
            if frames > 0
                && age_ns > STALL_AFTER.as_nanos() as i128
                && !self.stall_logged.swap(true, Ordering::Relaxed)
            {
                eprintln!(
                    "[moonclip] video source stalled ({:.1}s without new frames; exclusive fullscreen?)",
                    age_ns as f64 / 1e9
                );
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
        let (_, start, len) = super::box_children(&bytes, 0, bytes.len())
            .into_iter()
            .find(|(t, _, _)| t == b"moov")
            .expect("moov box");
        let moov = &bytes[start..start + len];
        let mut out = Vec::new();
        for (t, p, l) in super::box_children(moov, 0, moov.len()) {
            if &t != b"trak" {
                continue;
            }
            let mut tkhd = None;
            let mut audio = false;
            for (t2, p2, l2) in super::box_children(moov, p, l) {
                if &t2 == b"tkhd" {
                    tkhd = Some(p2);
                } else if &t2 == b"mdia" {
                    for (t3, p3, l3) in super::box_children(moov, p2, l2) {
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
            let rel = super::tkhd_alternate_group_off(ver).expect("tkhd version");
            let alt = u16::from_be_bytes([moov[t + rel], moov[t + rel + 1]]);
            out.push((ver, flags, alt));
        }
        out
    }

    #[test]
    fn nvenc_hq_flags() {
        let a = live_encoder_args("h264_nvenc", "h264", 20000, 60, true);
        let s = a.join(" ");
        assert!(s.contains("-preset p5"), "{s}");
        assert!(s.contains("-tune hq"), "{s}");
        assert!(s.contains("-profile:v high"), "{s}");
        assert!(s.contains("-spatial-aq 1"), "{s}");
        assert!(s.contains("-multipass disabled"), "{s}");
        assert!(s.contains("-rc cbr"), "{s}");
        assert!(s.contains("-g 120"), "{s}");
    }

    #[test]
    fn nvenc_plain_no_hq() {
        let a = live_encoder_args("av1_nvenc", "av1", 8000, 60, false);
        let s = a.join(" ");
        assert!(!s.contains("-preset"), "{s}");
        assert!(s.contains("-rc cbr"), "{s}");
    }

    #[test]
    fn x264_and_qsv_shapes() {
        let x = live_encoder_args("libx264", "x264", 20000, 30, false).join(" ");
        assert!(x.contains("-preset veryfast"), "{x}");
        assert!(x.contains("zerolatency"), "{x}");
        assert!(x.contains("-g 60"), "{x}");
        let q = live_encoder_args("h264_qsv", "h264", 20000, 60, false).join(" ");
        assert!(q.contains("-preset fast"), "{q}");
    }

    #[test]
    fn capture_cap_keeps_two_candidates_per_slot() {
        // 164 Hz monitor, 60 fps output -> 120, not the refresh and not 60:
        // two source frames per CFR slot so a suppressed present drops instead
        // of duplicating.
        assert_eq!(capture_max_fps(164, 60), 120);
        assert_eq!(capture_max_fps(144, 60), 120);
        // A 60 Hz panel is already the cap (never above it).
        assert_eq!(capture_max_fps(60, 60), 60);
        // 30 fps output still gets the min-90 headroom.
        assert_eq!(capture_max_fps(164, 30), 90);
        // Unknown refresh falls back to >= 2x.
        assert_eq!(capture_max_fps(0, 60), 120);
        assert_eq!(capture_max_fps(24, 30), 120);
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
    fn capture_bitrate_override_parses_and_guards() {
        assert_eq!(capture_bitrate_override(None), None);
        assert_eq!(capture_bitrate_override(Some("")), None);
        assert_eq!(capture_bitrate_override(Some("10000")), Some(10_000));
        assert_eq!(capture_bitrate_override(Some(" 20000 ")), Some(20_000));
        assert_eq!(capture_bitrate_override(Some("499")), None);
        assert_eq!(capture_bitrate_override(Some("junk")), None);
    }

    #[test]
    fn capture_filter_shapes() {
        // NVENC zero-copy; max fps is the 2x-headroom cap (`capture_max_fps`).
        let monitor = CaptureSource {
            kind: "gfxcapture",
            hmonitor: 65673,
            monitor_idx: 0,
            hwnd: None,
            window_exe: None,
            max_fps: 165,
        };
        let f = capture_filter("nvidia", "h264", &monitor, 1920, 1080, 720, 60);
        assert!(f.starts_with("gfxcapture=hmonitor=65673:max_framerate=165"), "{f}");
        assert!(f.contains("width=1280:height=720:resize_mode=scale_aspect:scale_mode=bicubic"), "{f}");
        assert!(!f.contains("hwdownload"), "{f}");
        assert!(f.ends_with("[out]"));
        // Fallback path downloads + lanczos + yuv420p.
        let amd = CaptureSource { kind: "gfxcapture", hmonitor: 1, monitor_idx: 0, hwnd: None, window_exe: None, max_fps: 120 };
        let f = capture_filter("amd", "h264", &amd, 1920, 1080, 720, 30);
        assert!(f.contains("hwdownload,format=bgra"), "{f}");
        assert!(f.contains("scale=-2:720:flags=lanczos"), "{f}");
        assert!(f.contains("format=yuv420p"), "{f}");
        // x264 always downloads even on NVIDIA.
        let big = CaptureSource { kind: "gfxcapture", hmonitor: 1, monitor_idx: 0, hwnd: None, window_exe: None, max_fps: 165 };
        let f = capture_filter("nvidia", "x264", &big, 2560, 1440, 0, 60);
        assert!(f.contains("hwdownload"), "{f}");
        assert!(!f.contains("scale="), "{f}");
        // Desktop Duplication fallback by output index, duplicates off.
        let dda = CaptureSource { kind: "ddagrab", hmonitor: 1, monitor_idx: 2, hwnd: None, window_exe: None, max_fps: 165 };
        let f = capture_filter("nvidia", "h264", &dda, 1920, 1080, 0, 60);
        assert!(f.starts_with("ddagrab=output_idx=2:framerate=165:dup_frames=0"), "{f}");
        assert!(!f.contains("hmonitor"), "{f}");
        // ddagrab has no GPU resizer: scaling goes through the download path.
        let f = capture_filter("nvidia", "h264", &dda, 1920, 1080, 720, 60);
        assert!(f.contains("hwdownload,format=bgra"), "{f}");
        assert!(f.contains("scale=-2:720:flags=lanczos"), "{f}");
        // Window capture: hwnd wins; scaling falls to the download path.
        let win = CaptureSource { kind: "gfxcapture", hmonitor: 1, monitor_idx: 0, hwnd: Some(4242), window_exe: None, max_fps: 144 };
        let f = capture_filter("nvidia", "h264", &win, 1920, 1080, 720, 60);
        assert!(f.starts_with("gfxcapture=hwnd=4242:max_framerate=144"), "{f}");
        assert!(f.contains("hwdownload"), "{f}");
        let win_exe = CaptureSource { kind: "gfxcapture", hmonitor: 1, monitor_idx: 0, hwnd: None, window_exe: Some("^cod.exe$"), max_fps: 144 };
        let f = capture_filter("nvidia", "h264", &win_exe, 1920, 1080, 0, 60);
        assert!(f.starts_with("gfxcapture=window_exe='^cod.exe$':max_framerate=144"), "{f}");
        assert!(!f.contains("hwdownload"), "{f}");
    }

    #[test]
    fn unique_dest_avoids_overwrite() {
        let dir = std::env::temp_dir().join(format!("moonclip-unique-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let base = "replay_2026-09-16_10-00-00.mp4";
        let first = unique_dest(&dir, base);
        assert_eq!(first.file_name().unwrap().to_str().unwrap(), base);
        std::fs::write(&first, b"x").unwrap();
        let second = unique_dest(&dir, base);
        assert_eq!(
            second.file_name().unwrap().to_str().unwrap(),
            "replay_2026-09-16_10-00-00_2.mp4"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mix_sums_aligned_tails() {
        assert_eq!(mix_i16(&[1000, -1000], &[500, 500]), vec![1500, -500]);
        assert_eq!(mix_i16(&[30000, 0], &[30000, 0]), vec![32767, 0]);
        assert!(mix_i16(&[], &[1]).is_empty());
    }

    #[test]
    fn wav_exact_size_and_pcm() {
        let dir = std::env::temp_dir().join(format!("moonclip-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.wav");
        let samples = vec![0i16; 48_000 * 2];
        write_wav(&p, &samples).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(bytes.len(), 44 + 48_000 * 2 * 2);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(i16::from_le_bytes([bytes[44], bytes[45]]), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn filename_shape() {
        let n = replay_filename();
        assert!(n.starts_with("replay_"), "{n}");
        assert!(n.ends_with(".mp4"), "{n}");
        assert_eq!(n.len(), "replay_YYYY-MM-DD_HH-MM-SS.mp4".len(), "{n}");
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

    /// MP4 hardening: each audio `tkhd` gets `alternate_group` in place,
    /// the video track and the rest stay byte identical.
    #[test]
    fn mp4_patch_audio_alternate_group() {
        use super::{audio_tkhd_alt_group_offsets, patch_audio_alternate_group};

        fn bx(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
            let mut v = Vec::new();
            v.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
            v.extend_from_slice(typ);
            v.extend_from_slice(payload);
            v
        }
        fn tkhd(version: u8, track_id: u32) -> Vec<u8> {
            let mut p = vec![0u8; if version == 0 { 84 } else { 96 }];
            p[0] = version;
            p[1..4].copy_from_slice(&[0, 0, 3]);
            let id_off = if version == 0 { 12 } else { 20 };
            p[id_off..id_off + 4].copy_from_slice(&track_id.to_be_bytes());
            p
        }
        fn hdlr(handler: &[u8; 4]) -> Vec<u8> {
            let mut p = vec![0u8; 16];
            p[8..12].copy_from_slice(handler);
            p
        }
        fn trak(version: u8, track_id: u32, handler: &[u8; 4]) -> Vec<u8> {
            let mut kids = bx(b"tkhd", &tkhd(version, track_id));
            kids.extend_from_slice(&bx(b"mdia", &bx(b"hdlr", &hdlr(handler))));
            bx(b"trak", &kids)
        }

        let mut moov = Vec::new();
        let video_trak_at = moov.len();
        moov.extend_from_slice(&trak(0, 1, b"vide"));
        let video_alt = video_trak_at + 16 + 34;
        let a0_at = moov.len();
        moov.extend_from_slice(&trak(0, 2, b"soun"));
        let mut expected = vec![a0_at + 16 + 34];
        let a1_at = moov.len();
        moov.extend_from_slice(&trak(1, 3, b"soun"));
        expected.push(a1_at + 16 + 46);
        assert_eq!(audio_tkhd_alt_group_offsets(&moov), expected);

        let mut file = bx(b"ftyp", &[0u8; 16]);
        file.extend_from_slice(&bx(b"mdat", &[0u8; 32]));
        let moov_payload = file.len() + 8;
        file.extend_from_slice(&bx(b"moov", &moov));
        let before = file.clone();
        let path = std::env::temp_dir().join(format!(
            "moonclip-mp4-group-{}.mp4",
            std::process::id()
        ));
        std::fs::write(&path, &file).unwrap();

        assert_eq!(patch_audio_alternate_group(&path, 1).unwrap(), 2);
        let after = std::fs::read(&path).unwrap();
        for (i, rel) in expected.iter().enumerate() {
            let at = moov_payload + rel;
            assert_eq!(&after[at..at + 2], &[0, 1], "audio track {i} not patched");
        }
        let vat = moov_payload + video_alt;
        assert_eq!(&after[vat..vat + 2], &[0, 0], "video track was patched");
        let diffs = before
            .iter()
            .zip(&after)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        let mut want: Vec<usize> = expected.iter().map(|r| moov_payload + r + 1).collect();
        want.sort_unstable();
        assert_eq!(diffs, want, "unexpected bytes changed");
        let _ = std::fs::remove_file(&path);
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
        use cpal::traits::DeviceTrait as _;
        use rodio::Source as _;

        let dev = super::super::devices::find_output_device("default_output")
            .expect("no output device");
        eprintln!(
            "[moonclip-test] tone device: '{}'",
            dev.name().unwrap_or_default()
        );
        let stream = rodio::OutputStreamBuilder::from_device(dev)
            .and_then(|b| b.open_stream())
            .expect("open tone stream");
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
