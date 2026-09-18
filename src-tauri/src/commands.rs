//! Tauri IPC handlers (Phase 2: persistence; Phase 3: capture).
//! V3 capture engine: embedded, isolated OBS Studio driven through obs-cmd.

use std::collections::HashMap;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::os::{
    self, backend_name, devices, new_engine, obs, resolve_obs, resolve_obscmd, video, AudioDevice,
    CaptureConfig, CaptureEngine,
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
            base_height
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
    if let Some(f) = overrides.fps {
        fps = f;
    }
    if let Some(b) = overrides.bitrate_kbps {
        bitrate = b;
    }

    // Private obs-websocket: dedicated port + generated password (persisted).
    let port: u16 = setting_str(&db, "obs_ws_port", "4456")
        .parse()
        .unwrap_or(4456)
        .clamp(1024, 65535);
    let mut password = setting_str(&db, "obs_ws_password", "");
    if password.len() < 16 {
        password =
            uuid::Uuid::new_v4().simple().to_string() + &uuid::Uuid::new_v4().simple().to_string();
        let _ = db.set_setting("obs_ws_password", &password);
    }

    let (obs_bin, _) = resolve_obs(app)?;
    let (obscmd_bin, _) = resolve_obscmd(app)?;

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
        window: setting_str(&db, "capture_window", ""),
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
        obs_bin: Some(obs_bin),
        obscmd_bin: Some(obscmd_bin),
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
    if let Err(e) = engine.start_buffer(config).await {
        eprintln!("[moonclip] obs start failed: {e}");
        return Err(e);
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
    start_engine(&app, &StartOverrides::default()).await
}

#[tauri::command]
pub async fn stop_buffer(app: AppHandle) -> Result<EngineStatus, String> {
    stop_engine(&app).await
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
                (false, engine.log_tail())
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

/// Gain lives in the generated OBS scene, so a running buffer restarts once
/// (with a visible notice) to apply it — same semantics as other settings.
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
    /// OBS binary resolved (bundled or env override).
    pub present: bool,
    /// `obs --version` output (empty when it cannot be probed).
    pub version: String,
    /// obs-cmd binary resolved.
    pub obscmd_present: bool,
    /// MoonClip-owned config dir (never the user's OBS config).
    pub config_dir: String,
    pub profile: String,
    pub collection: String,
    pub websocket_port: u16,
    /// bundled | env | missing
    pub source: String,
    /// Last lines of the newest OBS log inside our config dir.
    pub log_tail: Vec<String>,
}

#[tauri::command]
pub async fn obs_info(app: AppHandle) -> Result<ObsInfo, String> {
    let db = app.state::<DbState>();
    let port = setting_str(&db, "obs_ws_port", "4456")
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
    let obscmd_present = resolve_obscmd(&app).is_ok();
    Ok(ObsInfo {
        present,
        version,
        obscmd_present,
        config_dir: config_root.to_string_lossy().to_string(),
        profile: obs::OBS_PROFILE.to_string(),
        collection: obs::OBS_COLLECTION.to_string(),
        websocket_port: port,
        source,
        log_tail: obs::read_obs_log_tail(&config_root),
    })
}

/// One-click repair: stop the buffer and remove ONLY the MoonClip-owned OBS
/// config (the user's own OBS is never touched).
#[tauri::command]
pub async fn repair_obs_config(app: AppHandle) -> Result<(), String> {
    stop_engine(&app).await?;
    let root = os::obs_config_root()?;
    obs::reset_obs_config(&root)?;
    eprintln!("[moonclip] obs config reset: {}", root.display());
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct VideoOptions {
    pub codecs: Vec<CodecOpt>,
    pub heights: Vec<HeightOpt>,
    pub monitors: Vec<MonitorOpt>,
    pub current_codec: String,
    pub current_height: u32,
    pub current_fps: u32,
    pub current_monitor: String,
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

    Ok(VideoOptions {
        codecs,
        heights,
        monitors,
        current_codec,
        current_height,
        current_fps,
        current_monitor,
        buffer_height: current_height,
        transcoding: false,
        max_source_height,
        vendor,
        encoder,
        fps_options: vec![24, 30, 60, 120, 144],
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

/// Optional first-run test: start the buffer with candidate values (not
/// persisted), record for `seconds`, save, validate the file and restore the
/// previous buffer state. Returns the measured numbers for the wizard.
#[tauri::command]
pub async fn test_hardware(
    app: AppHandle,
    height: Option<u32>,
    fps: Option<u32>,
    seconds: Option<u32>,
) -> Result<HardwareTestResult, String> {
    let seconds = seconds.unwrap_or(10).clamp(5, 30);
    let overrides = StartOverrides {
        height,
        fps,
        duration_seconds: Some(seconds),
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
    let mut result = HardwareTestResult {
        ok: false,
        height: config.out_height,
        fps: config.fps,
        codec: config.codec.clone(),
        encoder: config.encoder.clone(),
        bitrate_kbps: config.bitrate_kbps,
        requested_seconds: seconds,
        startup_ms: 0,
        measured_duration_ms: 0,
        size_bytes: 0,
        fallback_height: None,
        fallback_fps: None,
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
                Ok(()) => result.ok = true,
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
