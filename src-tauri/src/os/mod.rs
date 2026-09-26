//! OS isolation (OBS-style): ALL platform-specific code lives under os/.
//!
//! RULE (enforced): no `cfg(target_os)` and no mention of Linux/Windows APIs
//! outside this directory. Backend selection happens ONLY here via re-export.
//! Shared code (commands, state, editor, storage, cue) talks to `crate::os`
//! and never knows which OS it runs on.

use std::path::PathBuf;
use tauri::AppHandle;

mod api;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod shared;
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

/// MoonClip-owned OBS config root (never the user's OBS config).
#[cfg(target_os = "linux")]
pub fn obs_config_root() -> Result<PathBuf, String> {
    linux::engine::config_root()
}
#[cfg(target_os = "windows")]
pub fn obs_config_root() -> Result<PathBuf, String> {
    windows::engine::config_root()
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn obs_config_root() -> Result<PathBuf, String> {
    linux::engine::config_root()
}

/// Name of the engine's config tree inside `obs_config_root` (the patched
/// Linux build is neutral; the Windows prebuilt keeps `obs-studio`).
#[cfg(target_os = "linux")]
pub fn engine_config_dir() -> &'static str {
    linux::engine::CONFIG_DIR_NAME
}
#[cfg(target_os = "windows")]
pub fn engine_config_dir() -> &'static str {
    windows::engine::CONFIG_DIR_NAME
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn engine_config_dir() -> &'static str {
    linux::engine::CONFIG_DIR_NAME
}

/// Encoder ids compiled into this platform's OBS build (Custom picker).
#[cfg(target_os = "linux")]
pub fn encoder_catalog() -> &'static [shared::encoder_options::EncoderEntry] {
    linux::engine::encoder_catalog()
}
#[cfg(target_os = "windows")]
pub fn encoder_catalog() -> &'static [shared::encoder_options::EncoderEntry] {
    windows::engine::encoder_catalog()
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn encoder_catalog() -> &'static [shared::encoder_options::EncoderEntry] {
    linux::engine::encoder_catalog()
}

/// Directory where resolved game icons are cached as PNGs.
pub fn game_icons_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("MoonClip").join("icons"))
}

/// Best available icon source for a resolved game (Linux artwork sources;
/// Windows wires `SHGetFileInfo` in its own step).
#[cfg(target_os = "linux")]
pub fn find_game_icon(r: &shared::detect::ResolvedCandidate) -> Option<PathBuf> {
    linux::detect::find_icon(r, &linux::detect::default_icon_roots())
}
#[cfg(target_os = "windows")]
pub fn find_game_icon(r: &shared::detect::ResolvedCandidate) -> Option<PathBuf> {
    windows::detect::find_icon(r)
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn find_game_icon(_r: &shared::detect::ResolvedCandidate) -> Option<PathBuf> {
    None
}

/// Raw running game candidates (read-only scan). Windows wires its own
/// scanner in the Windows step.
#[cfg(target_os = "linux")]
pub fn detect_candidates() -> Vec<shared::detect::CandidateProcess> {
    linux::detect::scan()
}
#[cfg(target_os = "windows")]
pub fn detect_candidates() -> Vec<shared::detect::CandidateProcess> {
    windows::detect::scan()
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn detect_candidates() -> Vec<shared::detect::CandidateProcess> {
    Vec::new()
}

/// Resolve candidates against cached manifest lookups (Steam/Heroic/Prism).
#[cfg(target_os = "linux")]
pub fn resolve_candidates(
    cands: Vec<shared::detect::CandidateProcess>,
) -> Vec<shared::detect::ResolvedCandidate> {
    linux::detect::resolve_all(cands)
}
#[cfg(target_os = "windows")]
pub fn resolve_candidates(
    cands: Vec<shared::detect::CandidateProcess>,
) -> Vec<shared::detect::ResolvedCandidate> {
    windows::detect::resolve_all(cands)
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn resolve_candidates(
    _cands: Vec<shared::detect::CandidateProcess>,
) -> Vec<shared::detect::ResolvedCandidate> {
    Vec::new()
}

/// Free physical memory in MB when the platform can report it (used by the
/// settings UI to color the buffer-size warning; `None` hides the check).
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

#[cfg(target_os = "linux")]
pub fn memory_free_mb() -> Option<u64> {
    linux::memory_free_mb()
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn memory_free_mb() -> Option<u64> {
    None
}
