//! Linux platform binding for the shared embedded-OBS engine.
//!
//! Isolation strategy: MoonClip stages its OWN writable copy of the embedded
//! OBS under `~/.local/share/MoonClip/obs`, drops a `portable_mode.txt` at its
//! root and launches it with `--portable`, so OBS writes its `config/` INSIDE
//! that copy by construction. `~/.config/obs-studio` is untouchable.
//!
//! If OBS is resolved from the system PATH (`/usr/bin/obs`, flatpak wrapper…)
//! there is nothing to copy: MoonClip falls back to `--config-dir` and the
//! config guard in `os/obs.rs` kills the child if the dir is not honored —
//! never mixing configs silently.
//!
//! Anti-cheat: the only capture source generated is PipeWire portal screen
//! capture (compositor-level). `game_capture` (process hooking) is never used.

use std::path::{Path, PathBuf};

use tokio::process::Command;

use super::super::encoder_options::{catalog_linux, EncoderEntry};
use super::super::obs::{
    copy_dir_recursive, marker_matches, obs_build_fingerprint, write_marker, ObsEngine,
    ObsPlatform, ObsRuntime,
};

/// Encoder ids compiled into the pinned Linux OBS build (Custom picker).
pub fn encoder_catalog() -> &'static [EncoderEntry] {
    catalog_linux()
}

pub struct LinuxPlatform;

/// MoonClip-owned OBS runtime root: `~/.local/share/MoonClip/obs`.
pub fn runtime_root() -> Result<PathBuf, String> {
    dirs::data_local_dir()
        .map(|d| d.join("MoonClip").join("obs"))
        .ok_or_else(|| "cannot resolve XDG data dir".to_string())
}

/// Dir that will contain `obs-studio/` (what `write_obs_config` fills).
pub fn config_root() -> Result<PathBuf, String> {
    Ok(runtime_root()?.join("config"))
}

/// System install prefixes we must not copy from (read-only or huge).
fn is_system_path(bin: &Path) -> bool {
    let s = bin.to_string_lossy();
    [
        "/usr/",
        "/opt/",
        "/snap/",
        "/var/lib/flatpak/",
        "/app/",
        "/nix/store/",
    ]
    .iter()
    .any(|p| s.starts_with(p))
}

/// Stage (once per bundled build) the writable portable copy. The bundled
/// Linux layout is `<prefix>/bin/obs` + `<prefix>/lib` + `<prefix>/share`.
fn stage_runtime(obs_bin: &Path) -> Result<PathBuf, String> {
    let root = runtime_root()?;
    let runtime_bin = root.join("bin").join("obs");
    let fingerprint = obs_build_fingerprint(obs_bin);
    let marker = root.join(".moonclip-source");
    if runtime_bin.exists() && marker_matches(&marker, &fingerprint) {
        return Ok(runtime_bin);
    }
    let src = obs_bin
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .ok_or_else(|| format!("unexpected OBS layout: {}", obs_bin.display()))?;
    eprintln!(
        "[moonclip] staging portable OBS copy: {} -> {}",
        src.display(),
        root.display()
    );
    if root.exists() {
        std::fs::remove_dir_all(&root)
            .map_err(|e| format!("cannot refresh OBS runtime {}: {e}", root.display()))?;
    }
    copy_dir_recursive(&src, &root)?;
    std::fs::write(root.join("portable_mode.txt"), b"")
        .map_err(|e| format!("cannot enable OBS portable mode: {e}"))?;
    write_marker(&marker, &fingerprint)?;
    if !runtime_bin.exists() {
        return Err(format!(
            "OBS runtime copy incomplete: {} missing",
            runtime_bin.display()
        ));
    }
    Ok(runtime_bin)
}

