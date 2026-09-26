//! Tauri IPC handlers (Phase 2: persistence; Phase 3: capture).
//! V3 capture engine: embedded, isolated OBS Studio driven over obs-websocket.

use std::collections::HashMap;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::os::shared::encoder_options as enc;
use crate::os::{
    self, backend_name, devices, new_engine, resolve_obs, shared::engine as obs, video, AudioDevice,
    CaptureConfig, CaptureEngine, CustomEncoder, CustomVideo,
};
use crate::state::AppState;
use crate::storage::models::{ClipRecord, CustomApp, RegisterAppInput};
use crate::storage::{secrets, DbState};

#[tauri::command]
pub fn list_clips(db: State<'_, DbState>) -> Result<Vec<ClipRecord>, String> {
    db.list_clips()
}

#[tauri::command]
pub fn toggle_favorite(db: State<'_, DbState>, id: String) -> Result<bool, String> {
    db.toggle_favorite(&id)
}

#[tauri::command]
pub fn delete_clip(db: State<'_, DbState>, id: String) -> Result<(), String> {
    db.delete_clip(&id)
}

/// Drop DB rows whose files are gone from disk. Returns rows removed.
#[tauri::command]
pub fn purge_missing_clips(db: State<'_, DbState>) -> Result<u32, String> {
    let base = db.clips_dir()?;
    let missing: Vec<String> = db
        .list_clips()?
        .into_iter()
        .filter(|c| !crate::storage::paths::resolve_clip_path(&base, &c.file_name).exists())
        .map(|c| c.id)
        .collect();
    let n = missing.len() as u32;
    for id in &missing {
        db.delete_row(id)?;
    }
    Ok(n)
}

/// Absolute filesystem path for a clip file name (for <video> / convertFileSrc).
#[tauri::command]
pub fn resolve_clip_src(db: State<'_, DbState>, file_name: String) -> Result<String, String> {
    if file_name.contains("..") || file_name.starts_with('/') || file_name.starts_with('\\') {
        return Err("invalid file name".into());
    }
    let base = db.clips_dir()?;
    Ok(base.join(&file_name).to_string_lossy().to_string())
}

#[tauri::command]
pub fn get_settings(db: State<'_, DbState>) -> Result<HashMap<String, String>, String> {
    db.get_settings()
}

// ---------------------------------------------------------------------------
// Capture configuration
// ---------------------------------------------------------------------------

fn setting_str(db: &DbState, key: &str, default: &str) -> String {
    db.get_settings()
        .ok()
        .and_then(|s| s.get(key).cloned())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn buffer_seconds(db: &DbState) -> i64 {
    db.get_settings()
        .ok()
        .and_then(|s| s.get("buffer_seconds").and_then(|v| v.parse().ok()))
        .unwrap_or(30)
        .max(5)
}

/// Custom mode capture parameters. `ladder` keeps the Medal row; in `custom`
/// the FPS is one of the Medal values (24/30/60/120/144) and the bitrate sits
/// in the slider range (3–100 Mbps); invalid values fall back to the ladder.
/// Returns `(fps, bitrate_kbps, was_clamped)`.
pub(crate) fn custom_capture_params(
    mode: &str,
    custom_fps: &str,
    custom_kbps: &str,
    ladder_fps: u32,
    ladder_kbps: u32,
) -> (u32, u32, bool) {
    if mode.trim() != "custom" {
        return (ladder_fps, ladder_kbps, false);
    }
    let wanted_fps = custom_fps.trim().parse::<u32>().ok();
    let (fps, clamped) = match wanted_fps {
        Some(v) if [24, 30, 60, 120, 144].contains(&v) => (v, false),
        Some(_) => (ladder_fps, true),
        None => (ladder_fps, false),
    };
    let bitrate = custom_kbps
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|v| (3_000..=100_000).contains(v))
        .unwrap_or(ladder_kbps);
    (fps, bitrate, clamped)
}

/// Overrides for the hardware test (never persisted by the test itself).
#[derive(Debug, Clone, Default)]
pub(crate) struct StartOverrides {
    pub height: Option<u32>,
    pub fps: Option<u32>,
    pub codec: Option<String>,
    pub encoder: Option<String>,
    pub bitrate_kbps: Option<u32>,
    pub duration_seconds: Option<u32>,
    /// Full Custom payloads for the "Probar" button (validated, not persisted).
    pub custom_encoder: Option<CustomEncoder>,
    pub custom_video: Option<CustomVideo>,
    /// Detected per-game context (window capture + duration), when known.
    pub game: Option<GameContext>,
}

/// Detection result for the game being captured: drives window targeting,
/// the per-game portal token and the user-chosen clip duration.
#[derive(Debug, Clone, Default)]
pub(crate) struct GameContext {
    pub key: String,
    pub title: String,
    /// 'x11' | 'portal'
    pub source_kind: Option<String>,
    pub window_match: Option<String>,
    /// Wayland-native restore token already stored for this game.
    pub token: Option<String>,
    /// User-chosen clip duration for this game.
    pub duration_seconds: Option<u32>,
}

/// Validate Custom JSON payloads before persisting: unknown encoder ids,
/// unknown options and out-of-range values are rejected with a clear error
/// and nothing is written. `proposed` carries the values about to be written
/// so the encoder+video pair validates together. Empty strings mean "unused".
async fn validate_custom_pair(
    app: &AppHandle,
    proposed: &HashMap<String, String>,
) -> Result<(), String> {
    let db = app.state::<DbState>();
    let stored = db.get_settings().unwrap_or_default();
    let ce_raw = proposed
        .get("custom_encoder_json")
        .or_else(|| stored.get("custom_encoder_json"))
        .map(|s| s.as_str())
        .unwrap_or("");
    let cv_raw = proposed
        .get("custom_video_json")
        .or_else(|| stored.get("custom_video_json"))
        .map(|s| s.as_str())
        .unwrap_or("");
    let custom_encoder = if ce_raw.trim().is_empty() {
        None
    } else {
        let parsed = enc::parse_custom_encoder(ce_raw)?;
        let entry = enc::lookup_encoder(&parsed.encoder)
            .ok_or_else(|| format!("unknown encoder '{}'", parsed.encoder))?;
        Some((entry, parsed))
    };
    if !cv_raw.trim().is_empty() {
        let (entry, parsed) = match &custom_encoder {
            Some((e, p)) => (*e, p),
            // Video overrides only make sense with an explicit Custom
            // encoder (color formats are family-specific).
            None => return Err("custom video needs a custom encoder first".into()),
        };
        let video = enc::parse_custom_video(entry.family, entry.codec, cv_raw)?;
        // Cross-check encoder settings against the video color (10-bit
        // profiles need P010): the pair validates together or not at all.
        enc::validate_pair(
            entry.family,
            entry.codec,
            &parsed.settings,
            Some(&video.color_format),
        )?;
    }
    Ok(())
}

