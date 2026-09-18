//! Windows capture-source helpers: ffmpeg fallback resolution, live-scaling
//! flag, foreground-window lookup, the WGC/DXGI filter graph and the startup
//! fallback decision. Hardware discovery (vendor, monitors, codec offer)
//! lives in `detector` and is re-exported here so shared code keeps one
//! import surface (`os::video::*`).

use std::path::PathBuf;

pub use super::detector::{
    capture_encoder_name, list_monitors, offered_codecs, resolve_monitor, transcode_encoder,
    vendor,
};

/// What to capture and at what maximum rate. Monitor selection is by native
/// HMONITOR (cross-process safe, validated) so the Rust-side monitor list is
/// authoritative — no index-order guessing. `max_fps` is the capture cap
/// (`capture_max_fps`: 2x the output rate, clamped to the panel refresh —
/// `MOONCLIP_CAPTURE_MAX_FPS` overrides). Window capture is opt-in
/// (`MOONCLIP_CAPTURE_SOURCE=window` or `MOONCLIP_CAPTURE_WINDOW_EXE=regex`).
#[derive(Debug, Clone)]
pub struct CaptureSource<'a> {
    /// `gfxcapture` (WGC: monitors and windows) or `ddagrab` (DDA, monitor).
    pub kind: &'a str,
    pub hmonitor: isize,
    pub monitor_idx: u32,
    /// WGC window selectors; `hwnd` wins over `window_exe`.
    pub hwnd: Option<u64>,
    pub window_exe: Option<&'a str>,
    pub max_fps: u32,
}

impl CaptureSource<'_> {
    fn is_window(&self) -> bool {
        self.kind == "gfxcapture" && (self.hwnd.is_some() || self.window_exe.is_some())
    }

    pub fn describe(&self) -> String {
        if let Some(h) = self.hwnd {
            format!("window hwnd={h}")
        } else if let Some(exe) = self.window_exe {
            format!("window exe~'{exe}'")
        } else if self.kind == "ddagrab" {
            format!("ddagrab output {}", self.monitor_idx)
        } else {
            format!("monitor hmonitor={}", self.hmonitor)
        }
    }
}

/// Startup source decision: WGC lives in DWM, so a real legacy exclusive
/// fullscreen (OpenGL/Vulkan) bypasses it and delivers no frames or an
/// all-black picture. Desktop Duplication owns the display in that case.
/// Returns the source to relaunch with, or `None` to keep the current one.
pub fn source_fallback(kind: &str, frames_seen: bool, black: Option<bool>) -> Option<&'static str> {
    if kind != "gfxcapture" {
        return None;
    }
    if !frames_seen || black == Some(true) {
        Some("ddagrab")
    } else {
        None
    }
}

/// True when the GPU chain can take D3D11 frames straight from `gfxcapture`
/// (validated: NVENC). AMD/Intel run the hwdownload fallback until their
/// zero-copy chain is validated on real hardware.
fn gpu_zero_copy(vendor: &str, codec: &str) -> bool {
    vendor == "nvidia" && codec != "x264"
}

/// Even output width for a target height, preserving the source aspect.
pub fn scaled_width(mw: u32, mh: u32, out_height: u32) -> u32 {
    if mh == 0 {
        return mw;
    }
    let w = ((mw as f64 * out_height as f64 / mh as f64).round() as u32).max(2);
    w & !1
}

/// Capture rate cap for WGC. Two candidates per CFR slot (2x the output rate,
/// min 90; fallback 120 when the refresh is unknown) so a suppressed WGC
/// present becomes a dropped excess frame instead of a duplicate. Never the
/// full 164 Hz refresh: that triples the WGC copy/filter work under a game
/// for frames the CFR filter then throws away. `MOONCLIP_CAPTURE_MAX_FPS`
/// overrides it (clamped to >= fps).
pub fn capture_max_fps(refresh_hz: u32, fps: u32) -> u32 {
    let headroom = (fps * 2).max(90);
    let cap = if refresh_hz >= 30 {
        refresh_hz.min(headroom)
    } else {
        (fps * 2).max(120)
    };
    cap.max(fps)
}

