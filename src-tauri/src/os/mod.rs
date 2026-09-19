//! OS isolation (OBS-style): ALL platform-specific code lives under os/.
//!
//! RULE (enforced): no `cfg(target_os)` and no mention of Linux/Windows APIs
//! outside this directory. Backend selection happens ONLY here via re-export.
//! Shared code (commands, state, editor, storage, cue) talks to `crate::os`
//! and never knows which OS it runs on.

use std::path::PathBuf;
use tauri::AppHandle;

mod api;
pub mod encoder_options;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod obs;
#[cfg(target_os = "windows")]
pub mod windows;
// Non-desktop dev fallback (docs builds, IDE checks): Linux backend.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub mod linux;

pub use api::{AudioDevice, CaptureConfig, CaptureEngine, CustomEncoder, CustomVideo};

#[cfg(target_os = "linux")]
pub use linux::{backend_name, devices, open, paths, prepare_environment, video, Engine};
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub use linux::{backend_name, devices, open, paths, prepare_environment, video, Engine};
#[cfg(target_os = "windows")]
pub use windows::{backend_name, devices, open, paths, prepare_environment, video, Engine};

/// New embedded-OBS engine for this OS.
#[cfg(target_os = "linux")]
pub fn new_engine() -> Engine {
    linux::new_engine()
}
#[cfg(target_os = "windows")]
pub fn new_engine() -> Engine {
    windows::new_engine()
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn new_engine() -> Engine {
    linux::new_engine()
}

/// Resolved embedded OBS binary + provenance (bundled/env).
#[cfg(target_os = "linux")]
pub fn resolve_obs(app: &AppHandle) -> Result<(PathBuf, &'static str), String> {
    linux::binary::resolve_obs(app)
}
#[cfg(target_os = "windows")]
pub fn resolve_obs(app: &AppHandle) -> Result<(PathBuf, &'static str), String> {
    windows::binary::resolve_obs(app)
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn resolve_obs(app: &AppHandle) -> Result<(PathBuf, &'static str), String> {
    linux::binary::resolve_obs(app)
}

/// Resolved embedded obs-cmd binary + provenance (bundled/env).
#[cfg(target_os = "linux")]
pub fn resolve_obscmd(app: &AppHandle) -> Result<(PathBuf, &'static str), String> {
    linux::binary::resolve_obscmd(app)
}
#[cfg(target_os = "windows")]
pub fn resolve_obscmd(app: &AppHandle) -> Result<(PathBuf, &'static str), String> {
    windows::binary::resolve_obscmd(app)
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn resolve_obscmd(app: &AppHandle) -> Result<(PathBuf, &'static str), String> {
    linux::binary::resolve_obscmd(app)
}

/// MoonClip-owned OBS config root (never the user's OBS config).
#[cfg(target_os = "linux")]
pub fn obs_config_root() -> Result<PathBuf, String> {
    linux::obs::config_root()
}
#[cfg(target_os = "windows")]
pub fn obs_config_root() -> Result<PathBuf, String> {
    windows::obs::config_root()
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn obs_config_root() -> Result<PathBuf, String> {
    linux::obs::config_root()
}

/// Encoder ids compiled into this platform's OBS build (Custom picker).
#[cfg(target_os = "linux")]
pub fn encoder_catalog() -> &'static [encoder_options::EncoderEntry] {
    linux::obs::encoder_catalog()
}
#[cfg(target_os = "windows")]
pub fn encoder_catalog() -> &'static [encoder_options::EncoderEntry] {
    windows::obs::encoder_catalog()
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn encoder_catalog() -> &'static [encoder_options::EncoderEntry] {
    linux::obs::encoder_catalog()
}

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