/// Build one validated capture config from persisted settings (+ overrides).
pub(crate) async fn build_capture_config(
    app: &AppHandle,
    overrides: &StartOverrides,
) -> Result<CaptureConfig, String> {
    let db = app.state::<DbState>();
    let output_dir = db.clips_dir()?;
    let duration_seconds = overrides
        .duration_seconds
        .or_else(|| overrides.game.as_ref().and_then(|g| g.duration_seconds))
        .unwrap_or_else(|| buffer_seconds(&db) as u32);

    // Legacy `video_codec=x264` (old CPU option) maps to h264 + cpu encoder.
    let stored_codec = setting_str(&db, "video_codec", "h264");
    let (mut codec, mut encoder) = match stored_codec.as_str() {
        "x264" => ("h264".to_string(), "cpu".to_string()),
        other => (other.to_string(), setting_str(&db, "video_encoder", "gpu")),
    };
    if !["h264", "hevc", "av1"].contains(&codec.as_str()) {
        codec = "h264".to_string();
    }
    if !["gpu", "cpu"].contains(&encoder.as_str()) {
        encoder = "gpu".to_string();
    }
    if let Some(c) = &overrides.codec {
        if ["h264", "hevc", "av1"].contains(&c.as_str()) {
            codec = c.clone();
        }
    }
    if let Some(e) = &overrides.encoder {
        if ["gpu", "cpu"].contains(&e.as_str()) {
            encoder = e.clone();
        }
    }

    // Custom mode (video_mode=custom): validated encoder + video payloads.
    // Codec and fps come from the payloads; unknown ids/values fail loudly.
    let mut custom_encoder: Option<CustomEncoder> = None;
    let mut custom_video: Option<CustomVideo> = None;
    if setting_str(&db, "video_mode", "ladder") == "custom" {
        let ce_raw = setting_str(&db, "custom_encoder_json", "");
        if !ce_raw.trim().is_empty() {
            custom_encoder = Some(enc::parse_custom_encoder(&ce_raw)?);
        }
        if let Some(ce) = &custom_encoder {
            let entry = enc::lookup_encoder(&ce.encoder)
                .ok_or_else(|| format!("unknown encoder '{}'", ce.encoder))?;
            codec = entry.codec.to_string();
            let cv_raw = setting_str(&db, "custom_video_json", "");
            if !cv_raw.trim().is_empty() {
                custom_video = Some(enc::parse_custom_video(entry.family, entry.codec, &cv_raw)?);
            }
        }
    }
    // Test/"Probar" overrides replace persisted payloads (validated here,
    // never persisted).
    if let Some(ce) = &overrides.custom_encoder {
        let entry = enc::lookup_encoder(&ce.encoder)
            .ok_or_else(|| format!("unknown encoder '{}'", ce.encoder))?;
        enc::validate(entry.family, entry.codec, &ce.settings)?;
        codec = entry.codec.to_string();
        custom_encoder = Some(ce.clone());
    }
    if let Some(cv) = &overrides.custom_video {
        let (family, fam_codec) = custom_encoder
            .as_ref()
            .and_then(|ce| enc::lookup_encoder(&ce.encoder))
            .map(|e| (e.family, e.codec.to_string()))
            .unwrap_or((enc::EncoderFamily::Nvenc, codec.clone()));
        let raw = serde_json::to_string(cv).map_err(|e| e.to_string())?;
        custom_video = Some(enc::parse_custom_video(family, &fam_codec, &raw)?);
    }
    // Pair gate (same as the pre-write check): 10-bit profiles need P010.
    if let Some(ce) = &custom_encoder {
        if let Some(entry) = enc::lookup_encoder(&ce.encoder) {
            let color = custom_video.as_ref().map(|v| v.color_format.as_str());
            enc::validate_pair(entry.family, entry.codec, &ce.settings, color)?;
        }
    }

    // Monitors: the OBS `monitor_id` device id is the stored value (stable
    // across index changes); legacy indices/alt names still resolve.
    let monitors = video::list_monitors().await;
    let monitor_setting = setting_str(&db, "monitor", "");
    let selected = video::resolve_monitor(&monitors, &monitor_setting);
    let (base_width, base_height) = selected
        .map(|m| (m.width, m.height))
        .unwrap_or((1920, 1080));

    let out_height = overrides
        .height
        .unwrap_or_else(|| setting_str(&db, "out_height", "0").parse().unwrap_or(0));
    let out_height = if out_height == 0 || out_height >= base_height {
        0
    } else {
        out_height
    };

    let ladder_fps: u32 = match setting_str(&db, "fps", "60").parse().unwrap_or(60) {
        30 => 30,
        120 => 120,
        144 => 144,
        24 => 24,
        _ => 60,
    };
    let ladder_bitrate = crate::video_quality::bitrate_kbps(
        if out_height == 0 {
            custom_video
                .as_ref()
                .map(|v| v.out_height)
                .unwrap_or(base_height)
        } else {
            out_height
        },
        &codec,
    );
    let (mut fps, mut bitrate, fps_clamped) = custom_capture_params(
        &setting_str(&db, "video_mode", "ladder"),
        &setting_str(&db, "custom_fps", ""),
        &setting_str(&db, "custom_bitrate_kbps", ""),
        ladder_fps,
        ladder_bitrate,
    );
    if fps_clamped {
        eprintln!("[moonclip] custom fps not in Medal values, using {fps}");
    }
    // Full Custom payloads override the legacy custom bitrate/fps.
    if let Some(cv) = &custom_video {
        fps = enc::custom_effective_fps(cv);
    }
    if let Some(f) = overrides.fps {
        fps = f;
    }
    if let Some(b) = overrides.bitrate_kbps {
        bitrate = b;
    }

    // Private obs-websocket: dedicated port + generated password (persisted).
    let port: u16 = setting_str(&db, "engine_ws_port", "4456")
        .parse()
        .unwrap_or(4456)
        .clamp(1024, 65535);
    let mut password = setting_str(&db, "engine_ws_password", "");
    if password.len() < 16 {
        password =
            uuid::Uuid::new_v4().simple().to_string() + &uuid::Uuid::new_v4().simple().to_string();
        let _ = db.set_setting("engine_ws_password", &password);
    }

    let (obs_bin, _) = resolve_obs(app)?;

    Ok(CaptureConfig {
        duration_seconds,
        fps,
        output_dir,
        codec,
        encoder,
        gpu_index: setting_str(&db, "gpu_index", "0").parse().unwrap_or(0),
        bitrate_kbps: bitrate,
        out_height,
        monitor: selected.map(|m| m.name.clone()).unwrap_or_default(),
        window: if overrides
            .game
            .as_ref()
            .map(|g| g.window_match.is_none() && g.source_kind.as_deref() == Some("portal"))
            .unwrap_or(false)
        {
            // Portal window capture: any non-empty marker selects the window
            // source; the session itself comes from the per-game token.
            overrides
                .game
                .as_ref()
                .map(|g| g.key.clone())
                .unwrap_or_default()
        } else {
            setting_str(&db, "capture_window", "")
        },
        window_match: overrides.game.as_ref().and_then(|g| g.window_match.clone()),
        desktop_device: devices::resolve_obs_device_id(
            &setting_str(&db, "desktop_device", "default_output"),
            true,
        )
        .await,
        mic_device: devices::resolve_obs_device_id(
            &setting_str(&db, "mic_device", "default_input"),
            false,
        )
        .await,
        gain_game: setting_str(&db, "gain_game", "100")
            .parse()
            .unwrap_or(100)
            .clamp(0, 200),
        gain_mic: setting_str(&db, "gain_mic", "100")
            .parse()
            .unwrap_or(100)
            .clamp(0, 200),
        mute_game: matches!(setting_str(&db, "mute_game", "0").as_str(), "1" | "true"),
        mute_mic: matches!(setting_str(&db, "mute_mic", "0").as_str(), "1" | "true"),
        audio_single_track: matches!(
            setting_str(&db, "audio_single_track", "0").as_str(),
            "1" | "true"
        ),
        container: if setting_str(&db, "container", "mp4") == "mkv" {
            "mkv".into()
        } else {
            "mp4".into()
        },
        vendor: video::vendor().await,
        base_width,
        base_height,
        portal_restore_token: overrides
            .game
            .as_ref()
            .and_then(|g| g.token.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| setting_str(&db, "engine_restore_token", "")),
        custom_encoder,
        custom_video,
        obs_bin: Some(obs_bin),
        websocket_port: port,
        websocket_password: password,
    })
}

async fn start_engine(app: &AppHandle, overrides: &StartOverrides) -> Result<EngineStatus, String> {
    let config = build_capture_config(app, overrides).await?;
    eprintln!(
        "[moonclip] obs engine: {}x{}@{} {} {}kbps replay={}s codec={} monitor={} audio={}",
        config.base_width,
        config.base_height,
        config.fps,
        if config.encoder == "cpu" {
            "x264"
        } else {
            "gpu"
        },
        config.bitrate_kbps,
        config.duration_seconds,
        config.codec,
        if config.monitor.is_empty() {
            "primary".into()
        } else {
            config.monitor.clone()
        },
        if config.audio_single_track {
            "Mix"
        } else {
            "Mix+Game+Mic"
        }
    );
    let mut engine = new_engine();
    let first_time = config.portal_restore_token.trim().is_empty();
    if let Err(e) = engine.start_buffer(config).await {
        eprintln!("[moonclip] obs start failed: {e}");
        return Err(e);
    }
    // Portal token + real canvas size: OBS refreshes the single-use restore
    // token on every successful Start, and the portal (not our profile)
    // decides the captured size. Persist both so the next start is silent and
    // pixel-correct. First run waits for the picker; later runs fail fast.
    let source_wait = if first_time {
        std::time::Duration::from_secs(60)
    } else {
        std::time::Duration::from_secs(15)
    };
    let mut waited = std::time::Duration::ZERO;
    let mut learned: Option<(u32, u32)> = None;
    while waited < source_wait {
        if let Some(size) = engine.detect_and_apply_source_size().await {
            learned = Some(size);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        waited += std::time::Duration::from_millis(500);
    }
    let Some((src_w, src_h)) = learned else {
        engine.stop_buffer().await.ok();
        return Err(
            "no screen captured: the system picker was cancelled or no stream was granted".into(),
        );
    };
    let now_ms = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    };
    if let Some(g) = &overrides.game {
        // Persist the detected per-game capture state (window/token) so the
        // next start restores silently.
        let db = app.state::<DbState>();
        let _ = db.set_game_capture(
            &g.key,
            &g.title,
            g.source_kind.as_deref(),
            g.window_match.as_deref(),
            None,
            now_ms(),
        );
    }
    if let Some(token) = engine.read_restore_token().await {
        let db = app.state::<DbState>();
        let _ = db.set_setting("engine_restore_token", &token);
        if let Some(g) = &overrides.game {
            let _ = db.set_game_capture(
                &g.key,
                &g.title,
                g.source_kind.as_deref(),
                g.window_match.as_deref(),
                Some(&token),
                now_ms(),
            );
        }
        engine.note("screen token updated");
    }
    {
        let db = app.state::<DbState>();
        let _ = db.set_setting("engine_source_width", &src_w.to_string());
        let _ = db.set_setting("engine_source_height", &src_h.to_string());
    }
    let tracks = engine.tracks_linked();
    {
        let st = app.state::<AppState>();
        *st.recorder.lock().await = Some(engine);
    }
    set_engine_error(app, None).await;
    set_audio_error(app, None).await;
    Ok(EngineStatus {
        running: true,
        backend: backend_name().to_string(),
        tracks_linked: tracks,
        audio_error: None,
        engine_error: None,
    })
}

async fn stop_engine(app: &AppHandle) -> Result<EngineStatus, String> {
    let st = app.state::<AppState>();
    let mut guard = st.recorder.lock().await;
    if let Some(mut engine) = guard.take() {
        engine.stop_buffer().await?;
    }
    drop(guard);
    {
        let st = app.state::<AppState>();
        st.detect.lock().await.auto.auto_started = false;
    }
    set_audio_error(app, None).await;
    set_engine_error(app, None).await;
    Ok(EngineStatus {
        running: false,
        backend: backend_name().to_string(),
        tracks_linked: 0,
        audio_error: None,
        engine_error: None,
    })
}

