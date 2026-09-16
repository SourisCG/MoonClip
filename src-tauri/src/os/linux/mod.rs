//! Linux backend assembly. Everything Linux-only is reachable via this module.

pub mod audio;
pub mod binary;
pub mod caps;
pub mod devices;
pub mod open;
pub mod paths;
pub mod video;
mod gsr;

pub use gsr::LinuxGsrEngine as Engine;

pub fn backend_name() -> &'static str {
    "gpu-screen-recorder"
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
