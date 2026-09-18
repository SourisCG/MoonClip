//! Linux backend assembly: shared embedded-OBS engine bound to the Linux
//! platform (PipeWire portal capture, PulseAudio devices, XDG-isolated OBS
//! config).

pub mod binary;
pub mod devices;
pub mod obs;
pub mod open;
pub mod paths;
pub mod video;

pub use super::obs::ObsEngine as Engine;

/// New engine wired to the Linux platform.
pub fn new_engine() -> Engine {
    obs::new_engine()
}

pub fn backend_name() -> &'static str {
    "obs"
}

/// WebKitGTK crashes at first paint on Wayland (Gdk `Error 71`) unless the
/// DMA-BUF renderer is disabled. Cargo dev runs get this from
/// `.cargo/config.toml`; packaged builds have no such injection, so apply it
/// here before any window exists (respects a user-set override).
pub fn prepare_environment() {
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none()
    {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
}