impl ObsPlatform for LinuxPlatform {
    fn prepare_runtime(&self, bundled_bin: &Path) -> Result<ObsRuntime, String> {
        if is_system_path(bundled_bin) {
            // Best effort: isolate a system OBS with --config-dir; the config
            // guard aborts if the build ignores it.
            return Ok(ObsRuntime {
                bin: bundled_bin.to_path_buf(),
                config_root: config_root()?,
                extra_args: vec![
                    "--config-dir".into(),
                    config_root()?.to_string_lossy().to_string(),
                ],
                portable: false,
            });
        }
        let bin = stage_runtime(bundled_bin)?;
        Ok(ObsRuntime {
            bin,
            config_root: config_root()?,
            extra_args: vec!["--portable".into()],
            portable: true,
        })
    }

    fn obs_launch_args(&self, _config_root: &Path, profile: &str, collection: &str) -> Vec<String> {
        vec![
            "--profile".into(),
            profile.into(),
            "--collection".into(),
            collection.into(),
            "--multi".into(),
            "--minimize-to-tray".into(),
            "--disable-shutdown-check".into(),
            "--disable-updater".into(),
            "--only-bundled-plugins".into(),
        ]
    }

    fn working_dir(&self, obs_bin: &Path) -> Option<PathBuf> {
        // The tarball layout expects CWD=bin/ so it can find lib/ and share/.
        obs_bin
            .parent()
            .map(|p| p.to_path_buf())
            .or_else(|| Some(PathBuf::from(".")))
    }

    fn kill_orphans(&self, obs_bin: &Path) {
        kill_orphan_process(obs_bin);
    }

    /// Best-effort window hiding on X11 (Wayland has no global window
    /// control): unmap OBS top-level windows via xdotool/wmctrl when
    /// installed. All failures ignored; the Linux owner iterates here.
    fn conceal_window(&self, pid: u32) {
        use std::process::Command;
        use std::time::Duration;
        let id = pid.to_string();
        // Fire twice (right away + after boot) so a late-created main
        // window is still caught; each attempt is self-limiting.
        for attempt in 0..2 {
            let id = id.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(if attempt == 0 { 1 } else { 8 }));
                // xdotool: unmap every window owned by our PID.
                let _ = Command::new("xdotool")
                    .args(["search", "--pid", &id, "windowunmap"])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                // wmctrl fallback: hide by OBS window title.
                let _ = Command::new("wmctrl")
                    .args(["-r", "OBS", "-b", "add,hidden"])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            });
        }
    }

    fn video_source(&self, _monitor: &str, window: &str) -> (&'static str, serde_json::Value) {
        // Wayland/X11 screen capture through the XDG portal (compositor-level,
        // no injection). OBS opens the system picker on first use; the user
        // can persist the choice in the portal (Remember).
        if !window.trim().is_empty() {
            return ("pipewire-window-capture-source", serde_json::json!({}));
        }
        ("pipewire-desktop-capture-source", serde_json::json!({}))
    }

    fn game_audio_source_id(&self) -> &'static str {
        "pulse_output_capture"
    }

    fn mic_audio_source_id(&self) -> &'static str {
        "pulse_input_capture"
    }

    fn encoder_id(&self, vendor: &str, codec: &str, encoder: &str) -> Option<&'static str> {
        if encoder == "cpu" {
            return Some("obs_x264");
        }
        match (vendor, codec) {
            ("nvidia", "h264") => Some("obs_nvenc_h264_tex"),
            ("nvidia", "hevc") => Some("obs_nvenc_hevc_tex"),
            ("nvidia", "av1") => Some("obs_nvenc_av1_tex"),
            ("intel", "h264") => Some("obs_qsv11_v2"),
            ("intel", "hevc") => Some("obs_qsv11_hevc"),
            ("intel", "av1") => Some("obs_qsv11_av1"),
            // AMD on Linux has no AMF; OBS goes through FFmpeg VAAPI.
            // Each codec needs its own VAAPI id: `ffmpeg_vaapi` is H.264-only.
            ("amd", "h264") => Some("ffmpeg_vaapi"),
            ("amd", "hevc") => Some("hevc_ffmpeg_vaapi"),
            ("amd", "av1") => Some("av1_ffmpeg_vaapi"),
            _ => None,
        }
    }

    fn encoder_catalog(&self) -> &'static [EncoderEntry] {
        encoder_catalog()
    }
}

