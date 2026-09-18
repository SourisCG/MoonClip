//! OS isolation (OBS-style): ALL platform-specific code lives under os/.
//!
//! RULE (enforced): no `cfg(target_os)` and no mention of Linux/Windows APIs
//! outside this directory. Backend selection happens ONLY here via re-export.
//! Shared code (commands, state, editor, storage, cue) talks to `crate::os`
//! and never knows which OS it runs on.

mod api;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "windows")]
pub mod windows;
// Non-desktop dev fallback (docs builds, IDE checks): Linux backend.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub mod linux;

pub use api::{AudioDevice, CaptureConfig, CaptureEngine, SavePlan, TranscodeEncoder};

#[cfg(target_os = "linux")]
pub use linux::{
    audio, backend_name, binary, caps, devices, open, paths, prepare_environment, video, Engine,
};
#[cfg(target_os = "windows")]
pub use windows::{
    audio, backend_name, binary, caps, devices, open, paths, prepare_environment, video, Engine,
};
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub use linux::{
    audio, backend_name, binary, caps, devices, open, paths, prepare_environment, video, Engine,
};

/// Free physical memory in MB when the platform can report it (used by the
/// settings UI to color the buffer-size warning; `None` hides the check).
/// Linux intentionally returns `None` until the Linux owner wires it.
#[cfg(target_os = "windows")]
pub fn memory_free_mb() -> Option<u64> {
    use ::windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    ok.is_ok().then_some(status.ullAvailPhys / (1024 * 1024))
}

#[cfg(not(target_os = "windows"))]
pub fn memory_free_mb() -> Option<u64> {
    None
}