/// Settings whose change restarts a running buffer so length, devices and
/// stored durations always match the recorder.
const RESTART_KEYS: &[&str] = &[
    "buffer_seconds",
    "mic_device",
    "desktop_device",
    "video_codec",
    "video_encoder",
    "gpu_index",
    "out_height",
    "fps",
    "monitor",
    "capture_window",
    "audio_single_track",
    "container",
    "video_mode",
    "custom_bitrate_kbps",
    "custom_fps",
    "custom_encoder_json",
    "custom_video_json",
    "gain_game",
    "gain_mic",
];

/// Restart the engine if it is running (used by every restart-key write path).
async fn restart_if_running(app: &AppHandle) -> Result<bool, String> {
    let running = {
        let st = app.state::<AppState>();
        let guard = st.recorder.lock().await;
        let r = guard.is_some();
        drop(guard);
        r
    };
    if !running {
        return Ok(false);
    }
    stop_engine(app).await?;
    start_engine(app, &StartOverrides::default()).await?;
    notify(
        app,
        "Búfer reiniciado con la nueva configuración",
        "Buffer restarted with the new configuration",
    );
    Ok(true)
}

#[tauri::command]
pub async fn set_setting(app: AppHandle, key: String, value: String) -> Result<(), String> {
    // Keep the previous value: a restart with an unusable new value (device
    // unplugged, encoder missing) must not leave the buffer stopped.
    let previous = {
        let db = app.state::<DbState>();
        db.get_settings().ok().and_then(|s| s.get(&key).cloned())
    };
    if key == "custom_encoder_json" || key == "custom_video_json" {
        let mut proposed = HashMap::new();
        proposed.insert(key.clone(), value.clone());
        validate_custom_pair(&app, &proposed).await?;
    }
    {
        let db = app.state::<DbState>();
        db.set_setting(&key, &value)?;
    }
    if RESTART_KEYS.contains(&key.as_str()) {
        if let Err(e) = restart_if_running(&app).await {
            if let Some(prev) = previous.as_deref() {
                let db = app.state::<DbState>();
                let _ = db.set_setting(&key, prev);
                if start_engine(&app, &StartOverrides::default()).await.is_ok() {
                    notify(
                        &app,
                        "Cambio no aplicado; se restauró la configuración anterior",
                        "Change not applied; previous configuration restored",
                    );
                }
            }
            return Err(e);
        }
    }
    Ok(())
}

/// Atomic video-quality change (preset card): writes codec, height and fps
/// together so a running buffer restarts ONCE, with the same
/// revert-on-failure semantics as `set_setting`.
#[tauri::command]
pub async fn set_video_quality(
    app: AppHandle,
    codec: String,
    height: u32,
    fps: u32,
) -> Result<(), String> {
    if !["h264", "hevc", "av1"].contains(&codec.as_str()) {
        return Err("unknown codec".into());
    }
    if ![24, 30, 60, 120, 144].contains(&fps) {
        return Err("fps must be one of 24/30/60/120/144".into());
    }
    if height != 0 && !crate::video_quality::HEIGHTS.contains(&height) {
        return Err("unknown height".into());
    }
    let pairs: [(&str, String); 4] = [
        ("video_codec", codec),
        ("out_height", height.to_string()),
        ("fps", fps.to_string()),
        // Picking a ladder cell exits custom mode; otherwise the custom
        // bitrate/fps would silently override the user's choice.
        ("video_mode", "ladder".to_string()),
    ];
    let previous: Vec<(&str, Option<String>)> = {
        let db = app.state::<DbState>();
        let settings = db.get_settings().unwrap_or_default();
        pairs
            .iter()
            .map(|(k, _)| (*k, settings.get(*k).cloned()))
            .collect()
    };
    {
        let db = app.state::<DbState>();
        for (key, value) in &pairs {
            db.set_setting(key, value)?;
        }
    }
    if let Err(e) = restart_if_running(&app).await {
        {
            let db = app.state::<DbState>();
            for (key, prev) in &previous {
                if let Some(p) = prev {
                    let _ = db.set_setting(key, p);
                }
            }
        }
        if start_engine(&app, &StartOverrides::default()).await.is_ok() {
            notify(
                &app,
                "Cambio no aplicado; se restauró la configuración anterior",
                "Change not applied; previous configuration restored",
            );
        }
        return Err(e);
    }
    Ok(())
}

/// Free physical memory for the settings warning (`free_mb: null` when the
/// platform cannot report it).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SystemMemory {
    pub free_mb: Option<u64>,
}

#[tauri::command]
pub fn system_memory() -> SystemMemory {
    SystemMemory {
        free_mb: crate::os::memory_free_mb(),
    }
}

/// One `key=value` setting for the bulk command.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SettingPair {
    pub key: String,
    pub value: String,
}

/// Atomic multi-setting write (custom mode / container): all keys land
/// together and a running buffer restarts ONCE, with the same
/// revert-on-failure semantics as `set_setting`.
#[tauri::command]
pub async fn set_settings(app: AppHandle, values: Vec<SettingPair>) -> Result<(), String> {
    if values.is_empty() {
        return Ok(());
    }
    // Custom payloads validate BEFORE anything is written (atomic).
    if values
        .iter()
        .any(|p| p.key == "custom_encoder_json" || p.key == "custom_video_json")
    {
        let proposed: HashMap<String, String> = values
            .iter()
            .map(|p| (p.key.clone(), p.value.clone()))
            .collect();
        validate_custom_pair(&app, &proposed).await?;
    }
    let previous: Vec<(String, Option<String>)> = {
        let db = app.state::<DbState>();
        let settings = db.get_settings().unwrap_or_default();
        values
            .iter()
            .map(|p| (p.key.clone(), settings.get(&p.key).cloned()))
            .collect()
    };
    {
        let db = app.state::<DbState>();
        for pair in &values {
            db.set_setting(&pair.key, &pair.value)?;
        }
    }
    if !values
        .iter()
        .any(|p| RESTART_KEYS.contains(&p.key.as_str()))
    {
        return Ok(());
    }
    if let Err(e) = restart_if_running(&app).await {
        {
            let db = app.state::<DbState>();
            for (key, prev) in &previous {
                if let Some(p) = prev {
                    let _ = db.set_setting(key, p);
                }
            }
        }
        if start_engine(&app, &StartOverrides::default()).await.is_ok() {
            notify(
                &app,
                "Cambio no aplicado; se restauró la configuración anterior",
                "Change not applied; previous configuration restored",
            );
        }
        return Err(e);
    }
    Ok(())
}

/// Running game candidates for the picker (read-only; already blacklist
/// filtered). Keeps processes that use the GPU, come from Steam/Wine or own
/// an X11 window.
#[tauri::command]
pub fn get_running_applications(
    db: State<'_, DbState>,
) -> Result<Vec<os::shared::detect::ResolvedCandidate>, String> {
    let cands = os::detect_candidates();
    let mut resolved = os::resolve_candidates(cands.clone());
    let apps = db.list_custom_apps()?;
    for (r, c) in resolved.iter_mut().zip(cands.iter()) {
        *r = os::shared::detect::matcher::with_custom(r.clone(), c, &apps);
    }
    resolved.retain(|c| {
        c.uses_gpu || c.steam_app_id.is_some() || c.is_wine || c.window_match.is_some()
    });
    resolved.sort_by(|a, b| a.title.cmp(&b.title));
    Ok(resolved)
}

/// Wall-clock milliseconds (diagnostics / last_seen).
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Detection worker tick (every ~3 s): resolve running candidates, apply the
/// user registrations, emit game changes and drive the Medal-style auto
/// buffer. Never touches the user's OBS; window targeting comes from the
/// per-game row (token/window_match).
pub(crate) async fn detect_tick(app: &AppHandle) {
    let cands = os::detect_candidates();
    let mut resolved = os::resolve_candidates(cands.clone());
    let apps = match app.try_state::<DbState>() {
        Some(db) => db.list_custom_apps().unwrap_or_default(),
        None => Vec::new(),
    };
    for (r, c) in resolved.iter_mut().zip(cands.iter()) {
        *r = os::shared::detect::matcher::with_custom(r.clone(), c, &apps);
    }
    let best = os::shared::detect::auto::pick_game(&resolved).cloned();
    let st = app.state::<AppState>();
    let running = st.recorder.lock().await.is_some();
    let auto_game = best.as_ref().map(os::shared::detect::auto::auto_game);
    let (action, changed) = {
        let mut det = st.detect.lock().await;
        let action = os::shared::detect::auto::decide(
            &mut det.auto,
            os::shared::detect::auto::AutoInput {
                now_ms: now_ms(),
                detected: auto_game.as_ref(),
                buffer_running: running,
            },
        );
        let before = det
            .current
            .as_ref()
            .map(|c| (c.game_key.clone(), c.title.clone()));
        let after = best
            .as_ref()
            .map(|c| (c.game_key.clone(), c.title.clone()));
        let changed = before != after;
        if changed {
            det.current = best.clone();
        }
        (action, changed)
    };
    if changed {
        let _ = app.emit("moonclip://game-changed", &best);
    }
    match action {
        os::shared::detect::auto::AutoAction::Start => {
            let Some(r) = best.as_ref() else { return };
            let saved = apps
                .iter()
                .find(|a| a.game_key.as_deref() == Some(r.game_key.as_str()));
            let (source_kind, window_match) = match (
                r.window_match.clone(),
                saved.and_then(|a| a.window_match.clone()),
            ) {
                (Some(m), _) => ("x11", Some(m)),
                (None, Some(m)) => ("x11", Some(m)),
                (None, None) => ("portal", None),
            };
            let ctx = GameContext {
                key: r.game_key.clone(),
                title: r.title.clone(),
                source_kind: Some(source_kind.to_string()),
                window_match,
                token: saved.and_then(|a| a.portal_token.clone()),
                duration_seconds: saved
                    .and_then(|a| a.clip_duration_seconds)
                    .map(|d| d.clamp(5, 3600) as u32),
            };
            let overrides = StartOverrides {
                game: Some(ctx),
                ..Default::default()
            };
            if let Err(e) = start_engine(app, &overrides).await {
                eprintln!("[moonclip] auto-buffer start failed: {e}");
            }
        }
        os::shared::detect::auto::AutoAction::Stop => {
            if let Err(e) = stop_engine(app).await {
                eprintln!("[moonclip] auto-buffer stop failed: {e}");
            }
        }
        os::shared::detect::auto::AutoAction::None => {}
    }
}

