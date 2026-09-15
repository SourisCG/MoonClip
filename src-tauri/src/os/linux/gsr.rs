//! Linux engine: gpu-screen-recorder as a replay-buffer daemon.
//! Ship model (see docs/THIRD_PARTY.md): prebuilt GSR sidecar bundled in the
//! package (rpm/deb/AppImage) or built as a Flatpak module. At runtime we
//! resolve: MOONCLIP_GSR_BIN override -> bundled sidecar -> system PATH.

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::time::sleep;

use super::super::{CaptureConfig, CaptureEngine, SavePlan};

pub struct LinuxGsrEngine {
    child: Option<Child>,
    output_dir: PathBuf,
    audio_args: Vec<String>,
    save_plan: Option<SavePlan>,
}

impl LinuxGsrEngine {
    pub fn new() -> Self {
        Self {
            child: None,
            output_dir: PathBuf::new(),
            audio_args: Vec::new(),
            save_plan: None,
        }
    }

    /// Locate the GSR binary: env override, bundled sidecar, then PATH.
    pub fn resolve_binary() -> Result<PathBuf, String> {
        if let Ok(path) = std::env::var("MOONCLIP_GSR_BIN") {
            let p = PathBuf::from(&path);
            if p.exists() {
                return Ok(p);
            }
            return Err(format!("MOONCLIP_GSR_BIN points nowhere: {path}"));
        }
        // Bundled sidecar next to the app binary (rpm/deb/AppImage layout).
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                for name in ["moonclip-gsr/gpu-screen-recorder", "gpu-screen-recorder"] {
                    let p = dir.join(name);
                    if p.exists() {
                        return Ok(p);
                    }
                }
            }
        }
        // System install (dev machines / Terra-COPR rpm).
        if let Ok(path) = which_gsr() {
            return Ok(path);
        }
        Err("gpu-screen-recorder not found. Install it (dev) — end users get it bundled.".into())
    }
}

fn which_gsr() -> Result<PathBuf, String> {
    let out = std::process::Command::new("sh")
        .args(["-c", "command -v gpu-screen-recorder"])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("not in PATH".into());
    }
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if p.is_empty() {
        return Err("not in PATH".into());
    }
    Ok(PathBuf::from(p))
}

