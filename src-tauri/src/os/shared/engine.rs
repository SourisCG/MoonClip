//! Embedded OBS Studio engine (shared, OS-free).
//!
//! MoonClip drives its OWN OBS Studio installation, fully isolated from any
//! OBS the user may have installed:
//!   - config lives in a MoonClip-owned directory (Windows: portable copy;
//!     Linux: `XDG_CONFIG_HOME`), never in `%APPDATA%/obs-studio` or
//!     `~/.config/obs-studio`
//!   - the profile and scene collection are generated names ("MoonClip"), so
//!     even a shared obs-websocket port can never touch the user's setup
//!   - obs-websocket binds 127.0.0.1 on a dedicated port with a generated
//!     password; only MoonClip's in-process `obws` client talks to it
//!
//! Capture is replay-buffer only. The engine writes the profile (basic.ini +
//! recordEncoder.json), the scene collection (Display capture + Game audio +
//! Mic audio, 3 audio tracks with Mix first), then launches OBS and controls
//! it over obs-websocket v5 (replay start/stop/save with flush polling,
//! scene check, inputs volume/mute, screenshots, video settings).
//!
//! ANTI-CHEAT (hard rule): the generated scene NEVER uses `game_capture`.
//! Game capture injects a DLL into the game process and is what kernel
//! anti-cheats (Vanguard, VAC, FACEIT, EAC, BattlEye) block or flag. Display
//! capture (DXGI/WGC on Windows, PipeWire portal on Linux) reads the
//! compositor output and never touches game memory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::{Child, Command};

use super::obsws::Obsws;
use super::super::{CaptureConfig, CaptureEngine};

// ---------------------------------------------------------------------------
// Identity constants (generated profile/scene — never the user's)
// ---------------------------------------------------------------------------

pub const OBS_PROFILE: &str = "MoonClip";
pub const OBS_COLLECTION: &str = "MoonClip";
pub const SCENE_NAME: &str = "MoonClip Capture";
/// The capture source name MUST differ from the scene name: OBS resolves the
/// active scene with `obs_get_source_by_name(current_scene)`, which returns
/// the FIRST source registered with that name. With both named the same, the
/// program scene resolved to the capture source, the program stayed empty and
/// the saved video was all black (observed live).
pub const VIDEO_SOURCE_NAME: &str = "MoonClip Screen";
pub const GAME_SOURCE_NAME: &str = "MoonClip Game Audio";
pub const MIC_SOURCE_NAME: &str = "MoonClip Mic";

/// Fixed UUID of libobs' main canvas (`6c69626f-6273-...` = "libobs-main").
/// Written on scene sources so OBS binds them explicitly to the program canvas.
pub const MAIN_CANVAS_UUID: &str = "6c69626f-6273-4c00-9d88-c5136d61696e";

/// Audio bitrates (§10 layout): Mix + Game 320k, Mic 192k.
pub const TRACK_BITRATE_MIX: u32 = 320;
pub const TRACK_BITRATE_GAME: u32 = 320;
pub const TRACK_BITRATE_MIC: u32 = 192;

/// `prev_ver` written into generated scene files (modern OBS encoding). It only
/// gates one-time migrations; a modern value avoids pointless rewrites.
const SCENE_PREV_VER: u64 = 536936450;

/// Hard rule: no game hooking, ever. Tests assert this string never appears in
/// any generated artifact.
pub const FORBIDDEN_SOURCE_ID: &str = "game_capture";

// ---------------------------------------------------------------------------
// Platform abstraction
// ---------------------------------------------------------------------------

/// A prepared, writable OBS runtime: the binary actually launched, the config
/// dir that will hold OUR `obs-studio/` folder, and any extra launch args.
/// Windows prepares a portable copy (portable_mode.txt) so OBS writes its
/// config inside MoonClip's data dir; Linux redirects the same tree with
/// `XDG_CONFIG_HOME` (see `obs_launch_env`). The user's OBS config is
/// untouchable either way.
#[derive(Debug, Clone)]
pub struct ObsRuntime {
    pub bin: PathBuf,
    pub config_root: PathBuf,
    pub extra_args: Vec<String>,
    /// Portable copy (Windows, true) vs env-redirected launch (Linux, false).
    pub portable: bool,
}

