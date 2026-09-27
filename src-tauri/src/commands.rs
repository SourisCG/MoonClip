//! Tauri IPC handlers (Phase 2: persistence; Phase 3: capture).
//! V3 capture engine: embedded, isolated OBS Studio driven over obs-websocket.

use std::collections::HashMap;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::os::shared::encoder_options as enc;
use crate::os::{
    self, backend_name, devices, new_engine, resolve_obs, shared::engine as obs, video, AudioDevice,
    CaptureConfig, CaptureEngine, CaptureInput, CustomEncoder, CustomVideo,
};
use crate::state::AppState;
use crate::storage::models::{ClipRecord, RegisteredInput};
use crate::storage::{secrets, DbState};

/// Persist OBS source settings with portal tokens stripped into the OS vault
/// (plaintext-DB fallback when the vault is unavailable, so capture never
/// breaks on minimal WMs).
fn persist_input_settings(
    db: &DbState,
    input_id: &str,
    input_name: &str,
    settings: &serde_json::Value,
) -> Result<(), String> {
    let mut sanitized = settings.clone();
    let secret = secrets::take_secrets(&mut sanitized);
    if !secret.is_empty() {
        match secrets::store_input_secrets(input_id, &secret) {
            Ok(()) => {}
            Err(e) => {
                eprintln!(
                    "[moonclip] secret vault unavailable ({e}); keeping the portal token in the local DB"
                );
                secrets::merge_secrets(&mut sanitized, secret);
            }
        }
    }
    db.set_input_settings(input_name, &sanitized.to_string())
}

/// Stored settings for an input with vault secrets merged back in.
fn input_settings_with_secrets(row: &RegisteredInput, fallback: serde_json::Value) -> serde_json::Value {
    let mut value = row
        .input_settings
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .unwrap_or(fallback);
    match secrets::load_input_secrets(&row.id) {
        Ok(Some(secret)) => secrets::merge_secrets(&mut value, secret),
        Ok(None) => {}
        Err(e) => eprintln!(
            "[moonclip] cannot read the secret vault ({e}); the portal picker may appear again"
        ),
    }
    value
}

/// Did the user already grant a portal (screen) restore token?
fn portal_token_present(db: &DbState) -> bool {
    let Ok(Some(screen)) = db.screen_input() else {
        return false;
    };
    let stored = screen.input_settings.as_deref().unwrap_or("");
    stored.contains("RestoreToken")
        || matches!(secrets::load_input_secrets(&screen.id), Ok(Some(_)))
}

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

/// Index MoonClip-style files that exist on disk but have no DB row (runs at
/// boot and on demand). NEVER deletes files: orphan rows keep the explicit
/// `purge_missing_clips` path.
#[tauri::command]
pub async fn reconcile_library(
    app: AppHandle,
) -> Result<crate::storage::reconcile::ReconcileReport, String> {
    let ffmpeg = crate::editor::ffmpeg::resolve_ffmpeg(&app)?;
    let db = app.state::<DbState>();
    crate::storage::reconcile::reconcile_dir(&db, &ffmpeg).await
}

