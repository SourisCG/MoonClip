//! Shared capture contract (OS-independent).
//! Every backend under os/linux and os/windows implements this trait.
//! Mirrors the OBS model: uniform interface, per-OS files behind it.
//!
//! MoonClip V3 engine: both platforms drive the embedded, fully isolated OBS
//! Studio (portable config dir + obs-websocket controlled in-process through
//! the `obws` crate). The trait below is the only surface shared code
//! (commands.rs) sees.

use std::path::PathBuf;

/// Custom encoder selection for video_mode=custom (None = ladder recipe).
/// `encoder` is an exact OBS encoder id from the pinned catalog;
/// `settings` is the validated user map (`obsopt.*` keys).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CustomEncoder {
    pub encoder: String,
    pub settings: serde_json::Map<String, serde_json::Value>,
}

/// Custom [Video] tab for video_mode=custom (None = ladder video).
/// `fps_type`: "common" | "integer" | "fractional".
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CustomVideo {
    pub out_width: u32,
    pub out_height: u32,
    pub scale_type: String,
    pub fps_type: String,
    pub fps_common: u32,
    pub fps_int: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub color_format: String,
    pub color_space: String,
    pub color_range: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaptureConfig {
    pub duration_seconds: u32,
    pub fps: u32,
    pub output_dir: PathBuf,
    /// App codec id: h264 | hevc | av1.
    pub codec: String,
    /// Encoder preference: `gpu` (hardware) or `cpu` (software x264).
    pub encoder: String,
    /// GPU index for hardware encoders (0 = default/auto in OBS).
    pub gpu_index: u32,
    /// CBR bitrate in kbps for the replay buffer.
    pub bitrate_kbps: u32,
    /// Delivered height (0 = source). OBS scales on the GPU.
    pub out_height: u32,
    /// Monitor selector (platform id/name). Empty = primary/portal.
    pub monitor: String,
    /// Optional window capture target (empty = monitor capture). For the
    /// portal route it only marks "capture a window"; the real target is the
    /// per-game restore token. Windows passes the title/class here (WGC).
    pub window: String,
    /// X11/XWayland `xcomposite_input` match (`id\r\nname\r\nclass`):
    /// when set, the window is captured with no portal dialog.
    pub window_match: Option<String>,
    /// Game/desktop audio device id ("default_output" = OS default).
    pub desktop_device: String,
    /// Microphone device id ("default_input" = OS default).
    pub mic_device: String,
    /// Per-track capture gain (0-200 %). Applied to the OBS source volume.
    pub gain_game: u32,
    pub gain_mic: u32,
    pub mute_game: bool,
    pub mute_mic: bool,
    /// Compatibility mode: record only the Mix track (1 instead of 3).
    pub audio_single_track: bool,
    /// Output container: `mp4` (default) or `mkv`.
    pub container: String,
    /// GPU vendor slug (nvidia/amd/intel/unknown), resolved by the caller.
    pub vendor: String,
    /// Base (canvas) resolution of the capture source. When `out_height == 0`
    /// the output resolution equals the base.
    pub base_width: u32,
    pub base_height: u32,
    /// Portal ScreenCast restore token (Wayland): pre-seeded into the capture
    /// source so OBS restores the screen session without the picker dialog.
    pub portal_restore_token: String,
    /// Custom encoder selection (video_mode=custom). None = ladder recipe.
    pub custom_encoder: Option<CustomEncoder>,
    /// Custom [Video] tab (video_mode=custom). None = ladder video.
    pub custom_video: Option<CustomVideo>,
    /// Resolved embedded OBS binary (None = resolve via bundle/PATH).
    pub obs_bin: Option<PathBuf>,
    /// obs-websocket port for MoonClip's private OBS instance.
    pub websocket_port: u16,
    /// obs-websocket password (hex, generated once and persisted).
    pub websocket_password: String,
}

/// Unified engine interface. Methods are async to allow signal waits / IPC.
#[allow(async_fn_in_trait)]
pub trait CaptureEngine: Send + Sync {
    async fn start_buffer(&mut self, config: CaptureConfig) -> Result<(), String>;
    async fn save_clip(&mut self) -> Result<PathBuf, String>;
    async fn stop_buffer(&mut self) -> Result<(), String>;
    /// Audio tracks the running configuration records (1 or 3).
    fn tracks_linked(&self) -> usize {
        0
    }
    /// Engine liveness (child process still running).
    fn check_alive(&mut self) -> bool {
        true
    }
    /// Bounded engine activity tail (last events) for diagnostics and the
    /// Settings activity panel. Never the upstream engine's own log.
    fn events_tail(&self) -> Vec<String> {
        vec![]
    }
    /// Record one diagnostic event in the engine activity ring (no-op for
    /// backends without a ring).
    fn note(&self, _msg: &str) {}
    /// Live mute toggle for one capture track ("game" | "mic"). Backends that
    /// cannot apply it live return an error (the setting still persists).
    async fn set_mute(&mut self, _track: &str, _muted: bool) -> Result<(), String> {
        Err("live mute not supported by this backend".into())
    }
    /// Live gain for one capture track, 0-200 %. Backends that cannot apply
    /// it live return an error (the setting still persists and applies on
    /// the next start).
    async fn set_volume(&mut self, _track: &str, _percent: u32) -> Result<(), String> {
        Err("live volume not supported by this backend".into())
    }
}

/// One capture device from OS enumeration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AudioDevice {
    pub id: String,
    pub description: String,
    /// "mic" or "desktop"
    pub kind: String,
}
