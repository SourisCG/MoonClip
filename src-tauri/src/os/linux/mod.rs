//! Linux backend assembly: shared embedded-OBS engine bound to the Linux
//! platform (PipeWire portal capture, PulseAudio devices, XDG-isolated OBS
//! config).

pub mod audio;
pub mod binary;
pub mod devices;
pub mod engine;
pub mod open;
pub mod paths;
pub mod portal;
pub mod winlist;
pub mod video;

pub use super::shared::engine::ObsEngine as Engine;

/// New engine wired to the Linux platform.
pub fn new_engine() -> Engine {
    engine::new_engine()
}

pub fn backend_name() -> &'static str {
    "engine"
}

/// Free physical memory in MB (`MemAvailable`), used by the settings UI to
/// color the buffer-size warning.
pub fn memory_free_mb() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb = text
        .lines()
        .find_map(|l| l.strip_prefix("MemAvailable:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    Some(kb / 1024)
}

/// App data dir matching Tauri's `app_data_dir()` on Linux.
fn app_data_dir() -> Option<std::path::PathBuf> {
    dirs::data_dir().map(|d| d.join("dev.souriscg.moonclip"))
}

fn dmabuf_state_path() -> Option<std::path::PathBuf> {
    app_data_dir().map(|d| d.join("dmabuf.state"))
}

/// WebKitGTK's DMA-BUF renderer is the accelerated Wayland path. Older builds
/// disabled it unconditionally (a first-paint crash on some drivers), which
/// forces software compositing and makes video/UI sluggish on every GPU.
/// Now it is vendor-neutral and self-healing:
///   - `MOONCLIP_DMABUF=0/1` forces the choice.
///   - otherwise the renderer stays ON; a `pending` marker is written before
///     the window exists and the frontend flips it to `ok` on first paint.
///   - if a start finds `pending` still there, the previous run crashed
///     before painting: disable the renderer from then on.
pub fn prepare_environment() {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return;
    }
    if let Some(force) = std::env::var_os("MOONCLIP_DMABUF") {
        if force == "0" {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
        return;
    }
    let Some(state) = dmabuf_state_path() else {
        return;
    };
    let current = std::fs::read_to_string(&state).unwrap_or_default();
    match current.trim() {
        "disabled" => {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
        "pending" => {
            eprintln!(
                "[moonclip] previous start never painted with DMA-BUF; using the software fallback"
            );
            let _ = std::fs::write(&state, "disabled");
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
        "ok" => {}
        _ => {
            if let Some(dir) = app_data_dir() {
                let _ = std::fs::create_dir_all(&dir);
            }
            let _ = std::fs::write(&state, "pending");
        }
    }
}

/// Called once the frontend mounted (real first paint).
pub fn mark_first_paint() {
    if std::env::var_os("WAYLAND_DISPLAY").is_none()
        || std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_some()
    {
        return;
    }
    if let Some(state) = dmabuf_state_path() {
        if std::fs::read_to_string(&state)
            .map(|s| s.trim() == "pending")
            .unwrap_or(false)
        {
            let _ = std::fs::write(&state, "ok");
        }
    }
}

/// Is the accelerated renderer disabled for this run?
pub fn software_compositing() -> bool {
    std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_some()
}

/// OBS source id for a registered input kind on Linux.
pub fn input_source_id(kind: &str) -> &'static str {
    use crate::os::shared::engine::ObsPlatform;
    let window = if kind == "screen" { "" } else { "x" };
    engine::LinuxPlatform.video_source("", window).0
}

/// Settings for the screen input (the Wayland portal picks the monitor).
pub fn screen_input_settings(monitor: &str) -> serde_json::Value {
    use crate::os::shared::engine::ObsPlatform;
    engine::LinuxPlatform.video_source(monitor, "").1
}
