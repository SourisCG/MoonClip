//! Editor sessions: stems, preview proxy, media URLs and cleanup.
//!
//! Opening the heavy editor creates a session directory (audio stems + an
//! H.264 preview proxy when the source codec cannot play in WebKitGTK).
//! Closing it kills any running export, releases the media-server tokens and
//! deletes the directory: nothing of the editor survives in normal mode.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use tauri::{AppHandle, Manager};
use tokio::process::Child;

use super::encoders::{self, EncoderInfo};
use super::project::{self, EditProject};

pub struct Session {
    pub id: String,
    pub dir: PathBuf,
    pub export: Arc<tokio::sync::Mutex<Option<Child>>>,
}

static SESSIONS: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();

fn sessions() -> &'static Mutex<HashMap<String, Session>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn get(session_id: &str) -> Option<(String, PathBuf)> {
    sessions()
        .lock()
        .ok()?
        .get(session_id)
        .map(|s| (s.id.clone(), s.dir.clone()))
}

pub fn export_handle(session_id: &str) -> Option<Arc<tokio::sync::Mutex<Option<Child>>>> {
    sessions()
        .lock()
        .ok()?
        .get(session_id)
        .map(|s| s.export.clone())
}

/// Close the session: kill exports, release media tokens, delete the dir.
pub async fn close(session_id: &str) -> Result<(), String> {
    let session = sessions()
        .lock()
        .map_err(|_| "editor sessions lock poisoned".to_string())?
        .remove(session_id);
    let Some(session) = session else {
        return Ok(());
    };
    {
        let mut guard = session.export.lock().await;
        if let Some(mut child) = guard.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
    crate::editor::media_server::release_session(session_id);
    if let Err(e) = std::fs::remove_dir_all(&session.dir) {
        eprintln!(
            "[moonclip] warning: cannot remove editor session dir {}: {e}",
            session.dir.display()
        );
    } else {
        eprintln!("[moonclip] editor session closed: {session_id}");
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrackInfo {
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorClipInfo {
    pub id: String,
    pub file_name: String,
    pub game_title: String,
    pub duration_ms: i64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub codec: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorOpenResult {
    pub session_id: String,
    pub project: EditProject,
    pub clip: EditorClipInfo,
    pub video_url: String,
    pub using_proxy: bool,
    pub audio_tracks: Vec<AudioTrackInfo>,
    pub encoders: Vec<EncoderInfo>,
}

/// Directory for saved edit projects (one per source clip, Medal-style reopen).
pub fn edits_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data dir: {e}"))?
        .join("edits");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create edits dir: {e}"))?;
    Ok(dir)
}

pub fn load_project(app: &AppHandle, clip_id: &str) -> Option<EditProject> {
    let path = edits_dir(app).ok()?.join(format!("{clip_id}.json"));
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_project(app: &AppHandle, project: &EditProject) -> Result<(), String> {
    let dir = edits_dir(app)?;
    let mut project = project.clone();
    project.updated_at = format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    let text = serde_json::to_string_pretty(&project).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{}.json", project.source_clip_id)), text)
        .map_err(|e| format!("cannot save edit project: {e}"))
}

/// Count audio streams in a file from `ffmpeg -i` stderr.
pub fn parse_audio_track_count(stderr: &str) -> usize {
    stderr
        .lines()
        .filter(|l| l.contains("Stream #") && l.contains("Audio:"))
        .count()
}

pub const TRACK_LABELS: [&str; 3] = ["mix", "game", "mic"];

/// Extract the clip's audio streams as playable stems (mix/game/mic by order).
/// URLs are session-scoped so `close` releases them with the temp dir.
async fn extract_stems(
    ffmpeg: &Path,
    input: &Path,
    dir: &Path,
    session_id: &str,
    track_count: usize,
) -> Result<Vec<AudioTrackInfo>, String> {
    let mut tracks = Vec::new();
    for (i, label) in TRACK_LABELS.iter().enumerate().take(track_count) {
        let out = dir.join(format!("stem_{i}.m4a"));
        let status = tokio::process::Command::new(ffmpeg)
            .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(input)
            .args(["-map", &format!("0:a:{i}"), "-c:a", "aac", "-b:a", "192k"])
            .arg(&out)
            .status()
            .await
            .map_err(|e| format!("cannot extract audio stem: {e}"))?;
        if status.success() && out.is_file() {
            tracks.push(AudioTrackInfo {
                label: (*label).to_string(),
                url: crate::editor::media_server::media_url_session(Some(session_id), &out)?,
            });
        }
    }
    Ok(tracks)
}

/// H.264 proxy generation for sources the WebView cannot decode (HEVC/AV1).
fn proxy_args(encoder: &str, height: u32, input: &Path) -> Vec<String> {
    let scale = format!("scale=-2:'min({height},720)'");
    let mut a: Vec<String> = [
        "-y", "-hide_banner", "-loglevel", "error", "-i", "", "-an", "-vf",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    a[5] = input.to_string_lossy().into_owned();
    a.push(scale);
    match encoder {
        "libx264" => a.extend(["-preset", "ultrafast", "-crf", "24"].map(String::from)),
        _ => a.extend(["-b:v", "4000k"].map(String::from)),
    }
    a
}

/// Open (or reuse) an editor session for a clip.
pub async fn open(app: &AppHandle, clip_id: &str) -> Result<EditorOpenResult, String> {
    let (clip, input, ffmpeg) = {
        let db = app.state::<crate::storage::DbState>();
        let clip = db
            .list_clips()?
            .into_iter()
            .find(|c| c.id == clip_id)
            .ok_or_else(|| "clip not found".to_string())?;
        let base = db.clips_dir()?;
        let input = crate::commands::validated_media_path(&base, &clip.file_name)?;
        let ffmpeg = crate::editor::ffmpeg::resolve_ffmpeg(app)?;
        (clip, input, ffmpeg)
    };

    let session_id = uuid::Uuid::new_v4().to_string();
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("no cache dir: {e}"))?
        .join("editor")
        .join(&session_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create editor dir: {e}"))?;

    // Probe streams (codec/size/fps) — the export and proxy decisions need it.
    let stderr = tokio::process::Command::new(&ffmpeg)
        .args(["-hide_banner", "-i"])
        .arg(&input)
        .output()
        .await
        .map(|o| String::from_utf8_lossy(&o.stderr).to_string())
        .unwrap_or_default();
    let probe = crate::editor::ffmpeg::probe_video_stream(&ffmpeg, &input).await;
    let (width, height, fps, codec) = probe
        .as_ref()
        .map(|p| (p.width, p.height, p.fps, p.codec_name.clone()))
        .unwrap_or((0, 0, 0.0, String::new()));
    let audio_tracks = extract_stems(
        &ffmpeg,
        &input,
        &dir,
        &session_id,
        parse_audio_track_count(&stderr),
    )
    .await?;

    let encoders = encoders::detect(&ffmpeg).await;
    let auto_encoder = encoders
        .iter()
        .find(|e| e.hw && e.available)
        .map(|e| e.id.clone())
        .unwrap_or_else(|| "libx264".to_string());

    // Proxy only when the WebView cannot decode the source codec.
    let mut using_proxy = false;
    let video_path = if !codec.is_empty() && codec != "h264" {
        let proxy = dir.join("proxy.mp4");
        let args = proxy_args(&auto_encoder, height, &input);
        let status = tokio::process::Command::new(&ffmpeg)
            .args(&args)
            .arg(&proxy)
            .status()
            .await
            .map_err(|e| format!("proxy encode failed: {e}"))?;
        if status.success() && proxy.is_file() {
            using_proxy = true;
            proxy
        } else {
            eprintln!("[moonclip] warning: proxy generation failed; using source");
            input.clone()
        }
    } else {
        input.clone()
    };

    let project = load_project(app, clip_id).unwrap_or_else(|| {
        let name = Path::new(&clip.file_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&clip.file_name)
            .to_string();
        project::default_project(clip_id, &name, clip.duration_ms)
    });

    let video_url =
        crate::editor::media_server::media_url_session(Some(&session_id), &video_path)?;

    let session = Session {
        id: session_id.clone(),
        dir,
        export: Arc::new(tokio::sync::Mutex::new(None)),
    };
    sessions()
        .lock()
        .map_err(|_| "editor sessions lock poisoned".to_string())?
        .insert(session_id.clone(), session);

    Ok(EditorOpenResult {
        session_id,
        project,
        clip: EditorClipInfo {
            id: clip.id,
            file_name: clip.file_name,
            game_title: clip.game_title,
            duration_ms: clip.duration_ms,
            width,
            height,
            fps,
            codec,
        },
        video_url,
        using_proxy,
        audio_tracks,
        encoders,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_audio_streams_from_ffmpeg_stderr() {
        let stderr = "\
Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'clip.mp4':
  Stream #0:0(und): Video: h264 (High) (avc1 / 0x31637661), yuv420p, 1920x1080, 60 fps
  Stream #0:1(und): Audio: aac (LC) (mp4a / 0x6134706D), 48000 Hz, stereo, fltp
  Stream #0:2(und): Audio: aac (LC) (mp4a / 0x6134706D), 48000 Hz, stereo, fltp
  Stream #0:3(und): Audio: aac (LC) (mp4a / 0x6134706D), 48000 Hz, stereo, fltp
";
        assert_eq!(parse_audio_track_count(stderr), 3);
        assert_eq!(parse_audio_track_count("no streams here"), 0);
    }

    #[test]
    fn proxy_args_target_h264_and_drop_audio() {
        let a = proxy_args("libx264", 1440, Path::new("/clips/in.mp4"));
        assert!(a.contains(&"-an".to_string()));
        assert!(a.windows(2).any(|w| w == ["-preset", "ultrafast"]));
        assert!(a.iter().any(|x| x.contains("min(1440,720)")));
        assert!(a.contains(&"/clips/in.mp4".to_string()));
    }
}
