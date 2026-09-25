//! Linux platform binding for the shared embedded-OBS engine.
//!
//! Isolation strategy: OBS is launched with
//! `XDG_CONFIG_HOME=<MoonClip data dir>/obs/config`, so OBS writes its whole
//! `obs-studio/` tree (config, logs, plugin config) there. The user's own
//! `~/.config/obs-studio` is never read or written, and both apps can run at
//! the same time (`--multi`, private websocket port).
//!
//! The bundled OBS is relocatable (RUNPATH `$ORIGIN/../lib64`), so no copy is
//! needed; a system `obs` fallback (dev) gets the exact same env treatment.
//!
//! Anti-cheat: the only capture source generated is PipeWire portal screen
//! capture (compositor-level). `game_capture` (process hooking) is never used.

use std::path::{Path, PathBuf};

use crate::os::shared::encoder_options::{catalog_linux, EncoderEntry};
use crate::os::shared::engine::{ObsEngine, ObsPlatform, ObsRuntime};

/// Encoder ids compiled into the pinned Linux OBS build (Custom picker).
pub fn encoder_catalog() -> &'static [EncoderEntry] {
    catalog_linux()
}

pub struct LinuxPlatform;

/// MoonClip-owned OBS config root: `~/.local/share/MoonClip/obs/config`.
/// Exported to the child as `XDG_CONFIG_HOME`, so OBS writes
/// `<root>/obs-studio/...` there (config, logs, plugin config).
pub fn config_root() -> Result<PathBuf, String> {
    dirs::data_local_dir()
        .map(|d| d.join("MoonClip").join("obs").join("config"))
        .ok_or_else(|| "cannot resolve XDG data dir".to_string())
}

impl ObsPlatform for LinuxPlatform {
    fn prepare_runtime(&self, bundled_bin: &Path) -> Result<ObsRuntime, String> {
        // No copy: the embedded build is relocatable and every write is
        // redirected with XDG_CONFIG_HOME (see obs_launch_env). A system OBS
        // (dev fallback) is isolated exactly the same way.
        Ok(ObsRuntime {
            bin: bundled_bin.to_path_buf(),
            config_root: config_root()?,
            extra_args: vec![],
            portable: false,
        })
    }

    fn obs_launch_env(&self, config_root: &Path) -> Vec<(String, String)> {
        vec![
            (
                "XDG_CONFIG_HOME".to_string(),
                config_root.to_string_lossy().to_string(),
            ),
            // Qt's own XDG-portal registration collides with the portal
            // identity used by OBS's PipeWire client and only logs a warning
            // ("Connection already associated with an application ID").
            // Nothing in the headless engine needs Qt portal theming.
            ("QT_NO_XDG_DESKTOP_PORTAL".to_string(), "1".to_string()),
        ]
    }