/// Everything the shared engine needs from the host OS. Implementations live
/// in `os/windows/obs.rs` and `os/linux/obs.rs`; selection only in `os/mod.rs`.
#[allow(async_fn_in_trait)]
pub trait ObsPlatform: Send + Sync {
    /// Ensure a writable portable OBS copy exists (or fall back to
    /// `--config-dir` for a system install) and return what to launch.
    /// MUST never write into the user's OBS config.
    fn prepare_runtime(&self, bundled_bin: &Path) -> Result<ObsRuntime, String>;
    /// CLI args after the binary path for an isolated launch.
    fn obs_launch_args(&self, config_root: &Path, profile: &str, collection: &str) -> Vec<String>;
    /// Extra environment for the OBS child. Linux redirects OBS's whole
    /// config tree with `XDG_CONFIG_HOME` (the user's config is untouchable).
    fn obs_launch_env(&self, _config_root: &Path) -> Vec<(String, String)> {
        vec![]
    }
    /// Working directory OBS expects (it locates `data/` relative to the exe).
    fn working_dir(&self, obs_bin: &Path) -> Option<PathBuf>;
    /// Per-command tweaks (Windows: hide the console window).
    fn configure(&self, _cmd: &mut Command) {}
    /// Tie the spawned OBS process lifetime to MoonClip (Windows: job object
    /// with KILL_ON_JOB_CLOSE) so a force-kill or crash never leaves it
    /// running. Best effort; implementations may no-op.
    fn adopt_child(&self, _pid: u32) {}
    /// Hide the OBS main window right after spawn so the user never sees
    /// it (no window, no tray). Best effort; must never touch other
    /// processes (filter by exact PID + image path) and never block startup.
    /// Implementations may no-op (Linux owner iterates).
    fn conceal_window(&self, _pid: u32, _exe: &Path) {}
    /// Undo `conceal_window` when the engine stops (Linux: unload the KWin
    /// script). Best effort; default no-op.
    fn unconceal_window(&self, _pid: u32) {}
    /// Whether this platform's OBS instance keeps its tray icon enabled.
    /// Windows disables it (the watcher hides the window instead); other
    /// platforms keep the tray path until they implement concealment.
    fn sys_tray_enabled(&self) -> bool {
        true
    }
    /// Kill leftover MoonClip OBS processes from a force-killed session.
    /// MUST only match our exact bundled binary path; never the user's OBS.
    fn kill_orphans(&self, obs_bin: &Path);
    /// OBS video source for the chosen monitor/window: (source id, settings).
    /// Implementations must never return the game-capture source.
    fn video_source(&self, monitor: &str, window: &str) -> (&'static str, serde_json::Value);
    /// OBS source ids for desktop (game) and microphone audio.
    fn game_audio_source_id(&self) -> &'static str;
    fn mic_audio_source_id(&self) -> &'static str;
    /// OBS encoder id for (vendor, codec, encoder preference).
    fn encoder_id(&self, vendor: &str, codec: &str, encoder: &str) -> Option<&'static str>;
    /// Encoder ids compiled into this platform's OBS build (Custom picker).
    fn encoder_catalog(&self) -> &'static [super::encoder_options::EncoderEntry];
}

// ---------------------------------------------------------------------------
// Resolved profile
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ObsProfile {
    pub encoder_id: String,
    /// App codec (h264/hevc/av1): resolved identity, asserted in tests.
    #[allow(dead_code)]
    pub codec: String,
    /// Exact map written to recordEncoder.json (ladder recipe or validated
    /// Custom settings).
    pub encoder_settings: serde_json::Map<String, serde_json::Value>,
    /// Full [Video] tab (ladder defaults or validated Custom video).
    pub scale_type: String,
    /// 0 = common values, 1 = integer, 2 = fractional.
    pub fps_type: u8,
    /// As-written FPSCommon (e.g. "60").
    pub fps_common: String,
    pub fps_int: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub color_format: String,
    pub color_space: String,
    pub color_range: String,
    /// GPU index for hardware encoders (kept on the config; the resolved
    /// `device` key lives in `encoder_settings`).
    #[allow(dead_code)]
    pub gpu_index: u32,
    pub fps: u32,
    pub base_width: u32,
    pub base_height: u32,
    pub out_width: u32,
    pub out_height: u32,
    /// Output follows the base (user chose "Source"): canvas learning keeps
    /// them equal; otherwise the explicit ladder/custom height is preserved.
    pub out_source: bool,
    pub bitrate_kbps: u32,
    pub replay_seconds: u32,
    pub replay_mb: u32,
    pub output_dir: PathBuf,
    pub container: String,
    pub desktop_device: String,
    pub mic_device: String,
    pub gain_game: u32,
    pub gain_mic: u32,
    pub mute_game: bool,
    pub mute_mic: bool,
    pub single_track: bool,
    pub video_source_id: String,
    pub game_audio_id: String,
    pub mic_audio_id: String,
    pub video_settings: serde_json::Value,
    pub websocket_port: u16,
    pub websocket_password: String,
}

fn even(v: u32) -> u32 {
    v.max(2) & !1
}

/// Preserve the base aspect for the requested output height.
pub fn scaled_width(base_w: u32, base_h: u32, out_h: u32) -> u32 {
    if base_h == 0 || base_w == 0 {
        return 1920;
    }
    even(((base_w as f64 * out_h as f64 / base_h as f64).round() as u32).max(2))
}

/// Exact replay-ring megabytes for a CBR bitrate, with headroom.
pub fn replay_mb(bitrate_kbps: u32, seconds: u32) -> u32 {
    let raw = (bitrate_kbps as u64 * seconds as u64) / 8 / 1024;
    let with_margin = raw + raw / 2 + 64;
    with_margin.clamp(128, 16 * 1024) as u32
}

/// OBS WASAPI/Pulse sources take the endpoint/device name or the literal
/// `default`. Legacy app values (`default_output`/`default_input`) and blank
/// strings MUST be normalized or OBS fails to enumerate (`0x80070057`).
pub fn normalize_device_id(stored: &str, render: bool) -> String {
    let t = stored.trim();
    if t.is_empty()
        || t == "default"
        || (render && t == "default_output")
        || (!render && t == "default_input")
    {
        "default".to_string()
    } else {
        t.to_string()
    }
}

impl ObsProfile {
    pub fn from_config(cfg: &CaptureConfig, platform: &dyn ObsPlatform) -> Result<Self, String> {
        use super::encoder_options as enc;
        let base_w = even(cfg.base_width.max(2));
        let base_h = even(cfg.base_height.max(2));

        // Encoder id + codec first (explicit Custom id from this
        // platform's catalog, or the ladder vendor mapping).
        let (encoder_id, codec) = match &cfg.custom_encoder {
            Some(custom) => {
                let entry = platform
                    .encoder_catalog()
                    .iter()
                    .find(|e| e.id == custom.encoder)
                    .copied()
                    .ok_or_else(|| format!("encoder '{}' not in this OBS build", custom.encoder))?;
                (entry.id.to_string(), entry.codec.to_string())
            }
            None => {
                let codec = match cfg.codec.as_str() {
                    "h264" | "hevc" | "av1" => cfg.codec.clone(),
                    other => return Err(format!("unsupported codec '{other}'")),
                };
                let encoder_id = platform
                    .encoder_id(&cfg.vendor, &codec, &cfg.encoder)
                    .ok_or_else(|| {
                        format!(
                            "no OBS encoder for codec '{}' / '{}' on '{}'",
                            codec, cfg.encoder, cfg.vendor
                        )
                    })?
                    .to_string();
                (encoder_id, codec)
            }
        };
        let codec_family =
            enc::family_of(&encoder_id).ok_or_else(|| format!("unknown encoder '{encoder_id}'"))?;

        // Video tab: validated Custom overrides or ladder defaults. The
        // color format gates 10-bit encoder profiles in the pair check.
        let custom_video = match &cfg.custom_video {
            Some(v) => Some(enc::parse_custom_video(
                codec_family,
                &codec,
                &serde_json::to_string(v).map_err(|e| e.to_string())?,
            )?),
            None => None,
        };
        let color = custom_video.as_ref().map(|v| v.color_format.as_str());

        // Encoder settings: validated Custom map (pair-checked against the
        // video color) or the ladder recipe for this encoder.
        let encoder_settings = match &cfg.custom_encoder {
            Some(custom) => enc::validate_pair(codec_family, &codec, &custom.settings, color)?,
            None => {
                let bitrate = cfg.bitrate_kbps.clamp(1_000, 200_000);
                enc::ladder_recipe(&encoder_id, bitrate, cfg.gpu_index, &codec)
                    .ok_or_else(|| format!("no recipe for encoder '{encoder_id}'"))?
            }
        };

        // Bitrate for the replay-ring math: explicit Custom `bitrate` when
        // the user set one, else the ladder value.
        let bitrate = match encoder_settings.get("bitrate").and_then(|v| v.as_u64()) {
            Some(b) => (b as u32).clamp(1_000, 200_000),
            None => cfg.bitrate_kbps.clamp(1_000, 200_000),
        };
        let seconds = cfg.duration_seconds.clamp(1, 3600);

        // Video tab: validated Custom overrides or ladder defaults.
        let out_source = cfg.custom_video.is_none() && (cfg.out_height == 0 || cfg.out_height >= base_h);
        let (
            out_w,
            out_h,
            scale_type,
            fps_type,
            fps_common,
            fps_int,
            fps_num,
            fps_den,
            color_format,
            color_space,
            color_range,
            fps,
        ) = match custom_video {
            Some(vv) => {
                let fps = enc::custom_effective_fps(&vv);
                (
                    vv.out_width,
                    vv.out_height,
                    vv.scale_type,
                    match vv.fps_type.as_str() {
                        "integer" => 1,
                        "fractional" => 2,
                        _ => 0,
                    },
                    vv.fps_common.to_string(),
                    vv.fps_int,
                    vv.fps_num,
                    vv.fps_den,
                    vv.color_format,
                    vv.color_space,
                    vv.color_range,
                    fps,
                )
            }
            None => {
                let (out_w, out_h) = if cfg.out_height == 0 || cfg.out_height >= base_h {
                    (base_w, base_h)
                } else {
                    let h = even(cfg.out_height);
                    (scaled_width(base_w, base_h, h), h)
                };
                (
                    out_w,
                    out_h,
                    "bicubic".to_string(),
                    0,
                    cfg.fps.clamp(1, 360).to_string(),
                    60,
                    60,
                    1,
                    "NV12".to_string(),
                    "709".to_string(),
                    "Partial".to_string(),
                    cfg.fps.clamp(1, 360),
                )
            }
        };

        let (video_source_id, mut video_settings) =
            platform.video_source(&cfg.monitor, &cfg.window);
        // Portal restore token (Wayland): pre-seed it so OBS restores the
        // screen session silently instead of showing the picker again. OBS
        // refreshes the token on every successful Start; the caller reads it
        // back and persists the newest one.
        if !cfg.portal_restore_token.trim().is_empty() {
            if let serde_json::Value::Object(map) = &mut video_settings {
                map.insert(
                    "RestoreToken".to_string(),
                    serde_json::Value::String(cfg.portal_restore_token.clone()),
                );
            }
        }
        Ok(Self {
            encoder_id,
            codec,
            encoder_settings,
            scale_type,
            fps_type,
            fps_common,
            fps_int,
            fps_num,
            fps_den,
            color_format,
            color_space,
            color_range,
            gpu_index: cfg.gpu_index,
            fps,
            base_width: base_w,
            base_height: base_h,
            out_width: out_w,
            out_height: out_h,
            out_source,
            bitrate_kbps: bitrate,
            replay_seconds: seconds,
            replay_mb: replay_mb(bitrate, seconds),
            output_dir: cfg.output_dir.clone(),
            container: if cfg.container == "mkv" {
                "mkv".into()
            } else {
                "mp4".into()
            },
            desktop_device: normalize_device_id(&cfg.desktop_device, true),
            mic_device: normalize_device_id(&cfg.mic_device, false),
            gain_game: cfg.gain_game.clamp(0, 200),
            gain_mic: cfg.gain_mic.clamp(0, 200),
            mute_game: cfg.mute_game,
            mute_mic: cfg.mute_mic,
            single_track: cfg.audio_single_track,
            video_source_id: video_source_id.to_string(),
            game_audio_id: platform.game_audio_source_id().to_string(),
            mic_audio_id: platform.mic_audio_source_id().to_string(),
            video_settings,
            websocket_port: cfg.websocket_port,
            websocket_password: cfg.websocket_password.clone(),
        })
    }