/// New engine wired to this platform.
pub fn new_engine() -> ObsEngine {
    ObsEngine::new(Box::new(LinuxPlatform))
}

/// Kill our own leftover OBS processes (force-killed sessions skip Drop).
/// Matches `/proc/<pid>/exe` against OUR runtime binary and only when the
/// parent is gone; the user's own OBS never matches.
fn kill_orphan_process(obs_bin: &Path) {
    let Ok(want) = std::fs::canonicalize(obs_bin) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return;
    };
    let mut by_pid: std::collections::HashMap<u32, (u32, PathBuf)> =
        std::collections::HashMap::new();
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) else {
            continue;
        };
        let parent = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| stat.rsplit_once(')').map(|(_, rest)| rest.to_string()))
            .and_then(|rest| rest.split_whitespace().nth(1)?.parse::<u32>().ok())
            .unwrap_or(0);
        by_pid.insert(pid, (parent, exe));
    }
    for (pid, (parent, exe)) in &by_pid {
        if *pid == std::process::id() || *parent == 0 {
            continue;
        }
        if by_pid.contains_key(parent) {
            continue; // parent alive: running engine, never touch it
        }
        if exe != &want {
            continue;
        }
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(*pid as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
        eprintln!("[moonclip] killed orphan OBS pid={pid}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> LinuxPlatform {
        LinuxPlatform
    }

    #[test]
    fn encoder_ids_cover_linux_blocks() {
        assert_eq!(
            p().encoder_id("nvidia", "h264", "gpu"),
            Some("obs_nvenc_h264_tex")
        );
        assert_eq!(p().encoder_id("intel", "h264", "gpu"), Some("obs_qsv11_v2"));
        assert_eq!(p().encoder_id("amd", "h264", "gpu"), Some("ffmpeg_vaapi"));
        // VAAPI ids are codec-specific: the H.264 id must never serve HEVC/AV1.
        assert_eq!(
            p().encoder_id("amd", "hevc", "gpu"),
            Some("hevc_ffmpeg_vaapi")
        );
        assert_eq!(
            p().encoder_id("amd", "av1", "gpu"),
            Some("av1_ffmpeg_vaapi")
        );
        assert_eq!(p().encoder_id("nvidia", "h264", "cpu"), Some("obs_x264"));
    }

    #[test]
    fn video_source_is_portal_never_game() {
        let (id, _) = p().video_source("", "");
        assert_eq!(id, "pipewire-desktop-capture-source");
        let (id, _) = p().video_source("", "Game");
        assert_eq!(id, "pipewire-window-capture-source");
        assert_ne!(id, super::super::super::obs::FORBIDDEN_SOURCE_ID);
    }

    #[test]
    fn runtime_lives_inside_moonclip_data_dir() {
        let root = runtime_root().unwrap();
        let s = root.to_string_lossy();
        assert!(s.contains("MoonClip"), "{s}");
        assert!(!s.ends_with(".config/obs-studio"), "{s}");
        assert_eq!(config_root().unwrap(), root.join("config"));
    }

    #[test]
    fn launch_args_do_not_force_config_dir() {
        let args = p().obs_launch_args(
            Path::new("/home/u/.local/share/MoonClip/obs/config"),
            "MoonClip",
            "MoonClip",
        );
        let s = args.join(" ");
        assert!(s.contains("--multi"), "{s}");
        assert!(!s.contains("--config-dir"), "{s}");
    }

    #[test]
    fn system_paths_are_detected() {
        assert!(is_system_path(Path::new("/usr/bin/obs")));
        assert!(is_system_path(Path::new("/var/lib/flatpak/app/obs")));
        assert!(!is_system_path(Path::new(
            "/home/u/.local/share/MoonClip/obs/bin/obs"
        )));
        assert!(!is_system_path(Path::new("/tmp/obs-portable/bin/obs")));
    }

    #[test]
    fn prepare_runtime_falls_back_for_system_obs() {
        let rt = p().prepare_runtime(Path::new("/usr/bin/obs")).unwrap();
        assert!(!rt.portable);
        assert!(rt.extra_args.iter().any(|a| a == "--config-dir"));
    }
}
