//! Windows backend assembly. Full WGC + WASAPI implementation happens
//! on the Windows test trip (see docs/PROGRESS.md cross-platform gate).
//! Same surface as os/linux so shared code never branches on OS.

pub mod audio;
pub mod binary;
pub mod caps;
pub mod devices;
pub mod open;
pub mod paths;
pub mod video;
mod wgc;

pub use wgc::WindowsCaptureEngine as Engine;

pub fn backend_name() -> &'static str {
    "windows-capture (stub)"
}

/// Windows (WebView2) needs no WebKitGTK/Wayland workarounds.
pub fn prepare_environment() {}