    /// Audio track bitmask (OBS `RecTracks`): 3 tracks or Mix only.
    pub fn rec_tracks(&self) -> u32 {
        if self.single_track {
            1
        } else {
            1 | 2 | 4
        }
    }

    pub fn tracks_linked(&self) -> usize {
        if self.single_track {
            1
        } else {
            3
        }
    }
}

// ---------------------------------------------------------------------------
// Profile / scene writers (pure — unit tested)
// ---------------------------------------------------------------------------

/// `basic.ini` for the generated MoonClip profile. Advanced output + replay
/// buffer + up to 3 AAC tracks; all MoonClip-owned.
pub fn render_basic_ini(p: &ObsProfile) -> String {
    let tracks = p.rec_tracks();
    let mut s = String::new();
    s.push_str("[General]\nName=MoonClip\n\n");
    s.push_str("[Output]\nMode=Advanced\n\n");
    s.push_str("[AdvOut]\n");
    s.push_str("RecType=Standard\n");
    s.push_str(&format!("RecFilePath={}\n", p.output_dir.display()));
    s.push_str(&format!("RecFormat2={}\n", p.container));
    s.push_str(&format!("RecEncoder={}\n", p.encoder_id));
    s.push_str("RecAudioEncoder=ffmpeg_aac\n");
    s.push_str("RecRB=true\n");
    s.push_str(&format!("RecRBTime={}\n", p.replay_seconds));
    s.push_str(&format!("RecRBSize={}\n", p.replay_mb));
    s.push_str(&format!("RecTracks={tracks}\n"));
    s.push_str("VodTrackEnabled=false\n");
    s.push_str("TrackIndex=1\n");
    s.push_str("VodTrackIndex=2\n");
    s.push_str("ApplyServiceSettings=false\n");
    s.push_str(&format!("Track1Bitrate={TRACK_BITRATE_MIX}\n"));
    s.push_str(&format!("Track2Bitrate={TRACK_BITRATE_GAME}\n"));
    s.push_str(&format!("Track3Bitrate={TRACK_BITRATE_MIC}\n"));
    s.push_str("Track1Name=Master Mix [Game+Voice]\n");
    s.push_str("Track2Name=Game/Desktop\n");
    s.push_str("Track3Name=Microphone\n\n");
    s.push_str("[Video]\n");
    s.push_str(&format!("BaseCX={}\n", p.base_width));
    s.push_str(&format!("BaseCY={}\n", p.base_height));
    s.push_str(&format!("OutputCX={}\n", p.out_width));
    s.push_str(&format!("OutputCY={}\n", p.out_height));
    // FPSType 0 = common values, 1 = integer, 2 = fractional.
    s.push_str(&format!("FPSType={}\n", p.fps_type));
    match p.fps_type {
        1 => s.push_str(&format!("FPSInt={}\n", p.fps_int)),
        2 => {
            s.push_str(&format!("FPSNum={}\n", p.fps_num));
            s.push_str(&format!("FPSDen={}\n", p.fps_den));
        }
        _ => s.push_str(&format!("FPSCommon={}\n", p.fps_common)),
    }
    s.push_str(&format!("ScaleType={}\n", p.scale_type));
    s.push_str(&format!("ColorFormat={}\n", p.color_format));
    s.push_str(&format!("ColorSpace={}\n", p.color_space));
    s.push_str(&format!("ColorRange={}\n\n", p.color_range));
    s.push_str("[Audio]\n");
    s.push_str("SampleRate=48000\n");
    s.push_str("ChannelSetup=Stereo\n");
    s.push_str("MonitoringDeviceId=default\n");
    s
}

/// `recordEncoder.json` for the selected encoder: the validated settings map
/// on the profile (ladder recipe or Custom). OBS 32.2.2 silently ignores
/// unknown keys, so every key here comes from the encoder_options registry.
pub fn render_record_encoder_json(p: &ObsProfile) -> String {
    serde_json::to_string_pretty(&serde_json::Value::Object(p.encoder_settings.clone()))
        .unwrap_or_else(|_| "{}".into())
}

/// OBS volume multiplier from a 0-200 % gain (1.0 = unity).
pub fn gain_to_volume(pct: u32) -> f64 {
    (pct.min(200) as f64) / 100.0
}

/// Stable UUIDs for one generated collection (fresh per start is fine; OBS
/// rewrites them when it saves).
#[derive(Debug, Clone)]
pub struct SourceUuids {
    pub scene: String,
    pub video: String,
    pub game_audio: String,
    pub mic_audio: String,
}

impl SourceUuids {
    pub fn generate() -> Self {
        Self {
            scene: uuid::Uuid::new_v4().to_string(),
            video: uuid::Uuid::new_v4().to_string(),
            game_audio: uuid::Uuid::new_v4().to_string(),
            mic_audio: uuid::Uuid::new_v4().to_string(),
        }
    }
}

/// One OBS source entry with the common defaults.
fn source_entry(
    uuid: &str,
    name: &str,
    id: &str,
    settings: serde_json::Value,
    mixers: u32,
    volume: f64,
    muted: bool,
) -> serde_json::Value {
    serde_json::json!({
        "prev_ver": SCENE_PREV_VER,
        "name": name,
        "uuid": uuid,
        "id": id,
        "versioned_id": id,
        "settings": settings,
        "mixers": mixers,
        "sync": 0,
        "flags": 0,
        "volume": volume,
        "balance": 0.5,
        "enabled": true,
        "muted": muted,
        "push-to-mute": false,
        "push-to-mute-delay": 0,
        "push-to-talk": false,
        "push-to-talk-delay": 0,
        "hotkeys": {},
        "deinterlace_mode": 0,
        "deinterlace_field_order": 0,
        "monitoring_type": 0,
        "private_settings": {}
    })
}

/// Generated scene collection. Anti-cheat invariant: the only video source id
/// is the platform display source; `game_capture` must never appear.
pub fn render_collection(p: &ObsProfile, uuids: &SourceUuids) -> String {
    use serde_json::json;
    let game_mixers = if p.single_track { 1 } else { 1 | 2 };
    let mic_mixers = if p.single_track { 1 } else { 1 | 4 };
    let scene_item = json!({
        "name": VIDEO_SOURCE_NAME,
        "source_uuid": uuids.video,
        "visible": true,
        "locked": false,
        "rot": 0.0,
        "pos": {"x": 0.0, "y": 0.0},
        "scale": {"x": 1.0, "y": 1.0},
        "align": 5,
        "bounds_type": 0,
        "bounds_align": 0,
        "bounds_crop": false,
        "bounds": {"x": 0.0, "y": 0.0},
        "crop_left": 0,
        "crop_top": 0,
        "crop_right": 0,
        "crop_bottom": 0,
        "id": 1,
        "group_item_backup": false,
        "scale_filter": "disable",
        "blend_method": "default",
        "blend_type": "normal",
        "show_transition": {"duration": 0},
        "hide_transition": {"duration": 0},
        "private_settings": {}
    });
    let scene = json!({
        "prev_ver": SCENE_PREV_VER,
        "name": SCENE_NAME,
        "uuid": uuids.scene,
        "id": "scene",
        "versioned_id": "scene",
        "settings": {"id_counter": 2, "custom_size": false, "items": [scene_item]},
        "mixers": 0,
        "sync": 0,
        "flags": 0,
        "volume": 1.0,
        "balance": 0.5,
        "enabled": true,
        "muted": false,
        "push-to-mute": false,
        "push-to-mute-delay": 0,
        "push-to-talk": false,
        "push-to-talk-delay": 0,
        "hotkeys": {},
        "deinterlace_mode": 0,
        "deinterlace_field_order": 0,
        "monitoring_type": 0,
        "canvas_uuid": MAIN_CANVAS_UUID,
        "private_settings": {}
    });
    let video_source = source_entry(
        &uuids.video,
        VIDEO_SOURCE_NAME,
        &p.video_source_id,
        p.video_settings.clone(),
        0,
        1.0,
        false,
    );
    let game_audio = source_entry(
        &uuids.game_audio,
        GAME_SOURCE_NAME,
        &p.game_audio_id,
        json!({"device_id": p.desktop_device}),
        game_mixers,
        gain_to_volume(p.gain_game),
        p.mute_game,
    );
    let mic_audio = source_entry(
        &uuids.mic_audio,
        MIC_SOURCE_NAME,
        &p.mic_audio_id,
        json!({"device_id": p.mic_device}),
        mic_mixers,
        gain_to_volume(p.gain_mic),
        p.mute_mic,
    );
    let doc = json!({
        "name": OBS_COLLECTION,
        "current_scene": SCENE_NAME,
        "current_program_scene": SCENE_NAME,
        "scene_order": [{"name": SCENE_NAME}],
        "sources": [scene, video_source],
        "groups": [],
        "quick_transitions": [
            {"name": "Cut", "duration": 300, "hotkeys": [], "id": 1, "fade_to_black": false},
            {"name": "Fade", "duration": 300, "hotkeys": [], "id": 2, "fade_to_black": false},
            {"name": "Fade", "duration": 300, "hotkeys": [], "id": 3, "fade_to_black": true}
        ],
        "transitions": [],
        "saved_projectors": [],
        "current_transition": "Fade",
        "transition_duration": 300,
        "preview_locked": false,
        "scaling_enabled": false,
        "scaling_level": 0,
        "scaling_off_x": 0.0,
        "scaling_off_y": 0.0,
        "AuxAudioDevice1": game_audio,
        "AuxAudioDevice2": mic_audio,
        "modules": {}
    });
    let mut text = serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".into());
    // Belt and suspenders: never ship a scene that can hook a game.
    debug_assert!(!text.contains(FORBIDDEN_SOURCE_ID));
    if text.contains(FORBIDDEN_SOURCE_ID) {
        text = "{}".to_string();
    }
    text
}

/// `global.ini`: select the generated profile/collection before OBS starts.
pub fn render_global_ini() -> String {
    let mut s = String::new();
    s.push_str("[Basic]\n");
    s.push_str(&format!("Profile={OBS_PROFILE}\n"));
    s.push_str(&format!("ProfileDir={OBS_PROFILE}\n"));
    s.push_str(&format!("SceneCollection={OBS_COLLECTION}\n"));
    s.push_str(&format!("SceneCollectionFile={OBS_COLLECTION}\n"));
    s
}

/// `user.ini`: suppress OBS's first-run Auto-Configuration Wizard. Without
/// `FirstRun=true`, OBS 32 queues `on_autoConfigure_triggered` on the first
/// portable start, which pops a "configure OBS for recording" dialog and can
/// break/undo a replay-buffer start while the wizard is pending.
/// `tray`: keep OBS's own tray icon (Linux fallback until the window can be
/// hidden there); Windows disables it and hides the window instead so the
/// user never sees the embedded instance.
pub fn render_user_ini(tray: bool) -> String {
    let mut s = String::from("[General]\nFirstRun=true\n");
    if !tray {
        s.push_str(
            "\n[BasicWindow]\nSysTrayEnabled=false\nSysTrayWhenStarted=false\nSysTrayMinimizeToTray=false\n",
        );
    }
    s
}

/// `plugin_config/obs-websocket/config.json` for OUR instance only.
pub fn render_websocket_config(port: u16, password: &str) -> String {
    use serde_json::json;
    serde_json::to_string_pretty(&json!({
        "alerts_enabled": false,
        "auth_required": true,
        "first_load": false,
        "server_enabled": true,
        "server_port": port,
        "server_password": password,
        "debug_enabled": false
    }))
    .unwrap_or_else(|_| "{}".into())
}


/// Write every generated file for one start. Returns the profile dir.
pub fn write_obs_config(
    root: &Path,
    p: &ObsProfile,
    tray_enabled: bool,
) -> Result<PathBuf, String> {
    let conf = root.join("obs-studio");
    let profile_dir = conf.join("basic").join("profiles").join(OBS_PROFILE);
    let scenes_dir = conf.join("basic").join("scenes");
    let ws_dir = conf.join("plugin_config").join("obs-websocket");
    for dir in [&profile_dir, &scenes_dir, &ws_dir] {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create OBS config dir {}: {e}", dir.display()))?;
    }
    std::fs::create_dir_all(&p.output_dir).map_err(|e| format!("cannot create clips dir: {e}"))?;
    let write = |path: PathBuf, text: String| -> Result<(), String> {
        std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
    };
    let uuids = SourceUuids::generate();
    // Preserve the portal RestoreToken OBS saved in the previous collection:
    // it refreshes the single-use token on every successful Start, so losing
    // it (force-kill before the DB read-back) would show the picker again.
    let collection_path = scenes_dir.join(format!("{OBS_COLLECTION}.json"));
    let mut profile = p.clone();
    let has_token = profile
        .video_settings
        .get("RestoreToken")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if !has_token {
        if let Some(token) = existing_restore_token(&collection_path) {
            if let serde_json::Value::Object(map) = &mut profile.video_settings {
                map.insert(
                    "RestoreToken".to_string(),
                    serde_json::Value::String(token),
                );
            }
        }
    }
    write(profile_dir.join("basic.ini"), render_basic_ini(p))?;
    write(
        profile_dir.join("recordEncoder.json"),
        render_record_encoder_json(p),
    )?;
    write(collection_path, render_collection(&profile, &uuids))?;
    write(conf.join("global.ini"), render_global_ini())?;
    write(conf.join("user.ini"), render_user_ini(tray_enabled))?;
    write(
        ws_dir.join("config.json"),
        render_websocket_config(p.websocket_port, &p.websocket_password),
    )?;
    Ok(profile_dir)
}

/// Remove the MoonClip-owned OBS config (Repair button). Only ever touches the
/// passed root.
pub fn reset_obs_config(root: &Path) -> Result<(), String> {
    if root.exists() {
        std::fs::remove_dir_all(root)
            .map_err(|e| format!("cannot reset OBS config {}: {e}", root.display()))?;
    }
    Ok(())
}

/// Bounded tail of the newest OBS log inside OUR config dir (diagnostics).
pub fn read_obs_log_tail(root: &Path) -> Vec<String> {
    let logs = root.join("obs-studio").join("logs");
    let Ok(entries) = std::fs::read_dir(&logs) else {
        return vec![];
    };
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("txt") {
            continue;
        }
        if let Ok(modified) = e.metadata().and_then(|m| m.modified()) {
            if newest.as_ref().map(|(t, _)| modified > *t).unwrap_or(true) {
                newest = Some((modified, p));
            }
        }
    }
    let Some((_, path)) = newest else {
        return vec![];
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return vec![];
    };
    let mut tail: Vec<String> = text.lines().rev().take(40).map(|s| s.to_string()).collect();
    tail.reverse();
    tail
}