/// Last game picked by the detection worker (null when none).
#[tauri::command]
pub async fn current_game(app: AppHandle) -> Option<os::shared::detect::ResolvedCandidate> {
    let st = app.state::<AppState>();
    let det = st.detect.lock().await;
    det.current.clone()
}

#[tauri::command]
pub fn list_custom_apps(db: State<'_, DbState>) -> Result<Vec<CustomApp>, String> {
    db.list_custom_apps()
}

#[tauri::command]
pub fn register_app(db: State<'_, DbState>, input: RegisterAppInput) -> Result<CustomApp, String> {
    db.register_app(input)
}

#[tauri::command]
pub fn delete_app(db: State<'_, DbState>, id: String) -> Result<(), String> {
    db.delete_app(&id)
}

#[tauri::command]
pub fn secret_store(alias: String, value: String) -> Result<(), String> {
    secrets::store_secret(&alias, &value)
}

#[tauri::command]
pub fn secret_get(alias: String) -> Result<String, String> {
    secrets::get_secret(&alias)
}

#[tauri::command]
pub fn secret_delete(alias: String) -> Result<(), String> {
    secrets::delete_secret(&alias)
}

// ---------------------------------------------------------------------------
// Capture control
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct EngineStatus {
    pub running: bool,
    pub backend: String,
    /// Audio tracks the running configuration records (0, 1 or 3).
    pub tracks_linked: usize,
    /// Last audio-gain apply error, if any.
    pub audio_error: Option<String>,
    /// Last engine death/exit error, if any (cleared on next start).
    pub engine_error: Option<String>,
}

async fn read_audio_error(app: &AppHandle) -> Option<String> {
    let st = app.try_state::<AppState>()?;
    let guard = st.audio_error.lock().await;
    guard.clone()
}

async fn set_audio_error(app: &AppHandle, err: Option<String>) {
    if let Some(st) = app.try_state::<AppState>() {
        *st.audio_error.lock().await = err;
    }
}

async fn read_engine_error(app: &AppHandle) -> Option<String> {
    let st = app.try_state::<AppState>()?;
    let guard = st.engine_error.lock().await;
    guard.clone()
}

async fn set_engine_error(app: &AppHandle, err: Option<String>) {
    if let Some(st) = app.try_state::<AppState>() {
        *st.engine_error.lock().await = err;
    }
}

/// Human-readable cause from the last OBS log lines (best effort).
fn engine_death_reason(tail: &[String]) -> String {
    let last = tail.iter().rev().find(|line| {
        let l = line.to_lowercase();
        l.contains("error") || l.contains("failed") || l.contains("warning")
    });
    match last {
        Some(line) => format!("capture engine exited: {line}"),
        None => "capture engine exited unexpectedly".to_string(),
    }
}

fn is_spanish(app: &AppHandle) -> bool {
    app.try_state::<DbState>()
        .and_then(|db| db.get_settings().ok())
        .and_then(|s| s.get("locale").cloned())
        .map(|l| l.starts_with("es"))
        .unwrap_or(true)
}

pub fn notify(app: &AppHandle, body_es: &str, body_en: &str) {
    use tauri_plugin_notification::NotificationExt;
    let body = if is_spanish(app) { body_es } else { body_en };
    let _ = app
        .notification()
        .builder()
        .title("MoonClip")
        .body(body)
        .show();
}

#[tauri::command]
pub async fn start_buffer(app: AppHandle) -> Result<EngineStatus, String> {
    {
        let st = app.state::<AppState>();
        if st.recorder.lock().await.is_some() {
            return Err("buffer already running".into());
        }
    }
    let res = start_engine(&app, &StartOverrides::default()).await;
    if res.is_ok() {
        // Manual session: the autopilot must never stop it.
        let st = app.state::<AppState>();
        st.detect.lock().await.auto.auto_started = false;
    }
    res
}

#[tauri::command]
pub async fn stop_buffer(app: AppHandle) -> Result<EngineStatus, String> {
    stop_engine(&app).await
}

/// Forget the portal screen choice (Wayland "Change screen"): clears the
/// restore token + learned canvas so the next start shows the system picker
/// again. Restarts a running buffer so the picker appears immediately.
#[tauri::command]
pub async fn clear_portal_token(app: AppHandle) -> Result<EngineStatus, String> {
    let was_running = {
        let st = app.state::<AppState>();
        let guard = st.recorder.lock().await;
        guard.is_some()
    };
    if was_running {
        stop_engine(&app).await?;
    }
    {
        let db = app.state::<DbState>();
        db.set_setting("engine_restore_token", "")?;
        db.set_setting("engine_source_width", "")?;
        db.set_setting("engine_source_height", "")?;
    }
    // The generated collection carries the last RestoreToken OBS saved and
    // `write_obs_config` merges it back, so it must be dropped too or the next
    // start would restore silently and the picker would never appear.
    if let Ok(root) = os::obs_config_root() {
        let collection = root
            .join(os::engine_config_dir())
            .join("basic")
            .join("scenes")
            .join(format!("{}.json", obs::OBS_COLLECTION));
        let _ = std::fs::remove_file(&collection);
    }
    if was_running {
        start_engine(&app, &StartOverrides::default()).await?;
        notify(
            &app,
            "Elige la pantalla a capturar (solo esta vez)",
            "Pick the screen to capture (this time only)",
        );
    }
    engine_status(app).await
}

/// Liveness sweep: drop a dead engine and surface the reason. Returns whether
/// an engine is currently running. Used by `engine_status` AND by a backend
/// watchdog task, because the webview (and its 2 s poll) is paused while the
/// window is hidden to tray during gaming.
pub(crate) async fn sweep_engine_liveness(app: &AppHandle) -> bool {
    let st = app.state::<AppState>();
    let mut guard = st.recorder.lock().await;
    let had_engine = guard.is_some();
    let (alive, tail) = match guard.as_mut() {
        Some(engine) => {
            if engine.check_alive() {
                (true, Vec::new())
            } else {
                (false, engine.events_tail())
            }
        }
        None => (false, Vec::new()),
    };
    if had_engine && !alive {
        guard.take();
    }
    drop(guard);
    if had_engine && !alive {
        let reason = engine_death_reason(&tail);
        eprintln!("[moonclip] engine died: {reason}");
        set_engine_error(app, Some(reason.clone())).await;
        let _ = app.emit(
            "moonclip://engine-stopped",
            serde_json::json!({ "reason": reason }),
        );
        notify(
            app,
            "El motor de captura se detuvo inesperadamente",
            "Capture engine stopped unexpectedly",
        );
    }
    alive
}

#[tauri::command]
pub async fn engine_status(app: AppHandle) -> Result<EngineStatus, String> {
    let alive = sweep_engine_liveness(&app).await;
    let tracks = if alive {
        let st = app.state::<AppState>();
        let guard = st.recorder.lock().await;
        guard.as_ref().map(|e| e.tracks_linked()).unwrap_or(0)
    } else {
        0
    };
    Ok(EngineStatus {
        running: alive,
        backend: backend_name().to_string(),
        tracks_linked: tracks,
        audio_error: read_audio_error(&app).await,
        engine_error: read_engine_error(&app).await,
    })
}