impl CaptureEngine for LinuxGsrEngine {
    async fn start_buffer(&mut self, config: CaptureConfig) -> Result<(), String> {
        if self.child.is_some() {
            return Err("recorder already running".into());
        }
        let bin = match &config.gsr_bin {
            Some(p) => p.clone(),
            None => Self::resolve_binary()?,
        };
        std::fs::create_dir_all(&config.output_dir)
            .map_err(|e| format!("cannot create clips dir: {e}"))?;
        // Track layout (order = track number):
        //   -a "<desktop>|<mic>" = track 1, MIX (plays everywhere)
        //   -a "<desktop>"       = track 2, game only
        //   -a "<mic>"           = track 3, mic only
        let audio_args = vec![
            format!("{}|{}", config.desktop_device, config.mic_device),
            config.desktop_device.clone(),
            config.mic_device.clone(),
        ];
        let mut cmd = Command::new(&bin);
        let source = if config.source.trim().is_empty() {
            "screen"
        } else {
            config.source.trim()
        };
        cmd.args([
            "-w", source,
            "-f", &config.fps.to_string(),
            "-k", &config.codec,
            "-c", "mp4",
            "-r", &config.duration_seconds.to_string(),
        ]);
        if let Some(scale) = scale_arg(config.out_height) {
            cmd.args(["-s", &scale]);
        }
        cmd.args([
            "-bm", "cbr",
            "-q", &config.bitrate_kbps.to_string(),
            "-tune", "quality",
            "-keyint", "2",
        ]);
        if let Some(opts) = &config.nvenc_opts {
            cmd.args(["-ffmpeg-video-opts", opts]);
        }
        let child = cmd
            .arg("-a")
            .arg(&audio_args[0])
            .arg("-a")
            .arg(&audio_args[1])
            .arg("-a")
            .arg(&audio_args[2])
            .args([
                "-ac", "aac",
                "-ab", "160",
                "-o", config.output_dir.to_str().ok_or("bad clips dir")?,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("cannot launch {}: {e}", bin.display()))?;
        self.output_dir = config.output_dir;
        self.audio_args = audio_args;
        self.save_plan = if config.save_height > 0 {
            Some(SavePlan {
                height: config.save_height,
                bitrate_kbps: config.save_bitrate_kbps,
                codec: config.codec.clone(),
                fps: config.fps,
                encoder: config.save_encoder,
            })
        } else {
            None
        };
        self.child = Some(child);
        Ok(())
    }

    async fn save_clip(&mut self) -> Result<PathBuf, String> {
        let child = self.child.as_ref().ok_or("recorder not running")?;
        let pid = child.id().ok_or("recorder has no PID")?;
        // Accept files touched from 1 s before the signal (FS mtime
        // granularity), never older ones: a previous save must not be
        // returned while the fresh remux is still being written.
        let since = std::time::SystemTime::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        kill(Pid::from_raw(pid as i32), Signal::SIGUSR1)
            .map_err(|e| format!("SIGUSR1 failed: {e}"))?;
        // GSR remuxes the RAM ring to disk; a fixed sleep proved racy on slow
        // disks / large rings, so poll for the fresh file instead.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(p) = newest_modified_after(&self.output_dir, since) {
                return Ok(p);
            }
            if std::time::Instant::now() >= deadline {
                return Err("no clip file appeared within 5 s of the save signal".into());
            }
            sleep(Duration::from_millis(100)).await;
        }
    }

    async fn stop_buffer(&mut self) -> Result<(), String> {
        if let Some(mut child) = self.child.take() {
            if let Some(pid) = child.id() {
                let _ = kill(Pid::from_raw(pid as i32), Signal::SIGINT);
                // Give it a moment, then wait (kill_on_drop covers hangs).
                let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
            }
        }
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "gpu-screen-recorder"
    }

    fn audio_args(&self) -> Vec<String> {
        self.audio_args.clone()
    }

    fn save_plan(&self) -> Option<SavePlan> {
        self.save_plan.clone()
    }
}

/// GSR `-s` value for ladder heights (16:9 box, kept aspect). None = original.
/// GSR-CLI-specific: lives in the Linux backend (Windows scales in-ffmpeg).
fn scale_arg(height: u32) -> Option<String> {
    match height {
        360 => Some("640x360".to_string()),
        720 => Some("1280x720".to_string()),
        1080 => Some("1920x1080".to_string()),
        1440 => Some("2560x1440".to_string()),
        _ => None,
    }
}

/// Newest `.mp4` in `dir` modified at/after `since` (None if none qualify).
/// Time-filtered on purpose: never falls back to a pre-signal clip.
fn newest_modified_after(dir: &Path, since: std::time::SystemTime) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().map_or(false, |x| x == "mp4"))
        .filter(|p| {
            p.metadata()
                .and_then(|m| m.modified())
                .map(|m| m >= since)
                .unwrap_or(false)
        })
        .max_by_key(|p| {
            p.metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        })
}

#[cfg(test)]
mod tests {
    use super::newest_modified_after;

    #[test]
    fn newest_modified_after_filters_by_time() {
        let dir = std::env::temp_dir().join(format!("moonclip-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("Replay_test.mp4");
        std::fs::write(&clip, b"v").unwrap();
        std::fs::write(dir.join("thumb_test.jpg"), b"t").unwrap();
        let now = std::time::SystemTime::now();
        assert_eq!(
            newest_modified_after(&dir, now - std::time::Duration::from_secs(60)),
            Some(clip)
        );
        assert_eq!(
            newest_modified_after(&dir, now + std::time::Duration::from_secs(60)),
            None
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
