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