/// The capture filtergraph. Scaling happens inside the filter on the GPU
/// (bicubic) for the zero-copy chain; the download fallback uses lanczos.
/// `showinfo` runs pre-encoder and feeds the PTS<->QPC clock anchor.
pub fn capture_filter(
    vendor: &str,
    codec: &str,
    src: &CaptureSource<'_>,
    mw: u32,
    mh: u32,
    out_height: u32,
    fps: u32,
) -> String {
    let scale = out_height > 0 && out_height < mh;
    // ddagrab has no working GPU resizer here (`scale_d3d11` refuses its
    // frames); window capture scales through the download path too (its
    // canvas is the window, not the monitor).
    let zero = gpu_zero_copy(vendor, codec) && !(src.kind == "ddagrab" && scale) && !(src.is_window() && scale);
    let mut f = if src.kind == "ddagrab" {
        // `dup_frames=0`: deliver on change only. Duplicates are ffmpeg's job
        // (`-r fps -fps_mode cfr`); paying the OS to fabricate them here would
        // multiply the capture cost for nothing.
        format!(
            "ddagrab=output_idx={}:framerate={}:dup_frames=0",
            src.monitor_idx, src.max_fps
        )
    } else if let Some(h) = src.hwnd {
        format!(
            "gfxcapture=hwnd={h}:max_framerate={}:capture_cursor=1",
            src.max_fps
        )
    } else if let Some(exe) = src.window_exe {
        format!(
            "gfxcapture=window_exe='{exe}':max_framerate={}:capture_cursor=1",
            src.max_fps
        )
    } else {
        format!(
            "gfxcapture=hmonitor={}:max_framerate={}:capture_cursor=1",
            src.hmonitor, src.max_fps
        )
    };
    if zero {
        if scale {
            let tw = scaled_width(mw, mh, out_height);
            f.push_str(&format!(
                ":width={tw}:height={out_height}:resize_mode=scale_aspect:scale_mode=bicubic"
            ));
        }
    } else {
        f.push_str(",hwdownload,format=bgra");
        if scale {
            f.push_str(&format!(",scale=-2:{out_height}:flags=lanczos"));
        }
        f.push_str(",format=yuv420p");
    }
    // showinfo runs pre-encoder and logs each frame's PTS (100 ns) on stderr:
    // the engine uses the log arrival QPC as the PTS<->QPC clock anchor, so
    // encoder lookahead can never shift A/V (see `note_clock_sample`).
    // `MOONCLIP_NO_SHOWINFO=1` drops it for load A/B tests (A/V falls back to
    // the coarser PES-arrival calibration).
    if std::env::var("MOONCLIP_NO_SHOWINFO").as_deref() != Ok("1") {
        f.push_str(",showinfo");
    }
    f.push_str("[out]");
    let _ = fps;
    f
}

/// Last-resort ffmpeg for direct engine use: explicit override, the bundled
/// sidecar checked into `src-tauri/binaries/` (dev/tests), else PATH.
/// Normal startup passes the resolved bundled sidecar via `ffmpeg_bin`.
pub fn capture_ffmpeg() -> PathBuf {
    if let Ok(path) = std::env::var("MOONCLIP_FFMPEG") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return p;
        }
    }
    let triple = crate::sidecar::host_triple();
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(triple)
        .join(format!("ffmpeg-{triple}.exe"));
    if dev.exists() {
        return dev;
    }
    PathBuf::from("ffmpeg")
}

/// True when this backend scales the capture on the GPU live (ffmpeg's
/// `gfxcapture` does), so saves are copy-only and the settings plan must
/// capture at the delivered height instead of buffering the source.
pub fn scales_live() -> bool {
    true
}

/// Foreground window handle, unless it belongs to this process (clicking
/// Start would otherwise capture our own UI). Used by the opt-in window
/// capture mode (`MOONCLIP_CAPTURE_SOURCE=window`).
pub fn foreground_window() -> Option<isize> {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == GetCurrentProcessId() {
            return None;
        }
        Some(hwnd.0 as isize)
    }
}

#[cfg(test)]
mod tests {
    use super::{capture_filter, capture_max_fps, source_fallback, CaptureSource};

    #[test]
    fn scales_live_on_windows() {
        assert!(super::scales_live());
    }

    #[test]
    fn capture_cap_keeps_two_candidates_per_slot() {
        // 164 Hz monitor, 60 fps output -> 120, not the refresh and not 60:
        // two source frames per CFR slot so a suppressed present drops instead
        // of duplicating.
        assert_eq!(capture_max_fps(164, 60), 120);
        assert_eq!(capture_max_fps(144, 60), 120);
        // A 60 Hz panel is already the cap (never above it).
        assert_eq!(capture_max_fps(60, 60), 60);
        // 30 fps output still gets the min-90 headroom.
        assert_eq!(capture_max_fps(164, 30), 90);
        // Unknown refresh falls back to >= 2x.
        assert_eq!(capture_max_fps(0, 60), 120);
        assert_eq!(capture_max_fps(24, 30), 120);
    }