/// Portal `RestoreToken` saved by OBS in our own scene collection file.
/// OBS refreshes the single-use token on every successful screencast Start and
/// persists it into the source settings; reading it back here keeps restores
/// silent even after a force-kill that skipped the live read-back.
pub fn existing_restore_token(collection_path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(collection_path).ok()?;
    let doc: serde_json::Value = serde_json::from_str(&text).ok()?;
    doc.get("sources")?
        .as_array()?
        .iter()
        .find_map(|s| {
            let id = s.get("id")?.as_str()?;
            if id == "pipewire-desktop-capture-source" || id == "pipewire-screen-capture-source" {
                s.get("settings")?
                    .get("RestoreToken")?
                    .as_str()
                    .map(|t| t.to_string())
            } else {
                None
            }
        })
        .filter(|t| !t.is_empty())
}

/// Recursive directory copy used to stage the writable portable OBS runtime.
#[allow(dead_code)] // used by the Windows backend + staging tests
pub fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("cannot create {}: {e}", dst.display()))?;
    let entries =
        std::fs::read_dir(src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", src.display()))?;
        let path = entry.path();
        let to = dst.join(entry.file_name());
        let ty = entry
            .file_type()
            .map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
        if ty.is_dir() {
            copy_dir_recursive(&path, &to)?;
        } else if ty.is_file() {
            std::fs::copy(&path, &to)
                .map_err(|e| format!("cannot copy {} -> {}: {e}", path.display(), to.display()))?;
        } else if ty.is_symlink() {
            let target = std::fs::canonicalize(&path)
                .map_err(|e| format!("cannot resolve symlink {}: {e}", path.display()))?;
            if target.is_dir() {
                copy_dir_recursive(&target, &to)?;
            } else {
                std::fs::copy(&target, &to).map_err(|e| {
                    format!("cannot copy {} -> {}: {e}", target.display(), to.display())
                })?;
            }
        }
    }
    Ok(())
}