/// Full save pipeline: OBS save -> dedupe -> thumbnail -> DB index -> ding ->
/// event. Stage timings go to the backend log (`[moonclip] save ...`).
pub(crate) async fn do_save_clip(app: &AppHandle) -> Result<ClipRecord, String> {
    // One save at a time: a second F9 while the first is mid-pipeline must
    // queue, not interleave (per-second clip names + DB insert).
    let state = app.state::<AppState>();
    let _save_guard = state.save_lock.lock().await;
    let t_total = std::time::Instant::now();
    let path = {
        let st = app.state::<AppState>();
        let mut guard = st.recorder.lock().await;
        let eng = guard
            .as_mut()
            .ok_or_else(|| "buffer not running".to_string())?;
        eng.save_clip().await?
    };
    let t_engine = t_total.elapsed();
    let db = app.state::<DbState>();
    // Same-second double saves collide: OBS names replay files by timestamp,
    // so the second file could overwrite the first on disk and the DB would
    // reject the duplicate. Rename to stem_2.mp4, stem_3.mp4… instead of
    // failing and losing the clip.
    let mut path = path;
    {
        let taken: std::collections::HashSet<String> = db
            .list_clips()
            .map(|clips| clips.into_iter().map(|c| c.file_name).collect())
            .unwrap_or_default();
        if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
            if taken.contains(name) {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or("bad clip file name")?
                    .to_string();
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("mp4");
                let mut n = 2u32;
                loop {
                    let cand = path.with_file_name(format!("{stem}_{n}.{ext}"));
                    let cand_name = cand
                        .file_name()
                        .and_then(|s| s.to_str())
                        .ok_or("bad clip file name")?;
                    if !taken.contains(cand_name) && !cand.exists() {
                        tokio::fs::rename(&path, &cand)
                            .await
                            .map_err(|e| format!("cannot dedupe clip name: {e}"))?;
                        path = cand;
                        break;
                    }
                    n += 1;
                    if n > 999 {
                        return Err("cannot find a free clip name".into());
                    }
                }
            }
        }
    }
    let base = db.clips_dir()?;
    let ffmpeg = crate::editor::ffmpeg::resolve_ffmpeg(app)?;
    let size = tokio::fs::metadata(&path)
        .await
        .map_err(|e| format!("cannot stat clip: {e}"))?
        .len() as i64;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("bad clip file name")?;
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("bad clip file name")?
        .to_string();
    let thumb_name = format!("thumb_{stem}.jpg");
    let thumb_path = base.join(&thumb_name);
    // Duration probe + thumbnail are independent (same input): run them
    // together instead of paying two cold ffmpeg spawns in series.
    // Thumbnail tries 1 s first; a sub-second clip retries near the head.
    let t_tail = std::time::Instant::now();
    let (probe_ms, thumb_res) = tokio::join!(
        crate::editor::ffmpeg::probe_duration_ms(&ffmpeg, &path),
        crate::editor::ffmpeg::make_thumbnail(&ffmpeg, &path, &thumb_path, 1.0),
    );
    // Real measured duration (the buffer is rarely full at save time).
    // Falls back to the configured length only if probing fails.
    let secs_ms = probe_ms.unwrap_or_else(|| buffer_seconds(&db) * 1000);
    if let Err(e) = thumb_res {
        if secs_ms < 1500 {
            crate::editor::ffmpeg::make_thumbnail(&ffmpeg, &path, &thumb_path, 0.05).await?;
        } else {
            return Err(e);
        }
    }
    let t_tail_elapsed = t_tail.elapsed();
    let t_db = std::time::Instant::now();
    let clip = db.insert_clip(&file_name, &thumb_name, "Unknown", secs_ms, size)?;
    eprintln!(
        "[moonclip] save total={:?} engine={t_engine:?} probe+thumb={t_tail_elapsed:?} db={:?} size={}MB",
        t_total.elapsed(),
        t_db.elapsed(),
        size / 1024 / 1024
    );
    {
        let st = app.state::<AppState>();
        let guard = st.recorder.lock().await;
        if let Some(eng) = guard.as_ref() {
            eng.note(&format!("clip saved: {file_name}"));
        }
    }
    crate::cue::play_ding();
    let _ = app.emit("moonclip://clip-saved", &clip);
    Ok(clip)
}

#[tauri::command]
pub async fn save_clip_now(app: AppHandle) -> Result<ClipRecord, String> {
    do_save_clip(&app).await
}

/// One-time correction: measure real durations for rows saved before probing
/// existed (the settings value was stored instead). Skips missing files and
/// rows already within 1.5 s of measured. Runs once at boot in background.
pub(crate) async fn backfill_durations(app: &AppHandle) {
    let Some(db) = app.try_state::<DbState>() else {
        return;
    };
    let Ok(clips) = db.list_clips() else { return };
    let Ok(base) = db.clips_dir() else { return };
    let Ok(ffmpeg) = crate::editor::ffmpeg::resolve_ffmpeg(app) else {
        return;
    };
    for clip in clips {
        let path = base.join(&clip.file_name);
        if !path.exists() {
            continue;
        }
        if let Some(ms) = crate::editor::ffmpeg::probe_duration_ms(&ffmpeg, &path).await {
            if (ms - clip.duration_ms).abs() > 1500 {
                let _ = db.update_duration(&clip.id, ms);
            }
        }
    }
}

/// F9 entry point: counter event always fires; clip saves only when running.
pub(crate) async fn handle_hotkey(app: AppHandle, shortcut: String, pressed_at: String) {
    let _ = app.emit(
        "moonclip://clip-hotkey",
        serde_json::json!({ "shortcut": shortcut, "pressed_at": pressed_at }),
    );
    let st = app.state::<AppState>();
    let guard = st.recorder.lock().await;
    let running = guard.is_some();
    drop(guard);
    if !running {
        notify(
            &app,
            "Búfer detenido — pulsa Start para grabar",
            "Buffer stopped — press Start to record",
        );
        return;
    }
    match do_save_clip(&app).await {
        Ok(clip) => notify(
            &app,
            &format!("Clip guardado: {}", clip.file_name),
            &format!("Clip saved: {}", clip.file_name),
        ),
        Err(e) => notify(
            &app,
            &format!("Error al guardar: {e}"),
            &format!("Save failed: {e}"),
        ),
    }
}

// ---------------------------------------------------------------------------
// Live capture gain + backend info
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct TrackGains {
    pub game: u32,
    pub mic: u32,
    pub mute_game: bool,
    pub mute_mic: bool,
}

fn read_gains(app: &AppHandle) -> TrackGains {
    let map = app
        .try_state::<DbState>()
        .and_then(|db| db.get_settings().ok())
        .unwrap_or_default();
    let num = |k: &str, d: u32| {
        map.get(k)
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
            .clamp(0, 200)
    };
    let flag = |k: &str| map.get(k).map(|v| v == "1" || v == "true").unwrap_or(false);
    TrackGains {
        game: num("gain_game", 100),
        mic: num("gain_mic", 100),
        mute_game: flag("mute_game"),
        mute_mic: flag("mute_mic"),
    }
}

#[tauri::command]
pub async fn audio_levels(app: AppHandle) -> Result<TrackGains, String> {
    Ok(read_gains(&app))
}

/// Live signal peaks (linear 0.0–1.0+) per captured stream, for the UI
/// meters. `null` when the backend does not expose them (OBS engine).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AudioPeaks {
    pub game: f32,
    pub mic: f32,
}

#[tauri::command]
pub async fn audio_peaks() -> Result<Option<AudioPeaks>, String> {
    Ok(None)
}

fn check_track(track: &str) -> Result<(), String> {
    if track == "game" || track == "mic" {
        Ok(())
    } else {
        Err("track must be 'game' or 'mic'".into())
    }
}

/// Gain applies live through obs-websocket while the buffer runs (no
/// restart). When the live apply fails, it falls back to the single-restart
/// path. Persisted either way, so the next start renders it in the scene.
#[tauri::command]
pub async fn set_track_gain(
    app: AppHandle,
    track: String,
    percent: u32,
) -> Result<TrackGains, String> {
    check_track(&track)?;
    let pct = percent.clamp(0, 200);
    {
        let db = app.state::<DbState>();
        db.set_setting(
            if track == "game" {
                "gain_game"
            } else {
                "gain_mic"
            },
            &pct.to_string(),
        )?;
    }
    let running = {
        let st = app.state::<AppState>();
        let guard = st.recorder.lock().await;
        guard.is_some()
    };
    if running {
        let live: Result<(), String> = {
            let st = app.state::<AppState>();
            let mut guard = st.recorder.lock().await;
            match guard.as_mut() {
                Some(engine) => engine.set_volume(&track, pct).await,
                None => Err("engine stopped".into()),
            }
        };
        match live {
            Ok(()) => {
                set_audio_error(&app, None).await;
                return Ok(read_gains(&app));
            }
            Err(e) => {
                eprintln!("[moonclip] live gain failed ({e}), restarting buffer");
            }
        }
    }
    if let Err(e) = restart_if_running(&app).await {
        set_audio_error(&app, Some(e.clone())).await;
        return Err(e);
    }
    set_audio_error(&app, None).await;
    Ok(read_gains(&app))
}

/// Mutes apply live through obs-cmd when the buffer runs, and persist for the
/// next start either way.
#[tauri::command]
pub async fn set_track_mute(
    app: AppHandle,
    track: String,
    muted: bool,
) -> Result<TrackGains, String> {
    check_track(&track)?;
    {
        let db = app.state::<DbState>();
        db.set_setting(
            if track == "game" {
                "mute_game"
            } else {
                "mute_mic"
            },
            if muted { "1" } else { "0" },
        )?;
    }
    let running = {
        let st = app.state::<AppState>();
        let guard = st.recorder.lock().await;
        let r = guard.is_some();
        drop(guard);
        r
    };
    if running {
        let st = app.state::<AppState>();
        let mut guard = st.recorder.lock().await;
        if let Some(engine) = guard.as_mut() {
            if let Err(e) = engine.set_mute(&track, muted).await {
                drop(guard);
                set_audio_error(&app, Some(e.clone())).await;
                return Err(e);
            }
        }
    }
    set_audio_error(&app, None).await;
    Ok(read_gains(&app))
}

/// Embedded OBS status for the Settings UI ("Motor OBS (aislado)").
#[derive(Debug, Clone, serde::Serialize)]
pub struct ObsInfo {
    /// Capture engine resolved (bundled or env override).
    pub present: bool,
    /// Version line reported by the engine binary (empty when it cannot be
    /// probed).
    pub version: String,
    /// MoonClip-owned config dir (never another app's config).
    pub config_dir: String,
    pub profile: String,
    pub collection: String,
    pub websocket_port: u16,
    /// bundled | env | missing
    pub source: String,
    /// Bounded engine activity tail (MoonClip's own events).
    pub events_tail: Vec<String>,
}

