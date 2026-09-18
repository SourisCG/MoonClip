//! Windows hardware discovery: DXGI adapters (vendor), monitors with native
//! HMONITOR handles + real refresh, and the codec/encoder offer probed with a
//! live micro-encode against the SHIPPED ffmpeg (never an incidental PATH
//! copy). OS floor: Windows 10 1903+ (WGC, used by ffmpeg's `gfxcapture`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE,
};
use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};

use super::super::TranscodeEncoder;

/// PCI vendor id → our vendor slug.
fn vendor_from_pci_id(id: u32) -> &'static str {
    match id {
        0x10DE => "nvidia",
        // 0x1002 = AMD GPUs, 0x1022 = AMD iGPUs (same driver stack).
        0x1002 | 0x1022 => "amd",
        0x8086 => "intel",
        _ => "unknown",
    }
}

/// Best hardware adapter: `(vendor, dedicated VRAM bytes)`. Prefers the
/// adapter with the most VRAM so a dGPU wins over an iGPU. Skips the
/// Microsoft Basic Render Driver (software).
fn pick_adapter() -> Option<(String, usize)> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut best: Option<(String, usize)> = None;
        let mut i = 0u32;
        loop {
            let adapter = match factory.EnumAdapters1(i) {
                Ok(a) => a,
                Err(_) => break,
            };
            i += 1;
            let Ok(desc) = adapter.GetDesc1() else {
                continue;
            };
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            let vendor = vendor_from_pci_id(desc.VendorId).to_string();
            let vram = desc.DedicatedVideoMemory;
            let better = match &best {
                None => true,
                Some((_, m)) => vram > *m,
            };
            if better {
                best = Some((vendor, vram));
            }
        }
        best
    }
}

/// GPU vendor, lowercase (`nvidia`/`amd`/`intel`/`unknown`), from DXGI.
pub async fn vendor(_bin: &Path) -> String {
    tokio::task::spawn_blocking(pick_adapter)
        .await
        .ok()
        .flatten()
        .map(|(v, _)| v)
        .unwrap_or_else(|| "unknown".into())
}

/// One capture monitor (mirrors `os/linux/video::Monitor`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Monitor {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

/// A monitor with its native HMONITOR, which is what the ffmpeg
/// `gfxcapture` filter takes (`hmonitor=<value>`) — validated cross-process,
/// so the Rust-side list is authoritative and index-order cannot drift.
/// `index` is the DXGI/output order used by the `ddagrab` fallback;
/// `refresh_hz` is the real mode refresh (0 when the driver will not say).
#[derive(Debug, Clone)]
pub struct MonitorTarget {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub hmonitor: isize,
    pub index: u32,
    pub refresh_hz: u32,
}

