//! Windows discovery for the Settings UI: GPU vendor, monitors and the codec
//! offer probed with a live micro-encode against the SHIPPED ffmpeg (never an
//! incidental PATH copy). Capture itself is owned by the embedded OBS.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE,
};

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
pub async fn vendor() -> String {
    tokio::task::spawn_blocking(pick_adapter)
        .await
        .ok()
        .flatten()
        .map(|(v, _)| v)
        .unwrap_or_else(|| "unknown".into())
}

/// One capture monitor. `name` is the OBS `monitor_id` (WinRT device
/// interface id, e.g. `\\?\DISPLAY#...`), which is what the modern
/// `monitor_capture` (duplicator) source takes; `alt_id` is the legacy GDI
/// name (`\\.\DISPLAY1`). `label` is for the Settings UI only.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Monitor {
    pub name: String,
    pub alt_id: String,
    pub label: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

fn cstr(buf: &[u8]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).to_string()
}

/// Native monitor geometry straight from GDI (no OBS dependency): the device
/// interface id (what OBS's duplicator source matches) and the legacy
/// `\\.\DISPLAYn` name, primary flagged for the UI default.
pub async fn list_monitors() -> Vec<Monitor> {
    tokio::task::spawn_blocking(|| {
        use windows::core::BOOL;
        use windows::Win32::Foundation::{LPARAM, RECT};
        use windows::Win32::Graphics::Gdi::{
            EnumDisplayDevicesA, EnumDisplayMonitors, GetMonitorInfoW, DISPLAY_DEVICEA, HDC,
            HMONITOR, MONITORINFO, MONITORINFOEXW,
        };
        unsafe extern "system" fn cb(
            hmonitor: HMONITOR,
            _hdc: HDC,
            _rect: *mut RECT,
            data: LPARAM,
        ) -> BOOL {
            let list = &mut *(data.0 as *mut Vec<Monitor>);
            let mut info = MONITORINFOEXW::default();
            info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            let ok =
                unsafe { GetMonitorInfoW(hmonitor, &mut info.monitorInfo as *mut MONITORINFO) }
                    .as_bool();
            if ok {
                let r = info.monitorInfo.rcMonitor;
                let alt_id = String::from_utf16_lossy(
                    &info.szDevice[..info
                        .szDevice
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(info.szDevice.len())],
                );
                let mut dd = DISPLAY_DEVICEA {
                    cb: std::mem::size_of::<DISPLAY_DEVICEA>() as u32,
                    ..Default::default()
                };
                // EDD_GET_DEVICE_INTERFACE_NAME = 0x1
                let mut id = String::new();
                if unsafe {
                    EnumDisplayDevicesA(windows::core::PCSTR(alt_id.as_ptr()), 0, &mut dd, 1)
                }
                .as_bool()
                {
                    let raw: Vec<u8> = dd.DeviceID.iter().map(|&c| c as u8).collect();
                    id = cstr(&raw);
                }
                if id.is_empty() {
                    id = alt_id.clone();
                }
                list.push(Monitor {
                    name: id,
                    alt_id,
                    label: String::new(),
                    width: (r.right - r.left).max(0) as u32,
                    height: (r.bottom - r.top).max(0) as u32,
                    primary: info.monitorInfo.dwFlags & 1 != 0,
                });
            }
            BOOL(1)
        }
        let mut list: Vec<Monitor> = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(
                None,
                None,
                Some(cb),
                LPARAM(&mut list as *mut Vec<Monitor> as isize),
            );
        }
        // Stable order: primary first, then the rest.
        list.sort_by_key(|m| !m.primary);
        for (i, m) in list.iter_mut().enumerate() {
            m.label = format!(
                "Monitor {}: {}×{}{}",
                i + 1,
                m.width,
                m.height,
                if m.primary { " (primary)" } else { "" }
            );
        }
        list
    })
    .await
    .unwrap_or_default()
}

/// Legacy stored value -> monitor from the current list: device id, GDI
/// alt_id, or a plain index from the primary-first order.
pub fn resolve_monitor<'a>(monitors: &'a [Monitor], stored: &str) -> Option<&'a Monitor> {
    let want = stored.trim();
    if want.is_empty() {
        return monitors
            .iter()
            .find(|m| m.primary)
            .or_else(|| monitors.first());
    }
    if let Some(m) = monitors.iter().find(|m| m.name == want || m.alt_id == want) {
        return Some(m);
    }
    if let Ok(idx) = want.parse::<usize>() {
        if let Some(m) = monitors.get(idx) {
            return Some(m);
        }
    }
    monitors
        .iter()
        .find(|m| m.primary)
        .or_else(|| monitors.first())
}

/// Conservative static codec offer per vendor (used when no ffmpeg is
/// available to probe with). AV1 is offered only on NVIDIA — encode blocks
/// are missing on most AMD/Intel iGPUs and on pre-Ada GeForce cards, and a
/// listed-but-broken codec is worse than a hidden one. CPU x264 is appended
/// by `offered_codecs` because it needs no GPU.
fn static_codecs_for_vendor(vendor: &str) -> Vec<String> {
    let base: &[&str] = match vendor {
        "nvidia" => &["h264", "hevc", "av1"],
        "amd" | "intel" => &["h264", "hevc"],
        _ => &["h264"],
    };
    base.iter().map(|s| s.to_string()).collect()
}

fn probe_encoder_name(vendor: &str, codec: &str) -> Option<&'static str> {
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
pub async fn offered_codecs(ffmpeg: &Path) -> Vec<String> {
    let v = vendor().await;
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

#[cfg(test)]
mod tests {
    use super::{resolve_monitor, static_codecs_for_vendor, vendor_from_pci_id, Monitor};

    fn m(name: &str, alt: &str, primary: bool) -> Monitor {
        Monitor {
            name: name.into(),
            alt_id: alt.into(),
            label: String::new(),
            width: 1920,
            height: 1080,
            primary,
        }
    }

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
    fn monitor_resolution_prefers_device_id_then_alt_then_index() {
        let monitors = vec![
            m("dev-a", r"\\.\DISPLAY1", true),
            m("dev-b", r"\\.\DISPLAY2", false),
        ];
        assert_eq!(
            resolve_monitor(&monitors, "dev-a").unwrap().alt_id,
            r"\\.\DISPLAY1"
        );
        assert_eq!(
            resolve_monitor(&monitors, r"\\.\DISPLAY2").unwrap().name,
            "dev-b"
        );
        assert_eq!(resolve_monitor(&monitors, "0").unwrap().name, "dev-a");
        assert_eq!(resolve_monitor(&monitors, "1").unwrap().name, "dev-b");
        // Unknown/legacy values fall back to primary, never to nothing.
        assert_eq!(resolve_monitor(&monitors, "DP-1").unwrap().name, "dev-a");
        assert_eq!(resolve_monitor(&monitors, "").unwrap().name, "dev-a");
        assert!(resolve_monitor(&[], "x").is_none());
    }
}
