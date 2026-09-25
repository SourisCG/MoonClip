//! obs-websocket v5 client for the embedded OBS (replaces the obs-cmd CLI).
//!
//! Why not obs-cmd: its latest release (v1.0.2) ships `input settings`,
//! `input volume` and `input mute` as stubs that only print "experimental"
//! and never talk to OBS. `obws` is the same crate obs-cmd uses internally,
//! so MoonClip controls everything directly: replay buffer, scenes, sources,
//! video settings and inputs (volume/mute/settings). Localhost only, plain
//! `ws://` (no TLS feature), authenticated with our generated password.

use std::path::{Path, PathBuf};
use std::time::Duration;

use obws::requests::config::SetVideoSettings;
use obws::requests::inputs::{InputId, Volume};
use obws::requests::sources::{SaveScreenshot, SourceId};
use obws::Client;

/// How long `replay_save` waits for OBS to flush the replay file to disk.
const REPLAY_SAVE_TIMEOUT: Duration = Duration::from_secs(45);
/// Poll interval while waiting for the flushed replay path.
const REPLAY_SAVE_POLL: Duration = Duration::from_millis(250);

pub struct Obsws {
    client: Client,
}

impl Obsws {
    /// Connect to the private obs-websocket (host is always localhost).
    pub async fn connect(port: u16, password: &str) -> Result<Self, String> {
        let client = Client::connect("127.0.0.1", port, Some(password))
            .await
            .map_err(|e| format!("obs-websocket connect failed: {e}"))?;
        Ok(Self { client })
    }

    pub async fn replay_start(&self) -> Result<(), String> {
        self.client
            .replay_buffer()
            .start()
            .await
            .map_err(|e| format!("replay start failed: {e}"))
    }

    pub async fn replay_stop(&self) -> Result<(), String> {
        self.client
            .replay_buffer()
            .stop()
            .await
            .map_err(|e| format!("replay stop failed: {e}"))
    }

    pub async fn replay_status(&self) -> Result<bool, String> {
        self.client
            .replay_buffer()
            .status()
            .await
            .map_err(|e| format!("replay status failed: {e}"))
    }

    /// Save the buffer and return the written file path. OBS flushes
    /// asynchronously, so poll `last_replay` until it changes (this is the
    /// reliability fix obs-cmd carried for issue #103; kept here directly).
    pub async fn replay_save(&self) -> Result<PathBuf, String> {
        let before = self.client.replay_buffer().last_replay().await.ok();
        self.client
            .replay_buffer()
            .save()
            .await
            .map_err(|e| format!("replay save failed: {e}"))?;
        let deadline = std::time::Instant::now() + REPLAY_SAVE_TIMEOUT;
        loop {
            if let Ok(path) = self.client.replay_buffer().last_replay().await {
                if !path.is_empty() && Some(&path) != before.as_ref() {
                    return Ok(PathBuf::from(path));
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err("replay saved but no new file appeared within 45 s".into());
            }
            tokio::time::sleep(REPLAY_SAVE_POLL).await;
        }
    }

    /// Active program scene name (used to prove the generated scene is live;
    /// a mismatch records black).
    pub async fn current_scene_name(&self) -> Result<String, String> {
        let scene = self
            .client
            .scenes()
            .current_program_scene()
            .await
            .map_err(|e| format!("current scene failed: {e}"))?;
        let name = scene.id.name;
        if name.is_empty() {
            Err("OBS reported no current program scene".to_string())
        } else {
            Ok(name)
        }
    }

    /// Whether a source is producing frames in the program (portal stream
    /// connected). More reliable than a screenshot when validating the start.
    pub async fn source_active(&self, source: &str) -> Result<bool, String> {
        let active = self
            .client
            .sources()
            .active(SourceId::Name(source))
            .await
            .map_err(|e| format!("source active failed: {e}"))?;
        Ok(active.active)
    }

    /// Save a source screenshot (`png`). Used to learn the real captured size.
    pub async fn save_screenshot(&self, source: &str, path: &Path) -> Result<(), String> {
        self.client
            .sources()
            .save_screenshot(SaveScreenshot {
                source: SourceId::Name(source),
                format: "png",
                width: None,
                height: None,
                compression_quality: None,
                file_path: path,
            })
            .await
            .map_err(|e| format!("save screenshot failed: {e}"))
    }

    /// Resize canvas/output at runtime (fps untouched).
    pub async fn set_video_settings(
        &self,
        base_w: u32,
        base_h: u32,
        out_w: u32,
        out_h: u32,
    ) -> Result<(), String> {
        self.client
            .config()
            .set_video_settings(SetVideoSettings {
                base_width: Some(base_w),
                base_height: Some(base_h),
                output_width: Some(out_w),
                output_height: Some(out_h),
                ..Default::default()
            })
            .await
            .map_err(|e| format!("set video settings failed: {e}"))
    }

    /// Current settings of a source (portal `RestoreToken` lives here after a
    /// successful screencast session).
    pub async fn input_settings(&self, source: &str) -> Result<serde_json::Value, String> {
        let settings = self
            .client
            .inputs()
            .settings::<serde_json::Value>(InputId::Name(source))
            .await
            .map_err(|e| format!("input settings failed: {e}"))?;
        Ok(settings.settings)
    }

    /// Live per-track gain (0-200 % -> OBS multiplier 0.0-2.0).
    pub async fn set_input_volume(&self, source: &str, percent: u32) -> Result<(), String> {
        let mul = ((percent.clamp(0, 200) as f32) / 100.0).clamp(0.0, 2.0);
        self.client
            .inputs()
            .set_volume(InputId::Name(source), Volume::Mul(mul))
            .await
            .map_err(|e| format!("set volume failed: {e}"))
    }

    /// Live mute/unmute of a generated audio source.
    pub async fn set_input_muted(&self, source: &str, muted: bool) -> Result<(), String> {
        self.client
            .inputs()
            .set_muted(InputId::Name(source), muted)
            .await
            .map_err(|e| format!("set mute failed: {e}"))
    }
}
