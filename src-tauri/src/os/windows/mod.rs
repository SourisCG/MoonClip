//! Windows backend assembly (ffmpeg `gfxcapture` WGC capture + WASAPI audio
//! via the `wasapi` crate). Same surface as os/linux so shared code never
//! branches on OS.

pub mod audio;
pub mod binary;
pub mod caps;
pub mod detector;
pub mod devices;
mod dsp;
mod encode;
mod engine;
mod mux;
pub mod open;
pub mod paths;
mod pts;
mod ring;
pub mod video;

pub use engine::WindowsCaptureEngine as Engine;

pub fn backend_name() -> &'static str {
    "gfxcapture"
}

/// Raise the calling thread to TIME_CRITICAL. The engine's pipe drains must
/// run even while a focused game owns the CPU: if a 64 KB pipe stays unread,
/// ffmpeg blocks mid-capture (the "one frame every few seconds" symptom).
pub fn boost_current_thread() {
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
    };
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
    }
}

/// Kill our own ffmpeg children left behind by an abrupt app exit.
///
/// `kill_on_drop`/`Drop` only run on a graceful shutdown: a force-killed host
/// (taskkill, Ctrl+C in the dev loop, a crash) leaves the capture + encoder
/// child alive holding a WGC session and an NVENC session, and every iteration
/// stacks another one. On the desktop they barely show; under a game they
/// compete for the GPU and NVENC — exactly the "only in-game" degradation.
///
/// A process is ours when its image path matches the engine's ffmpeg AND its
/// parent no longer exists. A live parent (running engine, `cargo test,` a
/// manual A/B) is never touched; a PATH-resolved `ffmpeg` is never guessed.
pub fn kill_orphan_ffmpeg(ffmpeg: &std::path::Path) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    };
    let Ok(want) = std::fs::canonicalize(ffmpeg) else {
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
            let path = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len)
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
                    eprintln!("[moonclip] killed orphan ffmpeg pid={pid}");
                }
            }
        }
    }
}

/// Process-level hardening for the recorder host: above-normal CPU class and
/// power throttling (EcoQoS) disabled, so Windows cannot park our drain
/// threads (or the encoder child) while the game runs. Windows (WebView2)
/// needs no WebKitGTK/Wayland workarounds.
pub fn prepare_environment() {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Threading::{
        GetCurrentProcess, ProcessPowerThrottling, SetPriorityClass, SetProcessInformation,
        ABOVE_NORMAL_PRIORITY_CLASS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
    };
    unsafe {
        let h: HANDLE = GetCurrentProcess();
        let _ = SetPriorityClass(h, ABOVE_NORMAL_PRIORITY_CLASS);
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: 0, // execution speed throttling disabled
        };
        let _ = SetProcessInformation(
            h,
            ProcessPowerThrottling,
            &state as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
    }
}
