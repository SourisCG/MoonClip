//! Shared capture contract (OS-independent).
//! Every backend under os/linux and os/windows implements this trait.
//! Mirrors the OBS model: uniform interface, per-OS files behind it.

use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaptureConfig {
    pub duration_seconds: u32,
    pub fps: u32,
    pub output_dir: PathBuf,
    /// Resolved GSR binary (bundled sidecar). None = resolve from env/PATH.
    pub gsr_bin: Option<PathBuf>,
    /// Audio sources for `-a` (game first, mic second).
    pub desktop_device: String,
    pub mic_device: String,
    /// Capture source for `-w` ("screen", monitor name, …). Empty = backend default.
    pub source: String,
    /// Video codec id for `-k` (h264/hevc/av1, as listed by the backend).
    pub codec: String,
    /// Spawn `-s` height (0 = omit, capture at source). This is the BUFFER
    /// resolution; the file the user asked for may differ (see save_height).
    pub out_height: u32,
    /// CBR bitrate in kbps for `-q` (matches the BUFFER resolution).
    pub bitrate_kbps: u32,
    /// Height to deliver at save time (0 = keep captured). When lower than
    /// the buffer resolution, the saver downscales with lanczos instead of
    /// trusting the backend's live scaler (proven soft on text, 1.5x ratios).
    pub save_height: u32,
    /// CBR bitrate for the delivered file (ladder row of save_height).
    pub save_bitrate_kbps: u32,
    /// Save-time encoder (None = no valid encoder on this GPU, keep source).
    pub save_encoder: Option<TranscodeEncoder>,
    /// Extra `-ffmpeg-video-opts` (NVENC HQ recipe, NVIDIA only). None = backend defaults.
    pub nvenc_opts: Option<String>,
    /// Resolved ffmpeg binary for encode/mux/probe duties (bundled sidecar
    /// first, dev PATH fallback). None = resolve via env/PATH legacy path.
    /// The Linux GSR backend ignores this (it shells its own sidecar setup).
    pub ffmpeg_bin: Option<PathBuf>,
    /// Compatibility mode: save with a single audio track (the Mix) so any
    /// player plays it. False = 3 tracks (Mix, Game, Mic; Linux parity).
    pub audio_single_track: bool,
}

/// Background downscale applied at save time (lanczos, per-vendor encoder).
#[derive(Debug, Clone, Default)]
pub struct SavePlan {
    pub height: u32,
    pub bitrate_kbps: u32,
    pub codec: String,
    pub fps: u32,
    /// None = no valid save-time encoder on this GPU (keep source file).
    pub encoder: Option<TranscodeEncoder>,
}

/// Unified engine interface. Methods are async to allow signal waits / IPC.
#[allow(async_fn_in_trait)]
pub trait CaptureEngine: Send + Sync {
    async fn start_buffer(&mut self, config: CaptureConfig) -> Result<(), String>;
    async fn save_clip(&mut self) -> Result<PathBuf, String>;
    async fn stop_buffer(&mut self) -> Result<(), String>;
    fn backend_name(&self) -> &'static str;
    /// The exact `-a` audio args the running engine spawned with (for stream matching).
    fn audio_args(&self) -> Vec<String> {
        vec![]
    }
    /// Downscale to apply at save time (None = deliver as captured).
    fn save_plan(&self) -> Option<SavePlan> {
        None
    }
    /// Engine liveness. Native backends own their threads and default to true;
    /// subprocess backends report whether the child is still running.
    fn check_alive(&mut self) -> bool {
        true
    }
    /// Bounded backend log tail (last lines) for diagnostics after a failure.
    fn log_tail(&self) -> Vec<String> {
        vec![]
    }
}

/// One capture device from `--list-audio-devices` (or OS enumeration).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AudioDevice {
    pub id: String,
    pub description: String,
    /// "mic" or "desktop"
    pub kind: String,
}

/// Video encoder id for save-time transcoding, chosen per GPU vendor.
/// Unknown vendors fall back to `None` (caller keeps the source file).
/// `X264` is the software CPU encoder — vendor-independent, always offered
/// on Windows as a fallback when no GPU block fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TranscodeEncoder {    Nvenc,
    Amf,
    Qsv,
    X264,
}

impl TranscodeEncoder {
    /// FFmpeg encoder name for `codec` (`h264`/`hevc`/`av1`/`x264`).
    pub fn ffmpeg_name(self, codec: &str) -> Option<&'static str> {
        match (self, codec) {
            (Self::Nvenc, "hevc") => Some("hevc_nvenc"),
            (Self::Nvenc, "av1") => Some("av1_nvenc"),
            (Self::Nvenc, _) => Some("h264_nvenc"),
            (Self::Amf, "hevc") => Some("hevc_amf"),
            (Self::Amf, _) => Some("h264_amf"),
            (Self::Qsv, "hevc") => Some("hevc_qsv"),
            (Self::Qsv, _) => Some("h264_qsv"),
            (Self::X264, "x264") => Some("libx264"),
            (Self::X264, _) => None,
        }
    }
}