#[tauri::command]
pub async fn obs_info(app: AppHandle) -> Result<ObsInfo, String> {
    let db = app.state::<DbState>();
    let port = setting_str(&db, "engine_ws_port", "4456")
        .parse()
        .unwrap_or(4456);
    let config_root = os::obs_config_root().unwrap_or_default();
    let (present, version, source) =
        match resolve_obs(&app) {
            Ok((bin, src)) => {
                let mut cmd = tokio::process::Command::new(&bin);
                cmd.arg("--version").kill_on_drop(true);
                let version =
                    match tokio::time::timeout(std::time::Duration::from_secs(15), cmd.output())
                        .await
                    {
                        Ok(Ok(out)) => {
                            let mut t = String::from_utf8_lossy(&out.stdout).trim().to_string();
                            if t.is_empty() {
                                t = String::from_utf8_lossy(&out.stderr).trim().to_string();
                            }
                            t.lines().next().unwrap_or_default().to_string()
                        }
                        _ => String::new(),
                    };
                (true, version, src.to_string())
            }
            Err(_) => (false, String::new(), "missing".to_string()),
        };
    Ok(ObsInfo {
        present,
        version,
        config_dir: config_root.to_string_lossy().to_string(),
        profile: obs::OBS_PROFILE.to_string(),
        collection: obs::OBS_COLLECTION.to_string(),
        websocket_port: port,
        source,
        events_tail: {
            let st = app.state::<AppState>();
            let guard = st.recorder.lock().await;
            guard.as_ref().map(|e| e.events_tail()).unwrap_or_default()
        },
    })
}

/// One-click repair: stop the buffer and remove ONLY the MoonClip-owned OBS
/// config (the user's own OBS is never touched).
#[tauri::command]
pub async fn repair_obs_config(app: AppHandle) -> Result<(), String> {
    stop_engine(&app).await?;
    let root = os::obs_config_root()?;
    obs::reset_obs_config(&root)?;
    eprintln!("[moonclip] engine config reset: {}", root.display());
    Ok(())
}

/// Capture devices (Windows: WASAPI enumeration; Linux: pactl/PipeWire).
#[tauri::command]
pub async fn list_audio_devices(app: AppHandle) -> Result<Vec<AudioDevice>, String> {
    devices::list_audio_devices(&app).await
}