    #[test]
    fn fallback_only_from_wgc() {
        assert_eq!(source_fallback("gfxcapture", false, None), Some("ddagrab"));
        assert_eq!(source_fallback("gfxcapture", true, Some(true)), Some("ddagrab"));
        assert_eq!(source_fallback("gfxcapture", true, Some(false)), None);
        assert_eq!(source_fallback("gfxcapture", true, None), None);
        assert_eq!(source_fallback("ddagrab", false, Some(true)), None);
    }

    #[test]
    fn capture_filter_shapes() {
        // NVENC zero-copy; max fps is the 2x-headroom cap (`capture_max_fps`).
        let monitor = CaptureSource {
            kind: "gfxcapture",
            hmonitor: 65673,
            monitor_idx: 0,
            hwnd: None,
            window_exe: None,
            max_fps: 165,
        };
        let f = capture_filter("nvidia", "h264", &monitor, 1920, 1080, 720, 60);
        assert!(f.starts_with("gfxcapture=hmonitor=65673:max_framerate=165"), "{f}");
        assert!(f.contains("width=1280:height=720:resize_mode=scale_aspect:scale_mode=bicubic"), "{f}");
        assert!(!f.contains("hwdownload"), "{f}");
        assert!(f.ends_with("[out]"));
        // Fallback path downloads + lanczos + yuv420p.
        let amd = CaptureSource { kind: "gfxcapture", hmonitor: 1, monitor_idx: 0, hwnd: None, window_exe: None, max_fps: 120 };
        let f = capture_filter("amd", "h264", &amd, 1920, 1080, 720, 30);
        assert!(f.contains("hwdownload,format=bgra"), "{f}");
        assert!(f.contains("scale=-2:720:flags=lanczos"), "{f}");
        assert!(f.contains("format=yuv420p"), "{f}");
        // x264 always downloads even on NVIDIA.
        let big = CaptureSource { kind: "gfxcapture", hmonitor: 1, monitor_idx: 0, hwnd: None, window_exe: None, max_fps: 165 };
        let f = capture_filter("nvidia", "x264", &big, 2560, 1440, 0, 60);
        assert!(f.contains("hwdownload"), "{f}");
        assert!(!f.contains("scale="), "{f}");
        // Desktop Duplication fallback by output index, duplicates off.
        let dda = CaptureSource { kind: "ddagrab", hmonitor: 1, monitor_idx: 2, hwnd: None, window_exe: None, max_fps: 165 };
        let f = capture_filter("nvidia", "h264", &dda, 1920, 1080, 0, 60);
        assert!(f.starts_with("ddagrab=output_idx=2:framerate=165:dup_frames=0"), "{f}");
        assert!(!f.contains("hmonitor"), "{f}");
        // ddagrab has no GPU resizer: scaling goes through the download path.
        let f = capture_filter("nvidia", "h264", &dda, 1920, 1080, 720, 60);
        assert!(f.contains("hwdownload,format=bgra"), "{f}");
        assert!(f.contains("scale=-2:720:flags=lanczos"), "{f}");
        // Window capture: hwnd wins; scaling falls to the download path.
        let win = CaptureSource { kind: "gfxcapture", hmonitor: 1, monitor_idx: 0, hwnd: Some(4242), window_exe: None, max_fps: 144 };
        let f = capture_filter("nvidia", "h264", &win, 1920, 1080, 720, 60);
        assert!(f.starts_with("gfxcapture=hwnd=4242:max_framerate=144"), "{f}");
        assert!(f.contains("hwdownload"), "{f}");
        let win_exe = CaptureSource { kind: "gfxcapture", hmonitor: 1, monitor_idx: 0, hwnd: None, window_exe: Some("^cod.exe$"), max_fps: 144 };
        let f = capture_filter("nvidia", "h264", &win_exe, 1920, 1080, 0, 60);
        assert!(f.starts_with("gfxcapture=window_exe='^cod.exe$':max_framerate=144"), "{f}");
        assert!(!f.contains("hwdownload"), "{f}");
    }
}