    fn obs_launch_args(&self, _config_root: &Path, profile: &str, collection: &str) -> Vec<String> {
        let mut args = vec![
            "--profile".to_string(),
            profile.to_string(),
            "--collection".to_string(),
            collection.to_string(),
            "--multi".to_string(),
            "--disable-shutdown-check".to_string(),
            "--disable-updater".to_string(),
            "--only-bundled-plugins".to_string(),
        ];
        // Only meaningful when the tray icon exists: with it disabled (KDE,
        // hidden through KWin) this would just make OBS toggle its window.
        if self.sys_tray_enabled() {
            args.push("--minimize-to-tray".to_string());
        }
        args
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

    /// Hide the embedded OBS window. KDE Plasma (KWin scripting over DBus)
    /// can do it on Wayland; other desktops keep the OBS tray icon as
    /// fallback (see `sys_tray_enabled`). Never touches other processes:
    /// windows are matched by our exact child PID.
    ///
    /// A PERSISTENT script is loaded: it hides already-existing windows and
    /// connects to `workspace.windowAdded`, so the window is hidden the
    /// instant it is created (no flash) and `skipSwitcher` keeps it out of
    /// Alt+Tab (skipTaskbar alone does not). Unloaded on stop.
    fn conceal_window(&self, pid: u32, _exe: &Path) {
        if kwin_available() {
            std::thread::spawn(move || {
                // Stale scripts from force-killed sessions (KWin already
                // loaded them, so removing the files is safe).
                if let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) {
                    for e in entries.flatten() {
                        let name = e.file_name();
                        let name = name.to_string_lossy();
                        if name.starts_with("moonclip-kwin-conceal-") {
                            let _ = std::fs::remove_file(e.path());
                        }
                    }
                }
                // KWin may not be ready for scripting at spawn time; a couple
                // of quick attempts cover that without any visible window.
                let mut ok = false;
                for attempt in 0..5 {
                    if conceal_with_kwin(pid) {
                        ok = true;
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200 * (attempt + 1)));
                }
                if ok {
                    eprintln!("[moonclip] obs window concealed via persistent KWin script (pid={pid})");
                } else {
                    eprintln!("[moonclip] warning: KWin conceal failed for pid={pid}");
                }
            });
            return;
        }
        // X11 fallback (xdotool/wmctrl); Wayland without KWin relies on the
        // tray icon kept by `sys_tray_enabled`.
        use std::process::Command;
        use std::time::Duration;
        let id = pid.to_string();
        for attempt in 0..2 {
            let id = id.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(if attempt == 0 { 1 } else { 8 }));
                let _ = Command::new("xdotool")
                    .args(["search", "--pid", &id, "windowunmap"])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                let _ = Command::new("wmctrl")
                    .args(["-r", "OBS", "-b", "add,hidden"])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            });
        }
    }

    /// No tray icon when KWin will hide the window; tray otherwise.
    fn sys_tray_enabled(&self) -> bool {
        !kwin_available()
    }

    /// Unload the persistent conceal script and remove its file.
    fn unconceal_window(&self, pid: u32) {
        if !kwin_available() {
            return;
        }
        let plugin = format!("moonclip-conceal-{pid}");
        let _ = std::process::Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                "org.kde.KWin",
                "--object-path",
                "/Scripting",
                "--method",
                "org.kde.kwin.Scripting.unloadScript",
                &plugin,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let _ = std::fs::remove_file(
            std::env::temp_dir().join(format!("moonclip-kwin-conceal-{pid}.js")),
        );
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

/// KDE Plasma session with the KWin scripting DBus service reachable.
fn kwin_available() -> bool {
    let kde = std::env::var("XDG_CURRENT_DESKTOP")
        .map(|v| v.to_ascii_uppercase().contains("KDE"))
        .unwrap_or(false)
        || std::env::var("KDE_FULL_SESSION")
            .map(|v| v == "true")
            .unwrap_or(false);
    if !kde {
        return false;
    }
    std::process::Command::new("gdbus")
        .args([
            "introspect",
            "--session",
            "--dest",
            "org.kde.KWin",
            "--object-path",
            "/Scripting",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Load a PERSISTENT KWin script that hides our OBS windows (exact child PID)
/// the instant they are created and keeps them out of taskbar/pager/Alt+Tab.
/// Returns true when KWin accepted the script.
fn conceal_with_kwin(pid: u32) -> bool {
    use std::process::Command;

    let plugin = format!("moonclip-conceal-{pid}");
    let script = format!(
        "var targetPid = {pid};\n\
         function hideOurs(w) {{\n\
           if (w && w.pid === targetPid) {{\n\
             w.skipTaskbar = true;\n\
             w.skipPager = true;\n\
             w.skipSwitcher = true;\n\
             w.noBorder = true;\n\
             w.opacity = 0;\n\
             w.minimized = true;\n\
           }}\n\
         }}\n\
         var wins = workspace.windowList ? workspace.windowList() : [];\n\
         for (var i = 0; i < wins.length; ++i) {{ hideOurs(wins[i]); }}\n\
         if (workspace.windowAdded) {{ workspace.windowAdded.connect(hideOurs); }}\n"
    );
    // KWin reads the script file when the script is loaded/run, so it must
    // stay on disk while the script lives (removed on unconceal).
    let path = std::env::temp_dir().join(format!("moonclip-kwin-conceal-{pid}.js"));
    let tmp = std::env::temp_dir().join(format!("moonclip-kwin-conceal-{pid}.js.tmp"));
    if std::fs::write(&tmp, script).is_err() {
        return false;
    }
    if std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    let loaded = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.kde.KWin",
            "--object-path",
            "/Scripting",
            "--method",
            "org.kde.kwin.Scripting.loadScript",
            &path.to_string_lossy(),
            &plugin,
        ])
        .output();
    let Ok(out) = loaded else { return false };
    if !out.status.success() {
        eprintln!(
            "[moonclip] KWin loadScript failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return false;
    }
    let id: String = String::from_utf8_lossy(&out.stdout)
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    if id.is_empty() {
        eprintln!(
            "[moonclip] KWin loadScript returned no id: {}",
            String::from_utf8_lossy(&out.stdout).trim()
        );
        return false;
    }
    let run = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.kde.KWin",
            "--object-path",
            &format!("/Scripting/Script{id}"),
            "--method",
            "org.kde.kwin.Script.run",
        ])
        .output();
    match run {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            eprintln!(
                "[moonclip] KWin Script.run failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            false
        }
        Err(e) => {
            eprintln!("[moonclip] KWin Script.run spawn failed: {e}");
            false
        }
    }
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
        assert_ne!(id, crate::os::shared::engine::FORBIDDEN_SOURCE_ID);
    }

    #[test]
    fn config_lives_inside_moonclip_data_dir() {
        let root = config_root().unwrap();
        let s = root.to_string_lossy();
        assert!(s.contains("MoonClip"), "{s}");
        assert!(s.ends_with("obs/config"), "{s}");
        assert!(!s.ends_with(".config/obs-studio"), "{s}");
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
        // Neither flag exists on Linux OBS; isolation is XDG_CONFIG_HOME only.
        assert!(!s.contains("--config-dir"), "{s}");
        assert!(!s.contains("--portable"), "{s}");
    }

    /// The launch command line must never name the upstream product.
    #[test]
    fn launch_args_have_no_upstream_name() {
        let args = p().obs_launch_args(Path::new("/tmp/cfg"), "MoonClip", "MoonClip");
        let s = args.join(" ").to_lowercase();
        assert!(!s.contains("obs"), "{s}");
    }

    #[test]
    fn launch_env_isolates_config() {
        let rt = p().prepare_runtime(Path::new("/usr/bin/obs")).unwrap();
        assert!(rt.extra_args.is_empty(), "{:?}", rt.extra_args);
        let env = p().obs_launch_env(Path::new("/home/u/.local/share/MoonClip/obs/config"));
        assert_eq!(
            env,
            vec![
                (
                    "XDG_CONFIG_HOME".to_string(),
                    "/home/u/.local/share/MoonClip/obs/config".to_string()
                ),
                ("QT_NO_XDG_DESKTOP_PORTAL".to_string(), "1".to_string()),
            ]
        );
    }
}
