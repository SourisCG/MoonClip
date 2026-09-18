//! Windows platform binding for the shared embedded-OBS engine.
//!
//! Isolation strategy (proven requirement): OBS Studio does NOT honor
//! `--config-dir` on Windows (verified: it logged "Portable mode: false" and
//! wrote to `%APPDATA%\obs-studio`). So MoonClip stages its OWN writable copy
//! of the embedded OBS under `%LOCALAPPDATA%\MoonClip\obs`, drops a
//! `portable_mode.txt` at its root and launches it with `--portable`. OBS then
//! writes its `config/` INSIDE that copy by construction — the user's
//! `%APPDATA%\obs-studio` is untouchable, not merely avoided.
//!
//! `--multi` lets our instance coexist with the user's own OBS.
//!
//! Anti-cheat: the only capture source we generate is `monitor_capture`
//! (Desktop Duplication / WGC, compositor-level). `game_capture` is never
//! used — it hooks game processes and is what kernel anti-cheats block.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tokio::process::Command;

use super::super::obs::{
    copy_dir_recursive, marker_matches, obs_build_fingerprint, write_marker, ObsEngine,
    ObsPlatform, ObsRuntime,
};

pub struct WindowsPlatform {
    /// Job object handle (raw) created once; OBS children are assigned to it so
    /// they die when MoonClip does. 0 = creation failed (fall back to Drop).
    job: OnceLock<isize>,
}

impl WindowsPlatform {
    pub fn new() -> Self {
        Self {
            job: OnceLock::new(),
        }
    }

    /// `CreateJobObject` + `KILL_ON_JOB_CLOSE`, cached for the process life.
    fn job_handle(&self) -> Option<windows::Win32::Foundation::HANDLE> {
        let raw = *self.job.get_or_init(|| {
            use std::mem::zeroed;
            use windows::Win32::System::JobObjects::{
                CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            };
            unsafe {
                let Ok(job) = CreateJobObjectW(None, None) else {
                    return 0;
                };
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
                .is_ok();
                if !ok {
                    use windows::Win32::Foundation::CloseHandle;
                    let _ = CloseHandle(job);
                    return 0;
                }
                job.0 as isize
            }
        });
        (raw != 0).then_some(windows::Win32::Foundation::HANDLE(
            raw as *mut core::ffi::c_void,
        ))
    }
}

/// MoonClip-owned OBS runtime root: `%LOCALAPPDATA%\MoonClip\obs`. The
/// portable copy lives here (binary + `config/obs-studio`).
pub fn runtime_root() -> Result<PathBuf, String> {
    let base = dirs::data_local_dir()
        .map(|d| d.join("MoonClip").join("obs"))
        .ok_or_else(|| "cannot resolve %LOCALAPPDATA%".to_string())?;
    Ok(base)
}

/// Dir that will contain `obs-studio/` (what `write_obs_config` fills).
pub fn config_root() -> Result<PathBuf, String> {
    Ok(runtime_root()?.join("config"))
}

/// Root of a bundled OBS tree given its `bin/64bit/obs64.exe` path.
fn source_root(obs_bin: &Path) -> Result<PathBuf, String> {
    obs_bin
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .ok_or_else(|| format!("unexpected OBS layout: {}", obs_bin.display()))
}

/// Stage (once per bundled build) the writable portable copy.
fn stage_runtime(obs_bin: &Path) -> Result<PathBuf, String> {
    let root = runtime_root()?;
    let runtime_bin = root.join("bin").join("64bit").join("obs64.exe");
    let fingerprint = obs_build_fingerprint(obs_bin);
    let marker = root.join(".moonclip-source");
    if runtime_bin.exists() && marker_matches(&marker, &fingerprint) {
        return Ok(runtime_bin);
    }
    let src = source_root(obs_bin)?;
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
    // Portable mode marker: OBS writes config/ inside the copy.
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

impl ObsPlatform for WindowsPlatform {
    fn prepare_runtime(&self, bundled_bin: &Path) -> Result<ObsRuntime, String> {
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
            // Coexist with the user's own OBS instance.
            "--multi".into(),
            // Out of the way: tray + no shutdown/update prompts.
            "--minimize-to-tray".into(),
            "--disable-shutdown-check".into(),
            "--disable-updater".into(),
            "--only-bundled-plugins".into(),
        ]
    }

    fn working_dir(&self, obs_bin: &Path) -> Option<PathBuf> {
        obs_bin.parent().map(|p| p.to_path_buf())
    }

    fn configure(&self, cmd: &mut Command) {
        // CREATE_NO_WINDOW: never flash a console for OBS/obs-cmd children.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    fn adopt_child(&self, pid: u32) {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::JobObjects::AssignProcessToJobObject;
        use windows::Win32::System::Threading::{
            OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };
        let Some(job) = self.job_handle() else {
            return; // job creation failed: kill_on_drop + orphan sweep remain
        };
        unsafe {
            let Ok(proc) = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) else {
                return;
            };
            let ok = AssignProcessToJobObject(job, proc).is_ok();
            let _ = CloseHandle(proc);
            if !ok {
                eprintln!("[moonclip] could not bind OBS to the kill-on-close job");
            }
        }
    }

    fn kill_orphans(&self, obs_bin: &Path) {
        kill_orphan_process(obs_bin);
    }

    fn video_source(&self, monitor: &str, window: &str) -> (&'static str, serde_json::Value) {
        if !window.trim().is_empty() {
            // Window capture (WGC, no injection); opt-in for now.
            return (
                "window_capture",
                serde_json::json!({"window": window, "method": 2, "cursor": true}),
            );
        }
        // Modern `monitor_capture` (duplicator) takes the WinRT device
        // interface id under `monitor_id`. WGC (method 2) is the compositor
        // path: it captures HDR displays and coexists with other duplicators
        // (the DXGI method can return all-black when another app duplicates
        // the same output). `force_sdr` tonemaps HDR to our SDR pipeline.
        let id = if monitor.trim().is_empty() {
            r"\\.\DISPLAY1" // GDI fallback: OBS matches szDevice when the id misses
        } else {
            monitor.trim()
        };
        (
            "monitor_capture",
            serde_json::json!({
                "monitor_id": id,
                "method": 2,
                "capture_cursor": true,
                "force_sdr": true
            }),
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
            ("amd", "hevc") => Some("h265_texture_amf"),
            ("amd", "av1") => Some("av1_texture_amf"),
            ("intel", "h264") => Some("obs_qsv11_v2"),
            ("intel", "hevc") => Some("obs_qsv11_hevc"),
            ("intel", "av1") => Some("obs_qsv11_av1"),
            _ => None,
        }
    }
}