/// Move legacy flat clips into their game folder (idempotent; also runs at
/// boot). Never deletes anything.
#[tauri::command]
pub fn organize_library(
    db: State<'_, DbState>,
) -> Result<crate::storage::reconcile::OrganizeReport, String> {
    crate::storage::reconcile::organize_dir(&db)
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
    /// Name of the registered input to record (None = resolve from the
    /// running registered apps). `setup` keeps OBS visible for window picking.
    pub record_input: Option<String>,
    pub setup: bool,
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

    // Private obs-websocket: dedicated port + a password generated per start.
    // It is written into this run's generated OBS config and never persisted
    // (one less secret at rest; the DB scrub removes legacy rows).
    let port: u16 = setting_str(&db, "engine_ws_port", "4456")
        .parse()
        .unwrap_or(4456)
        .clamp(1024, 65535);
    let password =
        uuid::Uuid::new_v4().simple().to_string() + &uuid::Uuid::new_v4().simple().to_string();

    let (obs_bin, _) = resolve_obs(app)?;

    // Registered capture inputs: one OBS source per game plus the internal
    // screen source. The active one is visible, the rest stay hidden; audio
    // is global and never changes.
    let mut regs = db.list_registered_inputs()?;
    regs.push(ensure_screen_input(app.state::<DbState>())?);
    let monitor = selected.map(|m| m.name.clone()).unwrap_or_default();
    let inputs: Vec<CaptureInput> = regs
        .iter()
        .map(|r| {
            let fallback = if r.input_kind == "screen" {
                os::screen_input_settings(&monitor)
            } else {
                serde_json::json!({})
            };
            let settings = input_settings_with_secrets(r, fallback);
            CaptureInput {
                name: r.input_name.clone(),
                kind: r.input_kind.clone(),
                source_id: os::input_source_id(&r.input_kind).to_string(),
                settings,
                uuid: r.source_uuid.clone(),
            }
        })
        .collect();
    let active_input = overrides.record_input.clone().unwrap_or_default();

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
        inputs,
        active_input,
        setup: overrides.setup,
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
        "[moonclip] obs engine: {}x{}@{} {} {}kbps replay={}s codec={} input={} ({}/{} inputs) audio={}",
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
        if config.active_input.is_empty() {
            "-".to_string()
        } else {
            config.active_input.clone()
        },
        config.inputs.iter().filter(|i| i.kind == "window").count(),
        config.inputs.len(),
        if config.audio_single_track {
            "Mix"
        } else {
            "Mix+Game+Mic"
        }
    );
    let active_input = config.active_input.clone();
    let mut engine = new_engine();
    let first_time = config
        .inputs
        .iter()
        .find(|i| i.name == config.active_input)
        .map(|i| i.kind == "window" && i.settings.get("RestoreToken").is_none() && i.settings.get("window").is_none())
        .unwrap_or(false);
    if let Err(e) = engine.start_buffer(config).await {
        eprintln!("[moonclip] obs start failed: {e}");
        return Err(e);
    }
    // Setup runs stop here: the engine is up with the picker open and no
    // replay buffer; the registration flow polls the source settings.
    if overrides.setup {
        let tracks = engine.tracks_linked();
        let st = app.state::<AppState>();
        *st.recorder.lock().await = Some(engine);
        {
            let mut g = st.game.lock().await;
            g.active_input = Some(active_input.clone());
            g.retry_at = None;
            g.retry_input = None;
            g.suppressed_input = None;
        }
        set_engine_error(app, None).await;
        set_audio_error(app, None).await;
        return Ok(EngineStatus {
            running: true,
            backend: backend_name().to_string(),
            tracks_linked: tracks,
            audio_error: None,
            engine_error: None,
        });
    }
    // Portal token + real canvas size: OBS refreshes the restore token on a
    // successful Start and the portal (not our profile) decides the captured
    // size. The settings are persisted the moment the source reports them, so
    // a fresh token survives even if the later size probe fails (and is never
    // lost when the engine is force-stopped).
    let db = app.state::<DbState>();
    let active_row = if active_input.is_empty() {
        None
    } else {
        db.input_by_name(&active_input).ok().flatten()
    };
    let mut stored_settings = active_row.as_ref().and_then(|r| r.input_settings.clone());
    let source_wait = if first_time {
        std::time::Duration::from_secs(60)
    } else {
        std::time::Duration::from_secs(15)
    };
    let mut waited = std::time::Duration::ZERO;
    let mut learned: Option<(u32, u32)> = None;
    while waited < source_wait {
        if !active_input.is_empty() {
            if let Some(settings) = engine.read_input_settings(&active_input).await {
                // Compare/store the sanitized form so the vault token does not
                // count as a change on every poll.
                let mut sanitized = settings.clone();
                secrets::take_secrets(&mut sanitized);
                let text = sanitized.to_string();
                if stored_settings.as_deref() != Some(text.as_str()) {
                    if let Some(row) = &active_row {
                        let _ = persist_input_settings(&db, &row.id, &active_input, &settings);
                    }
                    engine.note("capture settings updated");
                    stored_settings = Some(text);
                }
            }
        }
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
    {
        let db = app.state::<DbState>();
        let _ = db.set_setting("engine_source_width", &src_w.to_string());
        let _ = db.set_setting("engine_source_height", &src_h.to_string());
    }
    let tracks = engine.tracks_linked();
    {
        let st = app.state::<AppState>();
        *st.recorder.lock().await = Some(engine);
        {
            let mut g = st.game.lock().await;
            g.active_input = Some(active_input.clone());
            g.retry_at = None;
            g.retry_input = None;
            g.suppressed_input = None;
        }
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
    st.game.lock().await.active_input = None;
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
    let active = {
        let st = app.state::<AppState>();
        let g = st.game.lock().await;
        g.active_input.clone()
    };
    stop_engine(app).await?;
    start_engine(
        app,
        &StartOverrides {
            record_input: active,
            ..Default::default()
        },
    )
    .await?;
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

/// Validate a stored media file name (a name, never a path) and resolve it
/// inside `base`.
pub(crate) fn validated_media_path(
    base: &std::path::Path,
    name: &str,
) -> Result<std::path::PathBuf, String> {
    if !crate::storage::paths::is_safe_relative_media_name(name) {
        return Err("invalid media name".into());
    }
    let path = base.join(name);
    if !path.is_file() {
        return Err(format!("media not found: {name}"));
    }
    Ok(path)
}

/// Raw thumbnail bytes for the gallery. The webview cannot load `asset://`
/// reliably, so images travel over IPC as bytes.
#[tauri::command]
pub fn read_thumbnail(
    db: State<'_, DbState>,
    thumbnail_name: String,
) -> Result<tauri::ipc::Response, String> {
    let base = db.clips_dir()?;
    let path = validated_media_path(&base, &thumbnail_name)?;
    let bytes =
        std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Ok(tauri::ipc::Response::new(bytes))
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
    manual_start(&app).await
}

/// Start button / hotkey: record the running registered game; if none is
/// open, the last recorded one (or the only one). Never records the screen.
async fn manual_start(app: &AppHandle) -> Result<EngineStatus, String> {
    let db = app.state::<DbState>();
    let inputs = db.list_registered_inputs().unwrap_or_default();
    if inputs.is_empty() {
        return Err("no registered game: press REGISTER GAME in Games".into());
    }
    let windows = tokio::task::spawn_blocking(os::list_windows)
        .await
        .unwrap_or_default();
    let last = setting_str(&db, "last_game_input", "");
    let pick = os::shared::winlist::pick_manual(
        &inputs,
        &windows,
        (!last.is_empty()).then_some(last.as_str()),
    )
    .ok_or_else(|| "no registered game is open: open the game or register it again".to_string())?
    .clone();
    let overrides = StartOverrides {
        record_input: Some(pick.input_name.clone()),
        ..Default::default()
    };
    let res = start_engine(app, &overrides).await?;
    let _ = db.set_setting("last_game_input", &pick.input_name);
    let st = app.state::<AppState>();
    st.game.lock().await.auto_started = false;
    Ok(res)
}

/// Poller tick: the checker lists desktop windows and matches them against
/// the registered games by title. Starts the buffer when a game window
/// appears, stops it (auto sessions only) when the window is gone.
pub(crate) async fn poll_games(app: &AppHandle) {
    let db = app.state::<DbState>();
    let inputs = db.list_registered_inputs().unwrap_or_default();
    let tracked = inputs
        .iter()
        .any(|r| r.window_title.as_deref().is_some_and(|t| !t.trim().is_empty()));
    let windows = if tracked {
        tokio::task::spawn_blocking(os::list_windows)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let matched = os::shared::winlist::first_match_registered(&inputs, &windows).cloned();
    let st = app.state::<AppState>();
    let running = st.recorder.lock().await.is_some();

    // Manual-stop suppression: kept only while the SAME window is still open;
    // when it disappears (or another game matches) the next appearance may
    // auto-start again.
    {
        let mut g = st.game.lock().await;
        match &matched {
            Some(row) if g.suppressed_input.as_deref() == Some(row.input_name.as_str()) => {}
            _ => g.suppressed_input = None,
        }
    }

    // Debug aid: when games are registered but no window matches, show what
    // the desktop reports (once per change) so mismatched titles are obvious.
    let seen = if matched.is_none() && tracked {
        windows
            .iter()
            .take(12)
            .map(|w| w.title.as_str())
            .collect::<Vec<_>>()
            .join(" | ")
    } else {
        String::new()
    };
    let mut log_seen = false;

    let changed = {
        let mut g = st.game.lock().await;
        let before = g.current.as_ref().map(|a| a.id.clone());
        let after = matched.as_ref().map(|a| a.id.clone());
        g.current = matched.clone();
        if g.last_seen_windows != seen {
            g.last_seen_windows = seen.clone();
            log_seen = !seen.is_empty();
        }
        if matched.is_none() && g.auto_started {
            g.missing_ticks += 1;
        } else {
            g.missing_ticks = 0;
        }
        before != after
    };
    if log_seen {
        eprintln!("[moonclip] no registered window match; desktop windows: {seen}");
    }
    if changed {
        let name = matched.as_ref().map(|a| a.display_name.clone());
        match &name {
            Some(n) => eprintln!("[moonclip] game: {n}"),
            None => eprintln!("[moonclip] game: none"),
        }
        let _ = app.emit("moonclip://game-changed", &name);
    }

    match (&matched, running) {
        (Some(row), false) => {
            let blocked = {
                let g = st.game.lock().await;
                g.suppressed_input.as_deref() == Some(row.input_name.as_str())
                    || (g.retry_input.as_deref() == Some(row.input_name.as_str())
                        && g.retry_at
                            .is_some_and(|t| std::time::Instant::now() < t))
            };
            if blocked {
                return;
            }
            let overrides = StartOverrides {
                record_input: Some(row.input_name.clone()),
                ..Default::default()
            };
            match start_engine(app, &overrides).await {
                Ok(_) => {
                    let _ = db.set_setting("last_game_input", &row.input_name);
                    let mut g = st.game.lock().await;
                    g.auto_started = true;
                    g.retry_at = None;
                    g.retry_input = None;
                }
                Err(e) => {
                    eprintln!("[moonclip] auto start failed: {e}");
                    let mut g = st.game.lock().await;
                    // Wait a full minute before hammering the engine/picker
                    // again with the same input (cancelled picker, bad token).
                    g.retry_at =
                        Some(std::time::Instant::now() + std::time::Duration::from_secs(60));
                    g.retry_input = Some(row.input_name.clone());
                }
            }
        }
        (None, true) => {
            let stop = {
                let g = st.game.lock().await;
                g.auto_started && g.missing_ticks >= 2
            };
            if stop {
                if let Err(e) = stop_engine(app).await {
                    eprintln!("[moonclip] auto stop failed: {e}");
                }
                st.game.lock().await.auto_started = false;
            }
        }
        _ => {}
    }
}

/// The internal full-screen input, created on first use (hidden from Games).
fn ensure_screen_input(db: State<'_, DbState>) -> Result<RegisteredInput, String> {
    if let Some(row) = db.screen_input()? {
        return Ok(row);
    }
    db.register_input("MoonClip Screen", "screen", "Full screen")
}

/// Explicit "Record screen" button: full-screen capture with the OBS monitor
/// source. Its own saved input; the picker appears the first time only.
#[tauri::command]
pub async fn start_screen_buffer(app: AppHandle) -> Result<EngineStatus, String> {
    {
        let st = app.state::<AppState>();
        if st.recorder.lock().await.is_some() {
            return Err("buffer already running".into());
        }
    }
    let screen = ensure_screen_input(app.state::<DbState>())?;
    let res = start_engine(
        &app,
        &StartOverrides {
            record_input: Some(screen.input_name.clone()),
            ..Default::default()
        },
    )
    .await?;
    let st = app.state::<AppState>();
    st.game.lock().await.auto_started = false;
    Ok(res)
}

/// Registered window title matched by the checker (null when none).
#[tauri::command]
pub async fn current_game(app: AppHandle) -> Option<String> {
    let st = app.state::<AppState>();
    let g = st.game.lock().await;
    g.current.as_ref().map(|a| a.display_name.clone())
}

#[tauri::command]
pub fn list_registered_inputs(db: State<'_, DbState>) -> Result<Vec<RegisteredInput>, String> {
    db.list_registered_inputs()
}

#[tauri::command]
pub async fn delete_registered_input(app: AppHandle, id: String) -> Result<(), String> {
    let removed = {
        let db = app.state::<DbState>();
        let row = db
            .list_registered_inputs()?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| "registered input not found".to_string())?;
        db.delete_registered_input(&id)?;
        row
    };
    // If that game is being recorded (or is next in line), stop and forget it
    // so a deleted game never keeps the buffer alive.
    {
        let st = app.state::<AppState>();
        let mut g = st.game.lock().await;
        if g.suppressed_input.as_deref() == Some(removed.input_name.as_str()) {
            g.suppressed_input = None;
        }
        if g.current.as_ref().map(|c| c.id.clone()) == Some(removed.id.clone()) {
            g.current = None;
        }
        let active = g.active_input.clone();
        drop(g);
        if active.as_deref() == Some(removed.input_name.as_str()) {
            stop_engine(&app).await?;
        }
    }
    Ok(())
}

/// Did the picker write a real target into the source settings yet?
fn pick_done(settings: &serde_json::Value, initial: &serde_json::Value) -> bool {
    ["RestoreToken", "window"].iter().any(|k| {
        let now = settings.get(*k).and_then(|v| v.as_str()).unwrap_or("");
        let before = initial.get(*k).and_then(|v| v.as_str()).unwrap_or("");
        !now.trim().is_empty() && now != before
    })
}

/// Shared REGISTER/EDIT flow: run the engine in setup mode so the picker
/// appears (KDE dialog on Linux, OBS window-capture dialog on Windows), wait
/// for the pick, then store the OBS settings + the window identity.
async fn pick_window(app: &AppHandle, input_name: &str) -> Result<(), String> {
    if app.state::<AppState>().recorder.lock().await.is_some() {
        stop_engine(app).await?;
    }
    start_engine(
        app,
        &StartOverrides {
            record_input: Some(input_name.to_string()),
            setup: true,
            ..Default::default()
        },
    )
    .await?;
    let initial = {
        let st = app.state::<AppState>();
        let guard = st.recorder.lock().await;
        match guard.as_ref() {
            Some(engine) => engine
                .read_input_settings(input_name)
                .await
                .unwrap_or_else(|| serde_json::json!({})),
            None => serde_json::json!({}),
        }
    };

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut picked: Option<serde_json::Value> = None;
    while std::time::Instant::now() < deadline {
        {
            let st = app.state::<AppState>();
            let guard = st.recorder.lock().await;
            let Some(engine) = guard.as_ref() else {
                break; // engine died or was stopped
            };
            if let Some(settings) = engine.read_input_settings(input_name).await {
                if pick_done(&settings, &initial) {
                    picked = Some(settings);
                    break;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let Some(settings) = picked else {
        stop_engine(app).await.ok();
        return Err("no window picked (dialog cancelled or timed out)".into());
    };

    let identity = os::window_identity(&settings);
    {
        let db = app.state::<DbState>();
        let Some(row) = db.input_by_name(input_name)? else {
            return Err("registered input vanished while picking".into());
        };
        persist_input_settings(&db, &row.id, input_name, &settings)?;
        match identity {
            Some(id) if !id.title.trim().is_empty() => {
                db.set_input_identity(
                    input_name,
                    id.title.trim(),
                    Some(id.title.trim()),
                    (!id.app_id.is_empty()).then_some(id.app_id.as_str()),
                    &id.exe,
                )?;
                // Every registered app gets its library folder; an existing
                // one with the same name is reused (never recreated).
                if row.clips_folder.trim().is_empty() {
                    if let Ok(base) = db.clips_dir() {
                        if let Ok(folder) = crate::storage::folders::ensure_game_folder(
                            &base,
                            id.title.trim(),
                        ) {
                            let _ = db.set_input_folder(input_name, &folder);
                        }
                    }
                }
            }
            _ => {
                // Identity unavailable: keep the placeholder name and let the
                // user re-pick via Edit; auto-start simply never matches.
                eprintln!("[moonclip] warning: could not read the picked window identity");
            }
        }
    }
    stop_engine(app).await.ok();
    Ok(())
}

/// Register a game: one button, opens the system picker, stores the window.
#[tauri::command]
pub async fn register_game(app: AppHandle) -> Result<RegisteredInput, String> {
    let (row, input_name) = {
        let db = app.state::<DbState>();
        let rows = db.list_registered_inputs()?;
        let mut n = rows.len() + 1;
        let mut input_name = format!("Game {n}");
        while db.input_by_name(&input_name)?.is_some() {
            n += 1;
            input_name = format!("Game {n}");
        }
        let row = db.register_input(&input_name, "window", &input_name)?;
        (row, input_name)
    };
    if let Err(e) = pick_window(&app, &input_name).await {
        let db = app.state::<DbState>();
        let _ = db.delete_registered_input(&row.id);
        return Err(e);
    }
    let db = app.state::<DbState>();
    db.input_by_name(&input_name)?
        .ok_or_else(|| "registered game vanished".to_string())
}

/// Edit a game: re-open the picker for that input (choose another window).
#[tauri::command]
pub async fn edit_game(app: AppHandle, id: String) -> Result<RegisteredInput, String> {
    let row = {
        let db = app.state::<DbState>();
        let row = db
            .list_registered_inputs()?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| "registered game not found".to_string())?;
        // Drop the stored target (and its vault token) so the picker appears.
        db.set_input_settings(&row.input_name, "{}")?;
        let _ = secrets::delete_input_secrets(&row.id);
        row
    };
    pick_window(&app, &row.input_name).await?;
    let db = app.state::<DbState>();
    db.input_by_name(&row.input_name)?
        .ok_or_else(|| "registered game vanished".to_string())
}

#[tauri::command]
pub async fn stop_buffer(app: AppHandle) -> Result<EngineStatus, String> {
    // A hand stop wins over the autopilot: do not restart this game until its
    // window goes away and comes back (or the user starts it again).
    let active = {
        let st = app.state::<AppState>();
        let g = st.game.lock().await;
        g.active_input.clone()
    };
    if let Some(name) = active {
        let st = app.state::<AppState>();
        st.game.lock().await.suppressed_input = Some(name);
    }
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
        db.set_setting("engine_source_width", "")?;
        db.set_setting("engine_source_height", "")?;
        if let Ok(Some(screen)) = db.screen_input() {
            let _ = secrets::delete_input_secrets(&screen.id);
        }
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
    let base = db.clips_dir()?;
    // Which game folder does this clip belong to? `clips_folder` is assigned
    // at registration; legacy rows derive it once and persist it. No game
    // detected -> the shared Unknown folder (stable grouping, never deleted).
    let (game_title, folder) = {
        let st = app.state::<AppState>();
        let g = st.game.lock().await;
        match g.current.as_ref() {
            Some(a) if !a.display_name.trim().is_empty() => {
                let folder = if a.clips_folder.trim().is_empty() {
                    let f = crate::storage::folders::ensure_game_folder(&base, &a.display_name)?;
                    let _ = db.set_input_folder(&a.input_name, &f);
                    f
                } else {
                    a.clips_folder.clone()
                };
                (a.display_name.clone(), folder)
            }
            _ => (
                "Unknown".to_string(),
                crate::storage::folders::ensure_game_folder(&base, "Unknown")?,
            ),
        }
    };
    // OBS writes the replay at the library root: move it into its game folder.
    let mut path = path;
    let folder_dir = base.join(&folder);
    tokio::fs::create_dir_all(&folder_dir)
        .await
        .map_err(|e| format!("cannot create game folder {folder}: {e}"))?;
    if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
        let dest = folder_dir.join(name);
        if path != dest {
            if let Err(e) = tokio::fs::rename(&path, &dest).await {
                tokio::fs::copy(&path, &dest)
                    .await
                    .map_err(|e2| format!("cannot move clip into {folder}: {e} / {e2}"))?;
                let _ = tokio::fs::remove_file(&path).await;
            }
            path = dest;
        }
    }
    // Same-second double saves collide: OBS names replay files by timestamp,
    // so the second file could overwrite the first on disk and the DB would
    // reject the duplicate. Rename to stem_2.mp4, stem_3.mp4… within the game
    // folder instead of failing and losing the clip.
    let file_name = {
        let taken: std::collections::HashSet<String> = db
            .list_clips()
            .map(|clips| clips.into_iter().map(|c| c.file_name).collect())
            .unwrap_or_default();
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("bad clip file name")?
            .to_string();
        if taken.contains(&format!("{folder}/{name}")) {
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
                if !taken.contains(&format!("{folder}/{cand_name}")) && !cand.exists() {
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
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("bad clip file name")?;
        format!("{folder}/{name}")
    };
    let ffmpeg = crate::editor::ffmpeg::resolve_ffmpeg(app)?;
    let size = tokio::fs::metadata(&path)
        .await
        .map_err(|e| format!("cannot stat clip: {e}"))?
        .len() as i64;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("bad clip file name")?;
    let thumb_name = format!("{folder}/thumb_{stem}.jpg");
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
    let clip = db.insert_clip(&file_name, &thumb_name, &game_title, secs_ms, size, &folder)?;
    if let Ok(pruned) = db.enforce_quota(Some(&clip.id), None) {
        if pruned > 0 {
            eprintln!("[moonclip] quota: pruned {pruned} old clips");
        }
    }
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
        match manual_start(&app).await {
            Ok(_) => notify(
                &app,
                "Búfer iniciado — vuelve a pulsar para guardar",
                "Buffer started — press again to save",
            ),
            Err(e) => notify(
                &app,
                &format!("No se pudo iniciar: {e}"),
                &format!("Could not start: {e}"),
            ),
        }
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
        portal_ready: portal_token_present(&db),
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
mod media_path_tests {
    use super::validated_media_path;

    #[test]
    fn media_path_is_a_name_only() {
        let tmp = std::env::temp_dir().join(format!("moonclip-media-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("thumb.jpg"), b"x").unwrap();

        assert!(validated_media_path(&tmp, "thumb.jpg").is_ok());
        for bad in ["../thumb.jpg", "/etc/passwd", "sub/thumb.jpg", "..", "", "a\\b.jpg"] {
            assert!(validated_media_path(&tmp, bad).is_err(), "{bad}");
        }
        assert!(validated_media_path(&tmp, "missing.jpg").is_err());

        let _ = std::fs::remove_dir_all(&tmp);
    }
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
// Clip editing (Phase 5)
// ---------------------------------------------------------------------------

/// Playable URL for a clip. `asset://` cannot play media on WebKitGTK, so the
/// backend serves the file over a loopback HTTP server with range support
/// (starts on first use; nothing runs before the first editor/trim open).
#[tauri::command]
pub fn media_url(db: State<'_, DbState>, clip_id: String) -> Result<String, String> {
    let clip = db
        .list_clips()?
        .into_iter()
        .find(|c| c.id == clip_id)
        .ok_or_else(|| "clip not found".to_string())?;
    let base = db.clips_dir()?;
    let path = validated_media_path(&base, &clip.file_name)?;
    crate::editor::media_server::media_url(&path)
}

/// Gallery quick trim: lossless stream copy (fast, keyframe-aligned) or a
/// short precise re-encode (frame-exact, keeps every audio track). Always
/// creates a NEW clip; the source file is never modified.
#[tauri::command]
pub async fn trim_clip(
    app: AppHandle,
    clip_id: String,
    start_ms: i64,
    end_ms: i64,
    precise: bool,
) -> Result<ClipRecord, String> {
    let db = app.state::<DbState>();
    let clip = db
        .list_clips()?
        .into_iter()
        .find(|c| c.id == clip_id)
        .ok_or_else(|| "clip not found".to_string())?;
    let base = db.clips_dir()?;
    let input = base.join(&clip.file_name);
    if !input.is_file() {
        return Err(format!("clip file missing: {}", clip.file_name));
    }
    // Clamp to the measured duration; a tiny selection is a UI bug.
    let total = clip.duration_ms.max(0);
    let start = start_ms.clamp(0, (total - 100).max(0));
    let end = end_ms.clamp(start + 100, total.max(start + 100));
    if end - start < 100 {
        return Err("selection too short to trim".into());
    }
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("bad clip file name")?
        .to_string();
    let ext = input
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("mp4")
        .to_ascii_lowercase();
    // Derived clips stay in the source clip's game folder.
    let folder = clip.folder.trim().to_string();
    let out_dir = if folder.is_empty() {
        base.clone()
    } else {
        let dir = base.join(&folder);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| format!("cannot create game folder {folder}: {e}"))?;
        dir
    };
    let taken: std::collections::HashSet<String> = db
        .list_clips()?
        .into_iter()
        .filter(|c| c.folder == folder)
        .map(|c| {
            c.file_name
                .rsplit_once('/')
                .map(|(_, n)| n.to_string())
                .unwrap_or(c.file_name)
        })
        .collect();
    let output = crate::editor::trim::unique_trim_path(&out_dir, &stem, &ext, &taken);
    let ffmpeg = crate::editor::ffmpeg::resolve_ffmpeg(&app)?;
    let mode = if precise {
        crate::editor::trim::TrimMode::Precise
    } else {
        crate::editor::trim::TrimMode::Lossless
    };
    crate::editor::trim::run_trim(
        &app,
        &ffmpeg,
        &crate::editor::trim::TrimSpec {
            clip_id: &clip_id,
            input: &input,
            output: &output,
            start_ms: start,
            end_ms: end,
            mode,
        },
    )
    .await?;
    if !output.is_file() {
        return Err("trim produced no output file".into());
    }
    let out_bare = output
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("bad output file name")?
        .to_string();
    let out_name = if folder.is_empty() {
        out_bare
    } else {
        format!("{folder}/{out_bare}")
    };
    let out_stem = output
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("bad output file name")?
        .to_string();
    let thumb_name = if folder.is_empty() {
        format!("thumb_{out_stem}.jpg")
    } else {
        format!("{folder}/thumb_{out_stem}.jpg")
    };
    let thumb_path = out_dir.join(format!("thumb_{out_stem}.jpg"));
    if let Err(e) =
        crate::editor::ffmpeg::make_thumbnail(&ffmpeg, &output, &thumb_path, 0.3).await
    {
        let _ = tokio::fs::remove_file(&output).await;
        return Err(e);
    }
    let size = tokio::fs::metadata(&output)
        .await
        .map_err(|e| format!("cannot stat trimmed clip: {e}"))?
        .len() as i64;
    let duration_ms = crate::editor::ffmpeg::probe_duration_ms(&ffmpeg, &output)
        .await
        .unwrap_or(end - start);
    let record = db.insert_clip(
        &out_name,
        &thumb_name,
        &clip.game_title,
        duration_ms,
        size,
        &folder,
    )?;
    let _ = db.enforce_quota(Some(&record.id), None);
    eprintln!(
        "[moonclip] trim {} -> {} ({}ms..{}ms, {})",
        clip.file_name,
        out_name,
        start,
        end,
        if precise { "precise" } else { "lossless" }
    );
    crate::cue::play_ding();
    let _ = app.emit("moonclip://clip-saved", &record);
    Ok(record)
}

// ---------------------------------------------------------------------------
// Advanced editor (Phase E2): sessions, projects and staged export
// ---------------------------------------------------------------------------

/// Open (or resume) the heavy editor for a clip: stems, preview proxy when
/// needed, saved project and available encoders. Creates the session dir.
#[tauri::command]
pub async fn editor_open(
    app: AppHandle,
    clip_id: String,
) -> Result<crate::editor::session::EditorOpenResult, String> {
    crate::editor::session::open(&app, &clip_id).await
}

/// Close the session: kills exports, releases media tokens, deletes temps.
#[tauri::command]
pub async fn editor_close(app: AppHandle, session_id: String) -> Result<(), String> {
    let _ = &app;
    crate::editor::session::close(&session_id).await
}

/// Autosaved project for a clip (None = never edited).
#[tauri::command]
pub fn editor_load_project(
    app: AppHandle,
    clip_id: String,
) -> Option<crate::editor::project::EditProject> {
    crate::editor::session::load_project(&app, &clip_id)
}

/// Persist the project (autosave with debounce on the frontend).
#[tauri::command]
pub fn editor_save_project(
    app: AppHandle,
    project: crate::editor::project::EditProject,
) -> Result<(), String> {
    crate::editor::session::save_project(&app, &project)
}

/// Start an export in the background; progress arrives as edit-progress
/// events and completion as `moonclip://editor-export-done`.
#[tauri::command]
pub async fn editor_export(
    app: AppHandle,
    session_id: String,
    project: crate::editor::project::EditProject,
) -> Result<(), String> {
    {
        let handle = crate::editor::session::export_handle(&session_id)
            .ok_or_else(|| "editor session not found".to_string())?;
        if handle.lock().await.is_some() {
            return Err("an export is already running".into());
        }
        let _ = crate::editor::session::save_project(&app, &project);
    }
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = crate::editor::export::run(&handle, &session_id, &project).await;
        if let Err(e) = &result {
            eprintln!("[moonclip] editor export failed: {e}");
        }
        let _ = handle.emit(
            "moonclip://editor-export-done",
            serde_json::json!({ "ok": result.is_ok(), "error": result.err() }),
        );
    });
    Ok(())
}

/// Editor playback health: clear any persisted system mute on our own stream
/// (KDE remembers per-app mute and would silence every new editor session).
#[tauri::command]
pub async fn editor_audio_health(app: AppHandle) -> Result<usize, String> {
    // The stream only exists once the AudioContext is running, so retry until
    // it appears and verifies as unmuted.
    let mut fixed_total = 0usize;
    for _ in 0..8 {
        fixed_total += os::unmute_editor_streams()?;
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        let still = os::editor_streams_muted();
        if still == 0 && fixed_total > 0 {
            return Ok(fixed_total);
        }
    }
    let _ = &app;
    Ok(fixed_total)
}

/// Editor diagnostics bridge: webview messages land in the app log (the dev
/// log is the only place we can see what the media engine actually did).
#[tauri::command]
pub fn editor_log(message: String) {
    eprintln!("[moonclip] editor: {message}");
}

/// Make another gallery clip usable by the editor session: returns its
/// preview URL (H.264 proxy generated on demand) and its audio stems.
#[tauri::command]
pub async fn editor_add_source(
    app: AppHandle,
    session_id: String,
    clip_id: String,
) -> Result<crate::editor::session::EditorSourceInfo, String> {
    crate::editor::session::ensure_source(&app, &session_id, &clip_id).await
}

/// Cancel the running export (if any).
#[tauri::command]
pub async fn editor_cancel_export(
    app: AppHandle,
    session_id: String,
) -> Result<(), String> {
    let _ = &app;
    let Some(handle) = crate::editor::session::export_handle(&session_id) else {
        return Ok(());
    };
    if let Some(mut child) = handle.lock().await.take() {
        let _ = child.kill().await;
        let _ = child.wait().await;
        eprintln!("[moonclip] editor export cancelled");
    }
    Ok(())
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