/// Fingerprint of the OBS build the runtime copy was staged from: canonical
/// exe size + mtime. A newer bundled OBS re-stages the copy.
#[allow(dead_code)] // used by the Windows backend + staging tests
pub fn obs_build_fingerprint(exe: &Path) -> String {
    let canonical = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    let meta = std::fs::metadata(exe).ok();
    let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{}\n{len}\n{mtime}\n", canonical.display())
}

/// Does the existing runtime marker match the bundled OBS fingerprint?
#[allow(dead_code)] // used by the Windows backend + staging tests
pub fn marker_matches(marker: &Path, fingerprint: &str) -> bool {
    std::fs::read_to_string(marker)
        .map(|s| s == fingerprint)
        .unwrap_or(false)
}

#[allow(dead_code)] // used by the Windows backend + staging tests
pub fn write_marker(marker: &Path, fingerprint: &str) -> Result<(), String> {
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(marker, fingerprint)
        .map_err(|e| format!("cannot write {}: {e}", marker.display()))
}

/// PNG width/height from the IHDR chunk (no image-crate dependency).
pub fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[0..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// Generated audio source name for a mixer track.
pub fn volume_source(track: &str) -> Option<&'static str> {
    match track {
        "game" => Some(GAME_SOURCE_NAME),
        "mic" => Some(MIC_SOURCE_NAME),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// How long `start_buffer` waits for OBS to accept websocket connections.
const OBS_BOOT_TIMEOUT: Duration = Duration::from_secs(45);
/// Poll interval for the boot wait and the config guard.
const OBS_BOOT_POLL: Duration = Duration::from_millis(400);
/// If OBS does not create `<root>/obs-studio` in this window, it is not using
/// our isolated config: kill it (never risk mixing with the user's OBS).
const CONFIG_GUARD_TIMEOUT: Duration = Duration::from_secs(12);

pub struct ObsEngine {
    platform: Box<dyn ObsPlatform>,
    child: Option<Child>,
    obsws: Option<Obsws>,
    profile: Option<ObsProfile>,
    config_root: PathBuf,
    error: Option<String>,
}

impl ObsEngine {
    pub fn new(platform: Box<dyn ObsPlatform>) -> Self {
        Self {
            platform,
            child: None,
            obsws: None,
            profile: None,
            config_root: PathBuf::new(),
            error: None,
        }
    }

    fn obsws(&self) -> Result<&Obsws, String> {
        self.obsws
            .as_ref()
            .ok_or_else(|| "obs-websocket not connected".to_string())
    }

    /// Abort a failed start: stop what we spawned and enrich the message with
    /// the last interesting OBS log line (the QMessageBox reason is not
    /// written to the log, but encoder/output errors are).
    async fn fail_start(&mut self, msg: String) -> String {
        self.error = Some(msg.clone());
        self.stop_buffer().await.ok();
        let tail = read_obs_log_tail(&self.config_root);
        let hint = tail
            .iter()
            .rev()
            .find(|line| {
                let l = line.to_lowercase();
                l.contains("error") || l.contains("failed") || l.contains("warning")
            })
            .cloned();
        match hint {
            Some(l) => format!("{msg} | OBS: {l}"),
            None => msg,
        }
    }

    /// Wait until OUR config dir exists, proving OBS is using the portable
    /// copy's config (never the user's OBS config).
    async fn wait_for_config_guard(&self) -> Result<(), String> {
        let marker = self.config_root.join("obs-studio");
        let deadline = std::time::Instant::now() + CONFIG_GUARD_TIMEOUT;
        while std::time::Instant::now() < deadline {
            if marker.exists() {
                return Ok(());
            }
            tokio::time::sleep(OBS_BOOT_POLL).await;
        }
        Err(format!(
            "OBS did not create its isolated config ({}); refusing to run so the user's OBS config is never touched",
            marker.display()
        ))
    }
    /// Portal restore token OBS persisted on the capture source, if any.
    /// OBS refreshes it on every successful Start (single-use tokens), so the
    /// caller persists the newest one after each start.
    pub async fn read_restore_token(&self) -> Option<String> {
        let obsws = self.obsws.as_ref()?;
        let settings = obsws.input_settings(VIDEO_SOURCE_NAME).await.ok()?;
        settings
            .get("RestoreToken")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    }

    /// Learn the real captured size (the portal decides it, not our profile)
    /// and resize canvas/output when it differs. Returns the size on success.
    pub async fn detect_and_apply_source_size(&mut self) -> Option<(u32, u32)> {
        let (old_bw, old_bh, old_out_h, out_source) = {
            let p = self.profile.as_ref()?;
            (p.base_width, p.base_height, p.out_height, p.out_source)
        };
        let shot = std::env::temp_dir().join(format!("moonclip-source-{}.png", std::process::id()));
        let obsws = self.obsws.as_ref()?;
        // Wait until the portal stream is really producing frames before
        // probing (an inactive source has no size yet).
        if !obsws.source_active(VIDEO_SOURCE_NAME).await.unwrap_or(false) {
            return None;
        }
        obsws.save_screenshot(VIDEO_SOURCE_NAME, &shot).await.ok()?;
        let bytes = std::fs::read(&shot).ok()?;
        let _ = std::fs::remove_file(&shot);
        let (w, h) = png_dimensions(&bytes)?;
        let (bw, bh) = (even(w), even(h));
        if bw == old_bw && bh == old_bh {
            return Some((bw, bh));
        }
        let out_h = if out_source {
            bh
        } else {
            even(old_out_h.min(bh))
        };
        let out_w = scaled_width(bw, bh, out_h);
        let res = self
            .obsws
            .as_ref()?
            .set_video_settings(bw, bh, out_w, out_h)
            .await;
        match res {
            Ok(()) => {
                if let Some(p) = self.profile.as_mut() {
                    p.base_width = bw;
                    p.base_height = bh;
                    p.out_width = out_w;
                    p.out_height = out_h;
                }
                eprintln!("[moonclip] obs canvas learned: {bw}x{bh} (output {out_w}x{out_h})");
                Some((bw, bh))
            }
            Err(e) => {
                eprintln!("[moonclip] obs canvas resize failed: {e}");
                None
            }
        }
    }

}

impl CaptureEngine for ObsEngine {
    async fn start_buffer(&mut self, config: CaptureConfig) -> Result<(), String> {
        self.stop_buffer().await.ok();
        self.error = None;

        // The caller resolves bundled binaries (it owns the AppHandle).
        let obs_bin = config
            .obs_bin
            .clone()
            .ok_or_else(|| "embedded OBS binary not resolved".to_string())?;
        let profile = ObsProfile::from_config(&config, self.platform.as_ref())?;

        // 1. Prepare the isolated runtime. Windows stages a WRITABLE PORTABLE
        //    copy (OBS writes its config inside it); Linux launches the
        //    relocatable build with XDG_CONFIG_HOME redirected. Either way the
        //    user's %APPDATA%/obs-studio (or ~/.config/obs-studio) is
        //    untouchable.
        let rt = self.platform.prepare_runtime(&obs_bin)?;
        self.config_root = rt.config_root.clone();
        eprintln!(
            "[moonclip] obs runtime: {} (portable={}, config={})",
            rt.bin.display(),
            rt.portable,
            rt.config_root.display()
        );

        // 2. Our own profile/scene/websocket config, before OBS starts.
        // The tray flag comes from the platform (Windows runs traceless).
        write_obs_config(
            &self.config_root,
            &profile,
            self.platform.sys_tray_enabled(),
        )?;

        // 3. Sweep leftovers of OUR runtime binary from a force-killed session.
        self.platform.kill_orphans(&rt.bin);

        // 4. Launch the embedded OBS, isolated and out of the way.
        let mut args =
            self.platform
                .obs_launch_args(&self.config_root, OBS_PROFILE, OBS_COLLECTION);
        args.extend(rt.extra_args.iter().cloned());
        let mut cmd = Command::new(&rt.bin);
        cmd.args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        if let Some(dir) = self.platform.working_dir(&rt.bin) {
            cmd.current_dir(dir);
        }
        for (k, v) in self.platform.obs_launch_env(&self.config_root) {
            cmd.env(k, v);
        }
        self.platform.configure(&mut cmd);
        let child = cmd
            .spawn()
            .map_err(|e| format!("cannot launch embedded OBS ({}): {e}", rt.bin.display()))?;
        let pid = child.id();
        self.child = Some(child);
        if let Some(pid) = pid {
            // Windows job object: OBS dies when MoonClip does, even on a
            // force-kill (no orphan holding capture/encode sessions).
            self.platform.adopt_child(pid);
            // Hide the embedded window right away so the user never sees
            // it (no-op on platforms without concealment).
            self.platform.conceal_window(pid, &rt.bin);
        }

        // 5. Safety guard: if OBS writes outside our config, abort immediately.
        if let Err(e) = self.wait_for_config_guard().await {
            self.stop_buffer().await.ok();
            let tail = read_obs_log_tail(&self.config_root);
            return Err(match tail.last() {
                Some(l) => format!("{e} | last log: {l}"),
                None => e,
            });
        }

        // 6. Wait for the private obs-websocket, then start the replay buffer.
        let deadline = std::time::Instant::now() + OBS_BOOT_TIMEOUT;
        let mut ws: Option<Obsws> = None;
        while std::time::Instant::now() < deadline {
            if let Some(child) = self.child.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    self.child = None;
                    let tail = read_obs_log_tail(&self.config_root);
                    return Err(format!(
                        "embedded OBS exited during startup ({status}){}",
                        tail.last().map(|l| format!(": {l}")).unwrap_or_default()
                    ));
                }
            }
            if let Ok(client) =
                Obsws::connect(profile.websocket_port, &profile.websocket_password).await
            {
                ws = Some(client);
                break;
            }
            tokio::time::sleep(OBS_BOOT_POLL).await;
        }
        let ws = match ws {
            Some(ws) => ws,
            None => {
                self.stop_buffer().await.ok();
                return Err("embedded OBS websocket did not come up within 45 s".into());
            }
        };

        // Start the replay buffer. One retry absorbs a transient state while
        // OBS finishes initializing its outputs.
        let mut start_res = ws.replay_start().await;
        if start_res.is_err() {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            start_res = ws.replay_start().await;
        }
        if let Err(e) = start_res {
            return Err(self.fail_start(e).await);
        }
        // OBS starts the output asynchronously (and a first-run wizard can
        // interfere): poll for a few seconds before declaring failure.
        let mut active = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(6);
        while std::time::Instant::now() < deadline {
            if let Ok(true) = ws.replay_status().await {
                active = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        if !active {
            return Err(self
                .fail_start(
                    "OBS accepted replay start but the output is not running \
                     (a first-run/auto-config dialog or an encoder error blocks it)"
                        .to_string(),
                )
                .await);
        }
        // Verify OBS is really rendering OUR scene: a scene/program mismatch
        // records all-black with no other error (name collisions, canvas
        // assignment). Refuse to run rather than save black clips.
        match ws.current_scene_name().await {
            Ok(scene) if scene == SCENE_NAME => {}
            Ok(other) => {
                return Err(self
                    .fail_start(format!(
                    "OBS program scene is '{other}', expected '{SCENE_NAME}' (would record black)"
                ))
                    .await)
            }
            Err(e) => eprintln!("[moonclip] warning: cannot verify OBS program scene: {e}"),
        }

        eprintln!(
            "[moonclip] obs: buffer started ({}x{}@{} {} {}kbps, replay {}s/{}MB, tracks {}, audio {})",
            profile.out_width,
            profile.out_height,
            profile.fps,
            profile.encoder_id,
            profile.bitrate_kbps,
            profile.replay_seconds,
            profile.replay_mb,
            profile.tracks_linked(),
            if profile.single_track { "Mix" } else { "Mix+Game+Mic" }
        );
        self.profile = Some(profile);
        self.obsws = Some(ws);
        Ok(())
    }

    async fn save_clip(&mut self) -> Result<PathBuf, String> {
        if self.child.is_none() {
            return Err("recorder not running".into());
        }
        let obsws = self.obsws()?;
        let path = obsws.replay_save().await?;
        if !path.exists() {
            return Err(format!(
                "OBS reported a saved replay that is not on disk: {}",
                path.display()
            ));
        }
        Ok(path)
    }

    async fn stop_buffer(&mut self) -> Result<(), String> {
        if let Some(obsws) = self.obsws.take() {
            // Best effort: stop the buffer before killing OBS.
            let _ = obsws.replay_stop().await;
        }
        if let Some(mut child) = self.child.take() {
            if let Some(pid) = child.id() {
                // Drop any window-management hooks we installed for it.
                self.platform.unconceal_window(pid);
            }
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        self.profile = None;
        Ok(())
    }

    fn tracks_linked(&self) -> usize {
        self.profile
            .as_ref()
            .map(|p| p.tracks_linked())
            .unwrap_or(0)
    }

    fn check_alive(&mut self) -> bool {
        match self.child.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    }

    fn log_tail(&self) -> Vec<String> {
        let mut tail = read_obs_log_tail(&self.config_root);
        if let Some(err) = &self.error {
            tail.push(format!("moonclip: {err}"));
        }
        tail
    }

    async fn set_mute(&mut self, track: &str, muted: bool) -> Result<(), String> {
        let source = match track {
            "game" => GAME_SOURCE_NAME,
            "mic" => MIC_SOURCE_NAME,
            other => return Err(format!("unknown track '{other}'")),
        };
        let obsws = self.obsws()?;
        obsws.set_input_muted(source, muted).await
    }

    async fn set_volume(&mut self, track: &str, percent: u32) -> Result<(), String> {
        let source = volume_source(track).ok_or_else(|| format!("unknown track '{track}'"))?;
        let obsws = self.obsws()?;
        obsws.set_input_volume(source, percent).await
    }
}

// ---------------------------------------------------------------------------
// Tests (pure logic only — no OBS required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct FakePlatform;

    impl ObsPlatform for FakePlatform {
        fn prepare_runtime(&self, bundled_bin: &Path) -> Result<ObsRuntime, String> {
            Ok(ObsRuntime {
                bin: bundled_bin.to_path_buf(),
                config_root: PathBuf::from("C:/MoonClip/obs/config"),
                extra_args: vec![],
                portable: true,
            })
        }
        fn obs_launch_args(&self, _r: &Path, _p: &str, _c: &str) -> Vec<String> {
            vec![]
        }
        fn working_dir(&self, _bin: &Path) -> Option<PathBuf> {
            None
        }
        fn kill_orphans(&self, _bin: &Path) {}
        fn video_source(&self, monitor: &str, _window: &str) -> (&'static str, serde_json::Value) {
            (
                "monitor_capture",
                json!({"monitor": monitor, "method": 1, "capture_cursor": true}),
            )
        }
        fn game_audio_source_id(&self) -> &'static str {
            "wasapi_output_capture"
        }
        fn mic_audio_source_id(&self) -> &'static str {
            "wasapi_input_capture"
        }
        fn encoder_id(&self, vendor: &str, codec: &str, encoder: &str) -> Option<&'static str> {
            if encoder == "cpu" {
                return Some("obs_x264");
            }
            match (vendor, codec) {
                ("nvidia", "h264") => Some("obs_nvenc_h264_tex"),
                ("nvidia", "hevc") => Some("obs_nvenc_hevc_tex"),
                ("nvidia", "av1") => Some("obs_nvenc_av1_tex"),
                ("amd", "h264") => Some("h264_texture_amf"),
                ("intel", "h264") => Some("obs_qsv11_v2"),
                _ => None,
            }
        }
        fn encoder_catalog(&self) -> &'static [super::super::encoder_options::EncoderEntry] {
            super::super::encoder_options::catalog_windows()
        }
    }

    fn config() -> CaptureConfig {
        CaptureConfig {
            duration_seconds: 30,
            fps: 60,
            output_dir: PathBuf::from("C:/clips"),
            codec: "h264".into(),
            encoder: "gpu".into(),
            gpu_index: 0,
            bitrate_kbps: 20_000,
            out_height: 1080,
            monitor: "0".into(),
            window: String::new(),
            desktop_device: "default_output".into(),
            mic_device: "default_input".into(),
            gain_game: 100,
            gain_mic: 100,
            mute_game: false,
            mute_mic: false,
            audio_single_track: false,
            container: "mp4".into(),
            vendor: "nvidia".into(),
            base_width: 2560,
            base_height: 1440,
            portal_restore_token: String::new(),
            custom_encoder: None,
            custom_video: None,
            obs_bin: Some(PathBuf::from("obs64.exe")),
            websocket_port: 4456,
            websocket_password: "abc123".into(),
        }
    }

    fn profile() -> ObsProfile {
        ObsProfile::from_config(&config(), &FakePlatform).unwrap()
    }

    #[test]
    fn profile_scales_output_and_keeps_base() {
        let p = profile();
        assert_eq!((p.base_width, p.base_height), (2560, 1440));
        assert_eq!((p.out_width, p.out_height), (1920, 1080));
        assert_eq!(p.encoder_id, "obs_nvenc_h264_tex");
        assert_eq!(p.rec_tracks(), 7);
        assert_eq!(p.tracks_linked(), 3);
    }

    #[test]
    fn source_resolution_keeps_base_as_output() {
        let mut c = config();
        c.out_height = 0;
        let p = ObsProfile::from_config(&c, &FakePlatform).unwrap();
        assert_eq!((p.out_width, p.out_height), (2560, 1440));
    }

    #[test]
    fn single_track_uses_one_mixer() {
        let mut c = config();
        c.audio_single_track = true;
        let p = ObsProfile::from_config(&c, &FakePlatform).unwrap();
        assert_eq!(p.rec_tracks(), 1);
        assert_eq!(p.tracks_linked(), 1);
    }

    #[test]
    fn cpu_encoder_maps_to_x264() {
        let mut c = config();
        c.encoder = "cpu".into();
        let p = ObsProfile::from_config(&c, &FakePlatform).unwrap();
        assert_eq!(p.encoder_id, "obs_x264");
    }

    #[test]
    fn unsupported_codec_vendor_fails_loudly() {
        let mut c = config();
        c.vendor = "unknown".into();
        let err = ObsProfile::from_config(&c, &FakePlatform).unwrap_err();
        assert!(err.contains("no OBS encoder"), "{err}");
    }

    #[test]
    fn basic_ini_has_advanced_replay_and_tracks() {
        let p = profile();
        let ini = render_basic_ini(&p);
        assert!(ini.contains("Mode=Advanced"), "{ini}");
        assert!(ini.contains("RecRB=true"));
        assert!(ini.contains("RecRBTime=30"));
        assert!(ini.contains("RecTracks=7"));
        assert!(ini.contains("RecEncoder=obs_nvenc_h264_tex"));
        assert!(ini.contains("Track1Bitrate=320"));
        assert!(ini.contains("Track3Bitrate=192"));
        assert!(ini.contains("Track1Name=Master Mix [Game+Voice]"));
        assert!(ini.contains("OutputCX=1920"));
        assert!(ini.contains("FPSCommon=60"));
        assert!(ini.contains("RecFormat2=mp4"));
    }

    #[test]
    fn basic_ini_single_track_writes_one_track() {
        let mut c = config();
        c.audio_single_track = true;
        let p = ObsProfile::from_config(&c, &FakePlatform).unwrap();
        let ini = render_basic_ini(&p);
        assert!(ini.contains("RecTracks=1"), "{ini}");
    }

    #[test]
    fn record_encoder_json_uses_registry_recipes() {
        // Ladder NVENC: measured recipe with OBS 32 keys (preset, NOT preset2).
        let p = profile();
        assert_eq!(p.encoder_id, "obs_nvenc_h264_tex");
        let v: serde_json::Value = serde_json::from_str(&render_record_encoder_json(&p)).unwrap();
        assert_eq!(v["rate_control"], "CBR");
        assert_eq!(v["bitrate"], 20_000);
        assert_eq!(v["preset"], "p5");
        assert_eq!(v["tune"], "hq");
        assert_eq!(v["multipass"], "disabled");
        assert_eq!(v["adaptive_quantization"], true);
        assert_eq!(v["device"], -1);
        assert_eq!(v["profile"], "high");
        assert!(v.get("preset2").is_none(), "{v}");
        assert!(v.get("psycho_aq").is_none(), "{v}");
        assert!(v.get("gpu").is_none(), "{v}");

        // CPU ladder: measured x264 recipe.
        let mut c = config();
        c.encoder = "cpu".into();
        let p = ObsProfile::from_config(&c, &FakePlatform).unwrap();
        let v: serde_json::Value = serde_json::from_str(&render_record_encoder_json(&p)).unwrap();
        assert_eq!(v["rate_control"], "CBR");
        assert_eq!(v["preset"], "veryfast");

        // Unvalidated ladder family (AMD): Auto recipe only.
        let mut c = config();
        c.vendor = "amd".into();
        let p = ObsProfile::from_config(&c, &FakePlatform).unwrap();
        assert_eq!(p.encoder_id, "h264_texture_amf");
        let v: serde_json::Value = serde_json::from_str(&render_record_encoder_json(&p)).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"bitrate": 20_000, "rate_control": "CBR"})
        );
    }

    #[test]
    fn custom_encoder_and_video_render_verbatim() {
        use super::super::encoder_options as enc;
        let mut c = config();
        let raw_enc = r#"{"encoder":"obs_nvenc_h264_tex","settings":{"rate_control":"CQP","cqp":20,"preset":"p7"}}"#;
        let custom = enc::parse_custom_encoder(raw_enc).unwrap();
        c.custom_encoder = Some(crate::os::api::CustomEncoder {
            encoder: custom.encoder,
            settings: custom.settings,
        });
        let raw_vid = r#"{"out_width":2560,"out_height":1440,"scale_type":"lanczos","fps_type":"fractional","fps_common":60,"fps_int":60,"fps_num":60000,"fps_den":1001,"color_format":"NV12","color_space":"709","color_range":"Partial"}"#;
        let vv = enc::parse_custom_video(enc::EncoderFamily::Nvenc, "h264", raw_vid).unwrap();
        c.custom_video = Some(crate::os::api::CustomVideo {
            out_width: vv.out_width,
            out_height: vv.out_height,
            scale_type: vv.scale_type,
            fps_type: vv.fps_type,
            fps_common: vv.fps_common,
            fps_int: vv.fps_int,
            fps_num: vv.fps_num,
            fps_den: vv.fps_den,
            color_format: vv.color_format,
            color_space: vv.color_space,
            color_range: vv.color_range,
        });
        let p = ObsProfile::from_config(&c, &FakePlatform).unwrap();
        assert_eq!(p.codec, "h264");
        let v: serde_json::Value = serde_json::from_str(&render_record_encoder_json(&p)).unwrap();
        assert_eq!(v["rate_control"], "CQP");
        assert_eq!(v["cqp"], 20);
        // Custom bitrate absent -> ladder bitrate drives the ring math.
        assert_eq!(p.bitrate_kbps, 20_000);
        let ini = render_basic_ini(&p);
        assert!(ini.contains("OutputCX=2560"), "{ini}");
        assert!(ini.contains("OutputCY=1440"), "{ini}");
        assert!(ini.contains("ScaleType=lanczos"), "{ini}");
        assert!(ini.contains("FPSType=2"), "{ini}");
        assert!(ini.contains("FPSNum=60000"), "{ini}");
        assert!(ini.contains("FPSDen=1001"), "{ini}");

        // Unknown encoder id on this platform fails loudly.
        let mut c = config();
        c.custom_encoder = Some(crate::os::api::CustomEncoder {
            encoder: "wat".into(),
            settings: serde_json::Map::new(),
        });
        let err = ObsProfile::from_config(&c, &FakePlatform).unwrap_err();
        assert!(err.contains("not in this OBS build"), "{err}");
    }

    #[test]
    fn volume_helpers_map_tracks_and_multipliers() {
        assert_eq!(volume_source("game"), Some(GAME_SOURCE_NAME));
        assert_eq!(volume_source("mic"), Some(MIC_SOURCE_NAME));
        assert_eq!(volume_source("mix"), None);
    }

    #[test]
    fn device_ids_normalize_magic_values() {
        assert_eq!(normalize_device_id("", true), "default");
        assert_eq!(normalize_device_id("default_output", true), "default");
        assert_eq!(normalize_device_id("default", true), "default");
        assert_eq!(normalize_device_id("default_input", false), "default");
        assert_eq!(
            normalize_device_id("{0.0.1.00000000}.{guid}", true),
            "{0.0.1.00000000}.{guid}"
        );
        // A desktop id is not valid for the mic and vice versa.
        assert_eq!(normalize_device_id("default_input", true), "default_input");
    }

    #[test]
    fn collection_never_uses_game_capture() {
        let p = profile();
        let uuids = SourceUuids::generate();
        let text = render_collection(&p, &uuids);
        assert!(!text.contains(FORBIDDEN_SOURCE_ID), "{text}");
        assert!(text.contains("monitor_capture"));
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["AuxAudioDevice1"]["mixers"], 3);
        assert_eq!(v["AuxAudioDevice2"]["mixers"], 5);
        assert_eq!(v["AuxAudioDevice1"]["volume"], 1.0);
        assert_eq!(v["sources"][0]["name"], SCENE_NAME);
        assert_eq!(v["sources"][1]["id"], "monitor_capture");
        // Scene and capture source MUST have distinct names: OBS resolves the
        // program scene by name and a collision produced all-black clips.
        assert_eq!(v["sources"][1]["name"], VIDEO_SOURCE_NAME);
        assert_ne!(v["sources"][0]["name"], v["sources"][1]["name"]);
        // Scene explicitly bound to libobs' main canvas.
        assert_eq!(v["sources"][0]["canvas_uuid"], MAIN_CANVAS_UUID);
        // The scene item references the capture source by uuid (OBS 30+).
        assert_eq!(
            v["sources"][0]["settings"]["items"][0]["source_uuid"],
            uuids.video
        );
    }

    #[test]
    fn collection_applies_gains_and_mutes() {
        let mut c = config();
        c.gain_game = 150;
        c.gain_mic = 50;
        c.mute_mic = true;
        let p = ObsProfile::from_config(&c, &FakePlatform).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&render_collection(&p, &SourceUuids::generate())).unwrap();
        assert_eq!(v["AuxAudioDevice1"]["volume"], 1.5);
        assert_eq!(v["AuxAudioDevice2"]["volume"], 0.5);
        assert_eq!(v["AuxAudioDevice2"]["muted"], true);
        assert_eq!(v["AuxAudioDevice1"]["muted"], false);
    }

    #[test]
    fn collection_single_track_mixes_only_track_one() {
        let mut c = config();
        c.audio_single_track = true;
        let p = ObsProfile::from_config(&c, &FakePlatform).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&render_collection(&p, &SourceUuids::generate())).unwrap();
        assert_eq!(v["AuxAudioDevice1"]["mixers"], 1);
        assert_eq!(v["AuxAudioDevice2"]["mixers"], 1);
    }

    #[test]
    fn websocket_config_is_private() {
        let text = render_websocket_config(4456, "deck");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["server_enabled"], true);
        assert_eq!(v["server_port"], 4456);
        assert_eq!(v["auth_required"], true);
        assert_eq!(v["server_password"], "deck");
    }

    #[test]
    fn existing_restore_token_reads_collection() {
        let dir = std::env::temp_dir().join(format!("moonclip-token-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("MoonClip.json");
        std::fs::write(
            &file,
            r#"{"sources":[{"id":"scene","settings":{}},{"id":"pipewire-desktop-capture-source","settings":{"RestoreToken":"tok123"}}]}"#,
        )
        .unwrap();
        assert_eq!(existing_restore_token(&file), Some("tok123".to_string()));
        std::fs::write(
            &file,
            r#"{"sources":[{"id":"pipewire-desktop-capture-source","settings":{}}]}"#,
        )
        .unwrap();
        assert_eq!(existing_restore_token(&file), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn replay_mb_has_margin_and_floor() {
        assert_eq!(replay_mb(20_000, 30), 173);
        assert_eq!(replay_mb(60_000, 60), 722);
        assert!(replay_mb(100_000, 600) > 100_000 * 600 / 8 / 1024);
    }

    #[test]
    fn gain_volume_conversion() {
        assert_eq!(gain_to_volume(0), 0.0);
        assert_eq!(gain_to_volume(100), 1.0);
        assert_eq!(gain_to_volume(150), 1.5);
        assert_eq!(gain_to_volume(500), 2.0);
    }

    #[test]
    fn global_ini_selects_generated_profile() {
        let ini = render_global_ini();
        assert!(ini.contains("Profile=MoonClip"));
        assert!(ini.contains("SceneCollection=MoonClip"));
    }

    #[test]
    fn user_ini_suppresses_first_run_wizard() {
        for tray in [true, false] {
            let ini = render_user_ini(tray);
            assert!(ini.contains("[General]"), "{ini}");
            assert!(ini.contains("FirstRun=true"), "{ini}");
        }
    }

    #[test]
    fn user_ini_disables_tray_when_asked() {
        let on = render_user_ini(true);
        assert!(!on.contains("SysTrayEnabled"), "{on}");
        let off = render_user_ini(false);
        assert!(off.contains("SysTrayEnabled=false"), "{off}");
        assert!(off.contains("SysTrayWhenStarted=false"), "{off}");
        assert!(off.contains("SysTrayMinimizeToTray=false"), "{off}");
    }

    #[test]
    fn portable_copy_and_marker_roundtrip() {
        let base = std::env::temp_dir().join(format!("moonclip-runtime-{}", std::process::id()));
        let src = base.join("src");
        let dst = base.join("dst");
        std::fs::create_dir_all(src.join("bin/64bit")).unwrap();
        std::fs::create_dir_all(src.join("data/lib")).unwrap();
        std::fs::write(src.join("bin/64bit/obs64.exe"), b"obs").unwrap();
        std::fs::write(src.join("data/lib/plugin.dll"), b"plugin").unwrap();
        copy_dir_recursive(&src, &dst).unwrap();
        assert_eq!(
            std::fs::read(dst.join("bin/64bit/obs64.exe")).unwrap(),
            b"obs"
        );
        assert_eq!(
            std::fs::read(dst.join("data/lib/plugin.dll")).unwrap(),
            b"plugin"
        );
        let fp = obs_build_fingerprint(&src.join("bin/64bit/obs64.exe"));
        assert!(fp.contains("obs64.exe"), "{fp}");
        let marker = dst.join(".moonclip-source");
        assert!(!marker_matches(&marker, &fp));
        write_marker(&marker, &fp).unwrap();
        assert!(marker_matches(&marker, &fp));
        assert!(!marker_matches(&marker, "something else"));
        std::fs::remove_dir_all(&base).ok();
    }
}