/// New engine wired to this platform.
pub fn new_engine() -> ObsEngine {
    ObsEngine::new(Box::new(WindowsPlatform::new()))
}

/// Kill our own leftover OBS processes (force-killed sessions never run Drop).
///
/// A process is ours when its image path matches OUR runtime binary AND its
/// parent no longer exists. A live parent (running engine, manual test) is
/// never touched; the user's own OBS path never matches.
fn kill_orphan_process(target: &Path) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    };
    let Ok(want) = std::fs::canonicalize(target) else {
        return;
    };
    let want = want.to_string_lossy().to_lowercase();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut live_pids = std::collections::HashSet::new();
        let mut procs: Vec<(u32, u32)> = Vec::new();
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                live_pids.insert(entry.th32ProcessID);
                procs.push((entry.th32ProcessID, entry.th32ParentProcessID));
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        for (pid, parent) in procs {
            if pid == std::process::id() || parent == 0 || live_pids.contains(&parent) {
                continue;
            }
            let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                continue;
            };
            let mut buf = [0u16; 512];
            let mut len = buf.len() as u32;
            let path = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                windows::core::PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
            .is_ok()
            .then(|| String::from_utf16_lossy(&buf[..len as usize]).to_lowercase());
            let _ = CloseHandle(handle);
            if path.as_deref() != Some(want.as_str()) {
                continue;
            }
            if let Ok(terminate) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                let killed = TerminateProcess(terminate, 1).is_ok();
                let _ = CloseHandle(terminate);
                if killed {
                    eprintln!("[moonclip] killed orphan OBS pid={pid}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> WindowsPlatform {
        WindowsPlatform::new()
    }

    #[test]
    fn encoder_ids_are_concrete() {
        assert_eq!(
            p().encoder_id("nvidia", "h264", "gpu"),
            Some("obs_nvenc_h264_tex")
        );
        assert_eq!(
            p().encoder_id("amd", "h264", "gpu"),
            Some("h264_texture_amf")
        );
        assert_eq!(
            p().encoder_id("intel", "hevc", "gpu"),
            Some("obs_qsv11_hevc")
        );
        assert_eq!(p().encoder_id("nvidia", "h264", "cpu"), Some("obs_x264"));
        assert_eq!(p().encoder_id("unknown", "h264", "gpu"), None);
    }

    #[test]
    fn video_source_never_game_capture() {
        let dev =
            r"\\?\DISPLAY#IPS2380#5&2772c986&0&UID4353#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
        let (id, settings) = p().video_source(dev, "");
        assert_eq!(id, "monitor_capture");
        assert_eq!(settings["monitor_id"], dev);
        assert_eq!(settings["method"], 2);
        assert_eq!(settings["force_sdr"], true);
        let (id, settings) = p().video_source("", "Game");
        assert_eq!(id, "window_capture");
        assert_eq!(settings["window"], "Game");
        assert_ne!(id, "game_capture");
        assert_ne!(id, super::super::super::obs::FORBIDDEN_SOURCE_ID);
    }

    #[test]
    fn launch_args_use_portable_and_coexist() {
        let args = p().obs_launch_args(Path::new("C:/mc/obs/config"), "MoonClip", "MoonClip");
        let s = args.join(" ");
        assert!(s.contains("--profile MoonClip"), "{s}");
        assert!(s.contains("--collection MoonClip"));
        assert!(s.contains("--multi"));
        assert!(s.contains("--minimize-to-tray"));
        assert!(
            !s.contains("--config-dir"),
            "portable mode must not rely on --config-dir"
        );
        assert!(
            !s.contains("--safe-mode"),
            "safe-mode would disable the websocket"
        );
    }

    #[test]
    fn runtime_lives_inside_moonclip_localappdata() {
        let root = runtime_root().unwrap();
        let s = root.to_string_lossy().to_lowercase();
        assert!(s.contains("moonclip"));
        assert!(!s.contains("appdata\\roaming"), "{s}");
        assert_eq!(
            config_root().unwrap(),
            root.join("config"),
            "config dir must be inside the portable copy"
        );
    }

    #[test]
    fn prepare_runtime_is_portable_with_marker() {
        // Pure path contract: config dir under the runtime root, portable flag
        // and --portable arg. (Staging itself is filesystem-heavy and covered
        // by os/obs.rs copy tests.)
        let root = runtime_root().unwrap();
        assert!(config_root().unwrap().starts_with(&root));
        let bin = PathBuf::from("C:/fake/bin/64bit/obs64.exe");
        assert_eq!(
            source_root(&bin).unwrap(),
            PathBuf::from("C:/fake"),
            "obs64.exe sits under <root>/bin/64bit"
        );
    }
}