/// Video options for the Settings UI: codec ids from the backend, ladder
/// heights, Medal bitrates/ranges and exact RAM estimates (CBR => exact).
/// NOTE: no human text crosses IPC — labels/notes live in frontend locales.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CodecOpt {
    pub id: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HeightOpt {
    pub height: u32,
    pub label: String,
    /// CBR kbps per codec, in codec order.
    pub bitrates: Vec<u32>,
    /// Medal recommended (min,max) kbps per codec, in codec order.
    pub recommended: Vec<(u32, u32)>,
    /// Exact 60 s ring megabytes per codec, in codec order.
    pub ring_mb_60s: Vec<u32>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MonitorOpt {
    pub name: String,
    pub label: String,
}

/// One OBS encoder for the Custom picker (single source of truth: the
/// pinned per-platform catalog + GPU vendor + live ffmpeg probe).
#[derive(Debug, Clone, serde::Serialize)]
pub struct EncoderOpt {
    pub id: String,
    pub codec: String,
    pub family: String,
    pub validated: bool,
    pub available: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct VisibleWhen {
    pub key: String,
    pub values: Vec<String>,
    pub and_key: Option<String>,
    pub and_values: Vec<String>,
}

/// One registry option serialized for the dynamic Custom form, already
/// resolved for one concrete encoder (codec-scoped values/ranges/defaults).
#[derive(Debug, Clone, serde::Serialize)]
pub struct OptionSpecJson {
    pub key: String,
    pub kind: String,
    pub values: Vec<String>,
    pub int_values: Vec<i64>,
    pub min: i64,
    pub max: i64,
    pub step: i64,
    pub default: Option<serde_json::Value>,
    pub i18n: String,
    pub visible_when: Option<VisibleWhen>,
    /// False when the option does not exist for this codec: the UI renders
    /// it greyed out and never sends it; the backend rejects it.
    pub supported: bool,
    /// Values only valid with P010 color format (greyed out otherwise).
    pub p010_values: Vec<String>,
    pub p010_ints: Vec<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EncoderSchema {
    pub encoder: String,
    pub family: String,
    pub codec: String,
    pub validated: bool,
    pub options: Vec<OptionSpecJson>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EncoderColors {
    pub id: String,
    pub formats: Vec<String>,
}

/// Current Custom selection (validated payloads, empty when ladder).
#[derive(Debug, Clone, serde::Serialize)]
pub struct CustomSelection {
    pub encoder: String,
    pub settings: serde_json::Map<String, serde_json::Value>,
    pub video: Option<CustomVideo>,
}

fn family_slug(f: enc::EncoderFamily) -> &'static str {
    match f {
        enc::EncoderFamily::Nvenc => "nvenc",
        enc::EncoderFamily::X264 => "x264",
        enc::EncoderFamily::Qsv => "qsv",
        enc::EncoderFamily::Amf => "amf",
        enc::EncoderFamily::Vaapi => "vaapi",
    }
}

fn spec_json(r: &enc::ResolvedOption) -> OptionSpecJson {
    let kind = match r.kind {
        enc::SpecKind::Enum => "enum",
        enc::SpecKind::Int => "int",
        enc::SpecKind::Bool => "bool",
        enc::SpecKind::Text => "text",
    };
    let default = match r.default {
        Some(enc::SpecDefault::Str(v)) => Some(serde_json::Value::from(v)),
        Some(enc::SpecDefault::Int(v)) => Some(serde_json::Value::from(v)),
        Some(enc::SpecDefault::Bool(v)) => Some(serde_json::Value::from(v)),
        None => None,
    };
    let visible_when = r.visible.when.map(|(k, vs)| {
        let (and_key, and_values) = r
            .visible
            .and_when
            .map(|(ak, avs)| {
                (
                    Some(ak.to_string()),
                    avs.iter().map(|v| v.to_string()).collect::<Vec<_>>(),
                )
            })
            .unwrap_or((None, Vec::new()));
        VisibleWhen {
            key: k.to_string(),
            values: vs.iter().map(|v| v.to_string()).collect(),
            and_key,
            and_values,
        }
    });
    OptionSpecJson {
        key: r.key.to_string(),
        kind: kind.to_string(),
        values: r.values.iter().map(|v| v.to_string()).collect(),
        int_values: r.int_values.clone(),
        min: r.min,
        max: r.max,
        step: r.step,
        default,
        i18n: r.i18n.to_string(),
        visible_when,
        supported: r.supported,
        p010_values: r.p010_values.iter().map(|v| v.to_string()).collect(),
        p010_ints: r.p010_ints.clone(),
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct VideoOptions {
    pub codecs: Vec<CodecOpt>,
    pub heights: Vec<HeightOpt>,
    pub monitors: Vec<MonitorOpt>,
    pub current_codec: String,
    pub current_height: u32,
    pub current_fps: u32,
    pub current_monitor: String,
    /// Wayland portal: a persisted screen choice exists (silent restore).
    /// `false` = the next start shows the system picker once.
    pub portal_ready: bool,
    /// With the OBS engine the buffer delivers at the requested resolution
    /// (GPU scaling inside OBS); kept for UI parity with the old backend.
    pub buffer_height: u32,
    pub transcoding: bool,
    pub max_source_height: u32,
    pub vendor: String,
    /// Encoder preference: gpu | cpu.
    pub encoder: String,
    /// Hardware monitors can be oversampled (24..144).
    pub fps_options: Vec<u32>,
    /// Custom picker: pinned catalog entries with live availability.
    pub encoders: Vec<EncoderOpt>,
    /// Custom form: option schema resolved per encoder id (codec-scoped
    /// values/ranges/defaults; unsupported options carry supported=false).
    pub encoder_schema: Vec<EncoderSchema>,
    /// Color formats each catalog encoder accepts.
    pub encoder_colors: Vec<EncoderColors>,
    /// [Video] tab enums.
    pub scale_filters: Vec<String>,
    pub fps_types: Vec<String>,
    pub fps_common_values: Vec<u32>,
    pub color_spaces: Vec<String>,
    pub color_ranges: Vec<String>,
    /// Current Custom selection (None fields when ladder).
    pub custom: Option<CustomSelection>,
}

#[tauri::command]
pub async fn video_options(app: AppHandle) -> Result<VideoOptions, String> {
    use crate::video_quality as q;
    let vendor = video::vendor().await;

    // Codec ids the platform can offer, filtered to known-good entries.
    // Labels/notes are frontend-owned (locales) — never hardcode UI text here.
    let ffmpeg = crate::editor::ffmpeg::resolve_ffmpeg(&app)
        .unwrap_or_else(|_| std::path::PathBuf::from("ffmpeg"));
    let mut codecs: Vec<CodecOpt> = Vec::new();
    for id in video::offered_codecs(&ffmpeg).await {
        if matches!(id.as_str(), "h264" | "hevc" | "av1" | "x264")
            && !codecs.iter().any(|c| c.id == id)
        {
            codecs.push(CodecOpt { id });
        }
    }
    if codecs.is_empty() {
        // H.264 always exists in an OBS install.
        codecs.push(CodecOpt { id: "h264".into() });
    }

    let db = app.state::<DbState>();
    let current_codec = match setting_str(&db, "video_codec", "h264").as_str() {
        id @ ("h264" | "hevc" | "av1") => id.to_string(),
        "x264" => "h264".to_string(),
        _ => "h264".to_string(),
    };
    let current_height: u32 = setting_str(&db, "out_height", "0").parse().unwrap_or(0);
    let current_fps: u32 = match setting_str(&db, "fps", "60").parse().unwrap_or(60) {
        24 => 24,
        30 => 30,
        120 => 120,
        144 => 144,
        _ => 60,
    };
    let encoder = match setting_str(&db, "video_encoder", "gpu").as_str() {
        "cpu" => "cpu".to_string(),
        _ => {
            if setting_str(&db, "video_codec", "") == "x264" {
                "cpu".to_string()
            } else {
                "gpu".to_string()
            }
        }
    };
    let listed = video::list_monitors().await;
    let max_source_height = listed.iter().map(|m| m.height).max().unwrap_or(0);
    let current_monitor = setting_str(&db, "monitor", "");
    let monitors = listed
        .iter()
        .map(|m| MonitorOpt {
            name: m.name.clone(),
            label: m.label.clone(),
        })
        .collect::<Vec<_>>();
    let heights = q::HEIGHTS
        .iter()
        .map(|&h| {
            let bitrates = codecs
                .iter()
                .map(|c| q::bitrate_kbps(h, &c.id))
                .collect::<Vec<_>>();
            let recommended = codecs
                .iter()
                .map(|c| q::recommended_kbps(h, &c.id))
                .collect::<Vec<_>>();
            let ring = bitrates
                .iter()
                .map(|&b| q::ring_mb(b, 60))
                .collect::<Vec<_>>();
            HeightOpt {
                height: h,
                label: format!("{h}p"),
                bitrates,
                recommended,
                ring_mb_60s: ring,
            }
        })
        .collect();

    // Custom picker: pinned catalog filtered by detected vendor and the
    // live codec probe (x264 needs no GPU, so it is always available).
    let families = enc::families_for_vendor(&vendor);
    let offered: Vec<String> = codecs.iter().map(|c| c.id.clone()).collect();
    let encoders = crate::os::encoder_catalog()
        .iter()
        .map(|e| {
            let in_family = families.contains(&e.family);
            let codec_ok = offered.iter().any(|c| c == e.codec) || e.id == "obs_x264";
            EncoderOpt {
                id: e.id.to_string(),
                codec: e.codec.to_string(),
                family: family_slug(e.family).to_string(),
                validated: enc::family_validated(e.family),
                available: in_family && codec_ok,
            }
        })
        .collect::<Vec<_>>();
    let encoder_schema = crate::os::encoder_catalog()
        .iter()
        .map(|e| EncoderSchema {
            encoder: e.id.to_string(),
            family: family_slug(e.family).to_string(),
            codec: e.codec.to_string(),
            validated: enc::family_validated(e.family),
            options: enc::resolved_options(e.family, e.codec)
                .iter()
                .map(spec_json)
                .collect(),
        })
        .collect::<Vec<_>>();
    let encoder_colors = crate::os::encoder_catalog()
        .iter()
        .map(|e| EncoderColors {
            id: e.id.to_string(),
            formats: enc::valid_color_formats(e.family, e.codec)
                .iter()
                .map(|s| s.to_string())
                .collect(),
        })
        .collect::<Vec<_>>();

    // Current Custom selection (validated payloads; absent when ladder).
    let custom = (|| -> Option<CustomSelection> {
        let ce_raw = setting_str(&db, "custom_encoder_json", "");
        if ce_raw.trim().is_empty() {
            return None;
        }
        let parsed = enc::parse_custom_encoder(&ce_raw).ok()?;
        let entry = enc::lookup_encoder(&parsed.encoder)?;
        let cv_raw = setting_str(&db, "custom_video_json", "");
        let video = if cv_raw.trim().is_empty() {
            None
        } else {
            let v: CustomVideo = serde_json::from_str(&cv_raw).ok()?;
            Some(v)
        };
        // Re-validate the pair so a stale DB can never reach the UI as valid
        // (settings + 10-bit profile gate against the stored video color).
        let video_color = video.as_ref().map(|v| v.color_format.as_str());
        enc::validate_pair(entry.family, entry.codec, &parsed.settings, video_color).ok()?;
        if let Some(v) = &video {
            let raw = serde_json::to_string(v).ok()?;
            enc::parse_custom_video(entry.family, entry.codec, &raw).ok()?;
        }
        Some(CustomSelection {
            encoder: parsed.encoder,
            settings: parsed.settings,
            video,
        })
    })();

    Ok(VideoOptions {
        codecs,
        heights,
        monitors,
        current_codec,
        current_height,
        current_fps,
        current_monitor,
        portal_ready: !setting_str(&db, "engine_restore_token", "").is_empty(),
        buffer_height: current_height,
        transcoding: false,
        max_source_height,
        vendor,
        encoder,
        fps_options: vec![24, 30, 60, 120, 144],
        encoders,
        encoder_schema,
        encoder_colors,
        scale_filters: enc::SCALE_FILTERS.iter().map(|s| s.to_string()).collect(),
        fps_types: vec!["common".into(), "integer".into(), "fractional".into()],
        fps_common_values: enc::FPS_COMMON_VALUES.to_vec(),
        color_spaces: enc::COLOR_SPACES.iter().map(|s| s.to_string()).collect(),
        color_ranges: enc::COLOR_RANGES.iter().map(|s| s.to_string()).collect(),
        custom,
    })
}

#[cfg(test)]
mod custom_mode_tests {
    use super::custom_capture_params;

    #[test]
    fn ladder_mode_ignores_custom_values() {
        assert_eq!(
            custom_capture_params("ladder", "120", "50000", 60, 20000),
            (60, 20000, false)
        );
    }

    #[test]
    fn custom_accepts_medal_fps_values() {
        assert_eq!(
            custom_capture_params("custom", "24", "50000", 60, 20000),
            (24, 50000, false)
        );
        assert_eq!(
            custom_capture_params("custom", "120", "50000", 60, 20000),
            (120, 50000, false)
        );
        assert_eq!(
            custom_capture_params("custom", "144", "50000", 60, 20000),
            (144, 50000, false)
        );
        // Invalid values clamp back to the ladder with a flag.
        assert_eq!(
            custom_capture_params("custom", "90", "50000", 60, 20000),
            (60, 50000, true)
        );
        assert_eq!(
            custom_capture_params("custom", "junk", "50000", 30, 10000),
            (30, 50000, false)
        );
    }

    #[test]
    fn custom_bitrate_range_guards() {
        assert_eq!(
            custom_capture_params("custom", "60", "2000", 60, 20000),
            (60, 20000, false)
        );
        assert_eq!(
            custom_capture_params("custom", "60", "100000", 60, 20000),
            (60, 100000, false)
        );
        assert_eq!(
            custom_capture_params("custom", "60", "100001", 60, 20000),
            (60, 20000, false)
        );
        assert_eq!(
            custom_capture_params("custom", "60", "", 60, 20000),
            (60, 20000, false)
        );
    }

    #[test]
    fn probe_check_matches_request() {
        use super::check_probe;
        use crate::editor::ffmpeg::VideoProbe;
        let probe = VideoProbe {
            codec_name: "h264".into(),
            profile: "High".into(),
            width: 1920,
            height: 1080,
            fps: 60.0,
        };
        assert!(check_probe(&probe, "h264", 1080, 60).is_ok());
        // Wrong codec / height / fps all fail loudly.
        assert!(check_probe(&probe, "hevc", 1080, 60).is_err());
        assert!(check_probe(&probe, "h264", 1440, 60).is_err());
        assert!(check_probe(&probe, "h264", 1080, 30).is_err());
        // NTSC-ish drift is tolerated; real mismatch is not.
        let mut p = probe.clone();
        p.fps = 59.94;
        assert!(check_probe(&p, "h264", 1080, 60).is_ok());
    }
}

/// Open a clip with the system default player, entirely from the backend.
///
/// Rationale: frontend `openPath` goes through IPC capability checks
/// (`opener:allow-open-path`); doing it here bypasses that layer, so one
/// fewer thing can silently break. Every step is logged (backend log IS
/// visible to developers) and failures name their layer. Fallback chain:
/// opener crate -> OS launcher (`xdg-open` / `cmd /C start`).
/// NOTE: Tauri camelCases Rust params on the wire: frontend sends `clipId`,
/// never `clip_id` (see docs/01_ARCHITECTURE.md IPC rule).
#[tauri::command]
pub async fn open_clip_external(app: AppHandle, clip_id: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let db = app.state::<DbState>();
    let base = db.clips_dir()?;
    let clips = db.list_clips()?;
    let clip = clips
        .into_iter()
        .find(|c| c.id == clip_id)
        .ok_or("clip not found")?;
    let abs = base.join(&clip.file_name);
    eprintln!("[moonclip] open_clip_external: {}", abs.display());
    if !abs.exists() {
        return Err(format!("file gone from disk: {}", clip.file_name));
    }
    match app.opener().open_path(abs.to_string_lossy(), None::<&str>) {
        Ok(()) => {
            eprintln!("[moonclip] open_clip_external: opener ok");
            return Ok(());
        }
        Err(e) => {
            eprintln!("[moonclip] open_clip_external: opener failed ({e}), trying OS launcher")
        }
    }
    // OS launcher lives in os::open — no cfg here (zero-cfg rule).
    match os::open::open_external(&abs) {
        Ok(()) => {
            eprintln!("[moonclip] open_clip_external: OS launcher ok");
            Ok(())
        }
        Err(e) => Err(format!("opener + OS launcher both failed ({e})")),
    }
}

/// NOTE: Tauri camelCases Rust params on the wire: frontend sends `clipId`,
/// never `clip_id` (see docs/01_ARCHITECTURE.md IPC rule).
#[tauri::command]
pub async fn preview_track(app: AppHandle, clip_id: String, track: u32) -> Result<String, String> {
    if !(1..=3).contains(&track) {
        return Err("track must be 1 (mix), 2 (game) or 3 (mic)".into());
    }
    let db = app.state::<DbState>();
    let clips = db.list_clips()?;
    let clip = clips
        .into_iter()
        .find(|c| c.id == clip_id)
        .ok_or("clip not found")?;
    let base = db.clips_dir()?;
    let input = base.join(&clip.file_name);
    let preview = std::env::temp_dir().join("moonclip-track-preview.m4a");
    let ffmpeg = crate::editor::ffmpeg::resolve_ffmpeg(&app)?;
    let status = tokio::process::Command::new(&ffmpeg)
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            &input.to_string_lossy(),
            "-map",
            &format!("0:{track}"),
            "-c:a",
            "aac",
        ])
        .arg(&preview)
        .status()
        .await
        .map_err(|e| format!("preview extract failed: {e}"))?;
    if !status.success() {
        return Err("preview extract failed".into());
    }
    Ok(preview.to_string_lossy().to_string())
}

// ---------------------------------------------------------------------------
// Hardware test (first-run wizard, optional)
// ---------------------------------------------------------------------------

/// Result of the optional first-run hardware test.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HardwareTestResult {
    pub ok: bool,
    pub height: u32,
    pub fps: u32,
    pub codec: String,
    pub encoder: String,
    pub bitrate_kbps: u32,
    pub requested_seconds: u32,
    pub startup_ms: u64,
    pub measured_duration_ms: u64,
    pub size_bytes: u64,
    /// Suggested step-down preset when the test failed (already applied to
    /// the UI selection, never persisted without the user's click).
    pub fallback_height: Option<u32>,
    pub fallback_fps: Option<u32>,
    /// Real encoded stream of the test clip (codec/resolution/fps read back
    /// from the file: proof OBS applied the requested settings).
    pub probe: Option<crate::editor::ffmpeg::VideoProbe>,
    pub error: Option<String>,
}

/// Validate a saved test clip: exists, has real bytes and roughly matches the
/// requested duration (a replay that did not record enough seconds is a fail).
pub(crate) fn validate_test_clip(
    measured_duration_ms: i64,
    size_bytes: u64,
    requested_seconds: u32,
) -> Result<(), String> {
    if size_bytes < 64 * 1024 {
        return Err(format!("clip is suspiciously small ({size_bytes} bytes)"));
    }
    let want = requested_seconds as i64 * 1000;
    if measured_duration_ms <= 0 {
        return Err("clip duration could not be measured".into());
    }
    if measured_duration_ms < want / 2 {
        return Err(format!(
            "recorded {measured_duration_ms} ms of the requested {want} ms"
        ));
    }
    if measured_duration_ms > want * 3 / 2 + 2000 {
        return Err(format!(
            "recorded {measured_duration_ms} ms, far more than requested ({want} ms)"
        ));
    }
    Ok(())
}

/// Suggested conservative step-down for a failed test.
pub(crate) fn test_fallback(height: u32, fps: u32) -> (Option<u32>, Option<u32>) {
    if height > 720 {
        (Some(720), Some(60))
    } else if fps > 30 {
        (Some(height), Some(30))
    } else {
        (None, None)
    }
}

/// Check the real encoded stream against what was requested: codec, output
/// height and fps must match, or the settings were not really applied.
pub(crate) fn check_probe(
    probe: &crate::editor::ffmpeg::VideoProbe,
    codec: &str,
    expect_height: u32,
    expect_fps: u32,
) -> Result<(), String> {
    if probe.codec_name != codec {
        return Err(format!(
            "encoded codec is '{}', expected '{codec}'",
            probe.codec_name
        ));
    }
    if probe.height != expect_height {
        return Err(format!(
            "encoded {}p, expected {expect_height}p",
            probe.height
        ));
    }
    if (probe.fps - expect_fps as f64).abs() > 2.0 {
        return Err(format!(
            "encoded {:.1} fps, expected {expect_fps} fps",
            probe.fps
        ));
    }
    Ok(())
}

/// Expected output height for a test (Custom video > explicit override >
/// stored ladder height > monitor base).
async fn expected_test_height(app: &AppHandle, overrides: &StartOverrides) -> u32 {
    if let Some(cv) = &overrides.custom_video {
        return cv.out_height;
    }
    if let Some(h) = overrides.height {
        return h;
    }
    let db = app.state::<DbState>();
    let stored: u32 = setting_str(&db, "out_height", "0").parse().unwrap_or(0);
    if stored > 0 {
        return stored;
    }
    let monitors = video::list_monitors().await;
    video::resolve_monitor(&monitors, &setting_str(&db, "monitor", ""))
        .map(|m| m.height)
        .unwrap_or(1080)
}

/// Optional first-run test: start the buffer with candidate values (not
/// persisted), record for `seconds`, save, validate the file and restore the
/// previous buffer state. Returns the measured numbers for the wizard.
/// NOTE: Tauri camelCases Rust params on the wire: frontend sends
/// `encoderId`, `encoderSettings` and `customVideo`.
#[tauri::command]
pub async fn test_hardware(
    app: AppHandle,
    height: Option<u32>,
    fps: Option<u32>,
    seconds: Option<u32>,
    encoder_id: Option<String>,
    encoder_settings: Option<serde_json::Map<String, serde_json::Value>>,
    custom_video: Option<CustomVideo>,
) -> Result<HardwareTestResult, String> {
    let seconds = seconds.unwrap_or(10).clamp(5, 30);
    // Full Custom payloads for the "Probar" button (validated, not persisted).
    let custom_encoder = match (encoder_id, encoder_settings) {
        (Some(id), settings) => {
            let entry =
                enc::lookup_encoder(&id).ok_or_else(|| format!("unknown encoder '{id}'"))?;
            let map = settings.unwrap_or_default();
            enc::validate(entry.family, entry.codec, &map)?;
            Some(CustomEncoder {
                encoder: id,
                settings: map,
            })
        }
        (None, _) => None,
    };
    if custom_video.is_some() && custom_encoder.is_none() {
        return Err("custom video needs a custom encoder".into());
    }
    if let (Some(cv), Some(ce)) = (&custom_video, &custom_encoder) {
        let entry = enc::lookup_encoder(&ce.encoder)
            .ok_or_else(|| format!("unknown encoder '{}'", ce.encoder))?;
        let raw = serde_json::to_string(cv).map_err(|e| e.to_string())?;
        let video = enc::parse_custom_video(entry.family, entry.codec, &raw)?;
        enc::validate_pair(
            entry.family,
            entry.codec,
            &ce.settings,
            Some(&video.color_format),
        )?;
    }
    let overrides = StartOverrides {
        height,
        fps,
        duration_seconds: Some(seconds),
        custom_encoder,
        custom_video,
        ..Default::default()
    };
    let config = build_capture_config(&app, &overrides).await?;
    let was_running = {
        let st = app.state::<AppState>();
        let guard = st.recorder.lock().await;
        guard.is_some()
    };
    stop_engine(&app).await.ok();

    let t0 = std::time::Instant::now();
    let expect_height = expected_test_height(&app, &overrides).await;
    let expect_codec = config.codec.clone();
    let expect_fps = config.fps;
    let mut result = HardwareTestResult {
        ok: false,
        height: expect_height,
        fps: expect_fps,
        codec: expect_codec.clone(),
        encoder: config.encoder.clone(),
        bitrate_kbps: config.bitrate_kbps,
        requested_seconds: seconds,
        startup_ms: 0,
        measured_duration_ms: 0,
        size_bytes: 0,
        fallback_height: None,
        fallback_fps: None,
        probe: None,
        error: None,
    };

    let run = async {
        let mut engine = new_engine();
        engine.start_buffer(config.clone()).await?;
        result.startup_ms = t0.elapsed().as_millis() as u64;
        tokio::time::sleep(std::time::Duration::from_secs(seconds as u64 + 1)).await;
        let path = engine.save_clip().await?;
        engine.stop_buffer().await.ok();
        Ok::<std::path::PathBuf, String>(path)
    }
    .await;

    match run {
        Ok(path) => {
            let size = tokio::fs::metadata(&path)
                .await
                .map(|m| m.len())
                .unwrap_or(0);
            let ffmpeg = crate::editor::ffmpeg::resolve_ffmpeg(&app)?;
            let duration = crate::editor::ffmpeg::probe_duration_ms(&ffmpeg, &path)
                .await
                .unwrap_or(0);
            result.size_bytes = size;
            result.measured_duration_ms = duration.max(0) as u64;
            match validate_test_clip(duration, size, seconds) {
                Ok(()) => {
                    // Prove OBS really encoded what was requested.
                    match crate::editor::ffmpeg::probe_video_stream(&ffmpeg, &path).await {
                        Some(probe) => {
                            result.probe = Some(probe.clone());
                            match check_probe(&probe, &expect_codec, expect_height, expect_fps) {
                                Ok(()) => result.ok = true,
                                Err(e) => result.error = Some(e),
                            }
                        }
                        None => {
                            result.error =
                                Some("could not read the video stream of the test clip".into());
                        }
                    }
                }
                Err(e) => result.error = Some(e),
            }
            // The test clip is not a user clip: remove it.
            let _ = tokio::fs::remove_file(&path).await;
        }
        Err(e) => result.error = Some(e),
    }

    if !result.ok && result.error.is_some() {
        let (fh, ff) = test_fallback(result.height, result.fps);
        result.fallback_height = fh;
        result.fallback_fps = ff;
    }

    // Restore the previous state (persisted settings are untouched).
    if was_running {
        if let Err(e) = start_engine(&app, &StartOverrides::default()).await {
            eprintln!("[moonclip] hardware test: could not restore buffer: {e}");
        }
    }
    eprintln!(
        "[moonclip] hardware test: ok={} {}x{}@{} {:?} startup={}ms duration={}ms size={}B",
        result.ok,
        result.height,
        result.fps,
        result.codec,
        result.error,
        result.startup_ms,
        result.measured_duration_ms,
        result.size_bytes
    );
    Ok(result)
}
