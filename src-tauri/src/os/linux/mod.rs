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

/// How the DMA-BUF renderer should be configured for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DmabufPlan {
    /// Accelerated, plain.
    Plain,
    /// Accelerated with NVIDIA explicit sync off (`__NV_DISABLE_EXPLICIT_SYNC`).
    ExplicitSync,
    /// Software fallback (`WEBKIT_DISABLE_DMABUF_RENDERER=1`).
    Disabled,
}

/// Pure state machine: given the persisted state, what to do this run.
/// Returns the state to persist (if it changed) plus the renderer plan.
fn dmabuf_plan(state: &str) -> (Option<&'static str>, DmabufPlan) {
    match state.trim() {
        // Absent: first run on this machine, try the accelerated path.
        "" => (Some("pending"), DmabufPlan::Plain),
        // The previous accelerated start crashed before painting. Retry with
        // NVIDIA explicit sync off: same accelerated path, but without the
        // sync handshake that makes some drivers abort the Wayland connection
        // (`Gdk-Message: Error 71`). Harmless on non-NVIDIA drivers.
        "pending" => (Some("pending-explicit"), DmabufPlan::ExplicitSync),
        // That crashed too: this machine cannot use the renderer.
        "pending-explicit" => (Some("disabled"), DmabufPlan::Disabled),
        // A previous run proved the accelerated path works; keep it.
        "ok" => (None, DmabufPlan::Plain),
        "ok-explicit" => (None, DmabufPlan::ExplicitSync),
        "disabled" => (None, DmabufPlan::Disabled),
        // Unknown state: restart the escalation from the top.
        _ => (Some("pending"), DmabufPlan::Plain),
    }
}

/// State to persist after a successful first paint.
fn dmabuf_state_after_paint(state: &str) -> Option<&'static str> {
    match state.trim() {
        "pending" => Some("ok"),
        "pending-explicit" => Some("ok-explicit"),
        _ => None,
    }
}

/// WebKitGTK's DMA-BUF renderer is the accelerated Wayland path. Older builds
/// disabled it unconditionally (a first-paint crash on some drivers), which
/// forces software compositing and makes video/UI sluggish on every GPU.
/// Now it is vendor-neutral and self-healing:
///   - `MOONCLIP_DMABUF=0/1` forces the choice (state untouched).
///   - otherwise the renderer stays ON; a `pending` marker is written before
///     the window exists and the frontend flips it to `ok` on first paint.
///   - if a start finds `pending` still there, the previous run crashed
///     before painting: retry with `__NV_DISABLE_EXPLICIT_SYNC=1`; if that
///     also fails, fall back to software compositing for good.
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
    let (persist, plan) = dmabuf_plan(&current);
    if let Some(next) = persist {
        if next == "pending" {
            if let Some(dir) = app_data_dir() {
                let _ = std::fs::create_dir_all(&dir);
            }
        }
        let _ = std::fs::write(&state, next);
    }
    match plan {
        DmabufPlan::Plain => {}
        DmabufPlan::ExplicitSync => {
            if persist == Some("pending-explicit") {
                eprintln!(
                    "[moonclip] previous start never painted with DMA-BUF; retrying with explicit sync off"
                );
            } else {
                eprintln!("[moonclip] DMA-BUF with explicit sync off (proven on this machine)");
            }
            std::env::set_var("__NV_DISABLE_EXPLICIT_SYNC", "1");
        }
        DmabufPlan::Disabled => {
            if persist == Some("disabled") {
                eprintln!(
                    "[moonclip] DMA-BUF crashed with and without explicit sync; using software compositing"
                );
            } else {
                eprintln!("[moonclip] DMA-BUF renderer disabled; using software compositing");
            }
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
    }
}

/// Called once the frontend mounted (real first paint).
pub fn mark_first_paint() {
    if std::env::var_os("WAYLAND_DISPLAY").is_none()
        || std::env::var_os("MOONCLIP_DMABUF").is_some()
        || std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_some()
    {
        return;
    }
    if let Some(state) = dmabuf_state_path() {
        let current = std::fs::read_to_string(&state).unwrap_or_default();
        if let Some(next) = dmabuf_state_after_paint(&current) {
            let _ = std::fs::write(&state, next);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_run_tries_the_accelerated_path() {
        assert_eq!(dmabuf_plan(""), (Some("pending"), DmabufPlan::Plain));
    }

    #[test]
    fn crash_escalates_to_explicit_sync_then_software() {
        assert_eq!(
            dmabuf_plan("pending"),
            (Some("pending-explicit"), DmabufPlan::ExplicitSync)
        );
        assert_eq!(
            dmabuf_plan("pending-explicit"),
            (Some("disabled"), DmabufPlan::Disabled)
        );
    }

    #[test]
    fn proven_states_are_reused_without_rewriting() {
        assert_eq!(dmabuf_plan("ok"), (None, DmabufPlan::Plain));
        assert_eq!(dmabuf_plan("ok-explicit"), (None, DmabufPlan::ExplicitSync));
        assert_eq!(dmabuf_plan("disabled"), (None, DmabufPlan::Disabled));
    }

    #[test]
    fn unknown_state_restarts_the_escalation() {
        assert_eq!(
            dmabuf_plan("garbage"),
            (Some("pending"), DmabufPlan::Plain)
        );
    }

    #[test]
    fn first_paint_promotes_pending_states_only() {
        assert_eq!(dmabuf_state_after_paint("pending"), Some("ok"));
        assert_eq!(
            dmabuf_state_after_paint("pending-explicit"),
            Some("ok-explicit")
        );
        assert_eq!(dmabuf_state_after_paint("ok"), None);
        assert_eq!(dmabuf_state_after_paint("ok-explicit"), None);
        assert_eq!(dmabuf_state_after_paint("disabled"), None);
    }
}