/// Current mode refresh rate for a GDI device name (`\\.\DISPLAY1`), 0 when
/// unavailable. The engine clamps its capture cap to this so WGC never runs
/// above the panel; the cap itself is 2x the output rate (`capture_max_fps`).
fn refresh_hz(device_name: &str) -> u32 {
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Gdi::{
        EnumDisplaySettingsW, DEVMODEW, ENUM_CURRENT_SETTINGS,
    };
    let mut name: Vec<u16> = device_name.encode_utf16().collect();
    name.push(0);
    let mut dm: DEVMODEW = unsafe { std::mem::zeroed() };
    dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
    let ok = unsafe { EnumDisplaySettingsW(PCWSTR(name.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm) };
    if ok.as_bool() {
        dm.dmDisplayFrequency
    } else {
        0
    }
}

fn wide_name(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn enumerate_monitors() -> Vec<MonitorTarget> {
    let mut out = Vec::new();
    unsafe {
        let factory: IDXGIFactory1 = match CreateDXGIFactory1() {
            Ok(f) => f,
            Err(_) => return out,
        };
        let mut i = 0u32;
        loop {
            let Ok(adapter) = factory.EnumAdapters1(i) else {
                break;
            };
            i += 1;
            if let Ok(desc) = adapter.GetDesc1() {
                if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                    continue;
                }
            }
            let mut j = 0u32;
            loop {
                let Ok(output) = adapter.EnumOutputs(j) else {
                    break;
                };
                j += 1;
                let Ok(desc) = output.GetDesc() else {
                    continue;
                };
                let rect = desc.DesktopCoordinates;
                let width = (rect.right - rect.left).max(0) as u32;
                let height = (rect.bottom - rect.top).max(0) as u32;
                if width == 0 || height == 0 {
                    continue;
                }
                out.push(MonitorTarget {
                    name: wide_name(&desc.DeviceName),
                    width,
                    height,
                    hmonitor: desc.Monitor.0 as isize,
                    index: out.len() as u32,
                    refresh_hz: refresh_hz(&wide_name(&desc.DeviceName)),
                });
            }
        }
    }
    out
}

/// Full monitor list (DXGI device names like `\\.\DISPLAY1` — also what the
/// WGC enumeration returns).
pub async fn list_monitors(_bin: &Path) -> Vec<Monitor> {
    tokio::task::spawn_blocking(|| {
        enumerate_monitors()
            .into_iter()
            .map(|m| Monitor {
                name: m.name,
                width: m.width,
                height: m.height,
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// Resolve the monitor for a `source` setting: exact name match, otherwise
/// the primary monitor. `None` when no monitor can be enumerated.
pub async fn resolve_monitor(source: &str) -> Option<MonitorTarget> {
    let want = source.trim().to_string();
    tokio::task::spawn_blocking(move || {
        let monitors = enumerate_monitors();
        if monitors.is_empty() {
            return None;
        }
        if !want.is_empty() {
            if let Some(m) = monitors.iter().find(|m| m.name == want) {
                return Some(m.clone());
            }
        }
        // Primary first (MonitorFromPoint on the origin), else the first one.
        let primary = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
        monitors
            .iter()
            .find(|m| m.hmonitor as *mut core::ffi::c_void == primary.0)
            .cloned()
            .or_else(|| monitors.first().cloned())
    })
    .await
    .ok()
    .flatten()
}

/// Conservative static codec offer per vendor (used when no ffmpeg is
/// available to probe with). AV1 is offered only on NVIDIA — encode blocks
/// are missing on most AMD/Intel iGPUs and on pre-Ada GeForce cards, and a
/// listed-but-broken codec is worse than a hidden one. `x264` (CPU) is
/// always appended by `offered_codecs`.
fn static_codecs_for_vendor(vendor: &str) -> Vec<String> {
    let base: &[&str] = match vendor {
        "nvidia" => &["h264", "hevc", "av1"],
        "amd" | "intel" => &["h264", "hevc"],
        _ => &["h264"],
    };
    base.iter().map(|s| s.to_string()).collect()
}

/// Canonical ffmpeg encoder for (`vendor`, `codec`) in the capture path.
/// `None` means the combo cannot encode here (caller must fail loudly —
/// the settings UI only offers probed codecs, so this guards stale configs).
/// Shared by the live engine and the save path.
pub fn capture_encoder_name(vendor: &str, codec: &str) -> Option<&'static str> {
    match (vendor, codec) {
        ("nvidia", "h264") => Some("h264_nvenc"),
        ("nvidia", "hevc") => Some("hevc_nvenc"),
        ("nvidia", "av1") => Some("av1_nvenc"),
        ("amd", "h264") => Some("h264_amf"),
        ("amd", "hevc") => Some("hevc_amf"),
        ("intel", "h264") => Some("h264_qsv"),
        ("intel", "hevc") => Some("hevc_qsv"),
        (_, "x264") => Some("libx264"),
        _ => None,
    }
}

fn probe_encoder_name(vendor: &str, codec: &str) -> Option<&'static str> {
    capture_encoder_name(vendor, codec)
}

/// True when `ffmpeg` can actually open the encoder (10 black frames through
/// it). This catches missing HW blocks (e.g. AV1 on a Turing card), which a
/// vendor string alone cannot.
async fn probe_encoder(ffmpeg: &Path, encoder: &str) -> bool {
    let out = tokio::time::timeout(
        Duration::from_secs(8),
        tokio::process::Command::new(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=320x240:d=0.4",
                "-c:v",
                encoder,
                "-f",
                "null",
                "-",
            ])
            .output(),
    )
    .await;
    matches!(out, Ok(Ok(o)) if o.status.success())
}

fn probe_cache() -> &'static Mutex<HashMap<String, Vec<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Codec ids this machine can really encode, in ladder order. Probes each
/// candidate encoder with a live micro-encode (cached per process); falls
/// back to the static vendor table when ffmpeg is missing. `ffmpeg` must be
/// the SHIPPED binary (bundled sidecar first) — never an incidental PATH
/// copy. `x264` is always included when its probe passes (or when probing
/// is impossible but the vendor is known — a CPU encoder needs no GPU).
pub async fn offered_codecs(_bin: &Path, ffmpeg: &Path) -> Vec<String> {
    let v = vendor(_bin).await;
    if let Some(hit) = probe_cache().lock().ok().and_then(|c| c.get(&v).cloned()) {
        return hit;
    }
    let ffmpeg_ok = tokio::process::Command::new(ffmpeg)
        .args(["-hide_banner", "-version"])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    let mut ids: Vec<String> = if ffmpeg_ok {
        let mut candidates = static_codecs_for_vendor(&v);
        if !candidates.iter().any(|c| c == "x264") {
            candidates.push("x264".to_string());
        }
        let mut ok_ids = Vec::new();
        for codec in &candidates {
            let keep = match probe_encoder_name(&v, codec) {
                Some(enc) => probe_encoder(ffmpeg, enc).await,
                None => false,
            };
            if keep {
                ok_ids.push(codec.clone());
            }
        }
        // Never strand the user with an empty list: h264 is the floor.
        if ok_ids.is_empty() {
            static_codecs_for_vendor(&v)
        } else {
            ok_ids
        }
    } else {
        let mut ids = static_codecs_for_vendor(&v);
        if !ids.iter().any(|c| c == "x264") {
            ids.push("x264".to_string());
        }
        ids
    };
    // Ladder order: h264, hevc, av1, x264.
    ids.sort_by_key(|c| match c.as_str() {
        "h264" => 0,
        "hevc" => 1,
        "av1" => 2,
        _ => 3,
    });
    if let Ok(mut c) = probe_cache().lock() {
        c.insert(v, ids.clone());
    }
    ids
}

/// Save-time transcode encoder for (`vendor`, `codec`) — mirrors
/// `os/linux/video`. AMD→Amf, Intel→Qsv, NVIDIA→Nvenc; `x264` is software
/// and resolves on any vendor.
pub fn transcode_encoder(vendor: &str, codec: &str) -> Option<TranscodeEncoder> {
    if codec == "x264" {
        return Some(TranscodeEncoder::X264);
    }
    match vendor {
        "nvidia" => Some(TranscodeEncoder::Nvenc),
        "amd" => Some(TranscodeEncoder::Amf),
        "intel" => Some(TranscodeEncoder::Qsv),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::TranscodeEncoder as TE;
    use super::transcode_encoder;
    use super::{static_codecs_for_vendor, vendor_from_pci_id};

    #[test]
    fn pci_ids_map() {
        assert_eq!(vendor_from_pci_id(0x10DE), "nvidia");
        assert_eq!(vendor_from_pci_id(0x1002), "amd");
        assert_eq!(vendor_from_pci_id(0x1022), "amd");
        assert_eq!(vendor_from_pci_id(0x8086), "intel");
        assert_eq!(vendor_from_pci_id(0x1234), "unknown");
    }

    #[test]
    fn static_offer_is_conservative() {
        // AV1 only where encode blocks are near-guaranteed.
        assert!(static_codecs_for_vendor("nvidia").contains(&"av1".to_string()));
        assert!(!static_codecs_for_vendor("amd").contains(&"av1".to_string()));
        assert!(!static_codecs_for_vendor("intel").contains(&"av1".to_string()));
        assert!(!static_codecs_for_vendor("unknown").contains(&"hevc".to_string()));
    }

    #[test]
    fn transcode_mapping() {
        assert_eq!(transcode_encoder("nvidia", "h264"), Some(TE::Nvenc));
        assert_eq!(transcode_encoder("amd", "h264"), Some(TE::Amf));
        assert_eq!(transcode_encoder("intel", "h264"), Some(TE::Qsv));
        assert_eq!(transcode_encoder("unknown", "h264"), None);
        // x264 is software: resolves on any vendor, unknown included.
        assert_eq!(transcode_encoder("unknown", "x264"), Some(TE::X264));
        assert_eq!(transcode_encoder("nvidia", "x264"), Some(TE::X264));
    }
}
