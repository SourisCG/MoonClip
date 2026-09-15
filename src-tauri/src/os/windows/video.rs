//! Windows video discovery: DXGI adapters (vendor) + WGC monitors + codec offer.
//! OS floor: Windows 10 1903+ (WGC). No sidecar binary is involved — the
//! `_bin` params exist only for signature parity with `os/linux/video`, and
//! are ignored. `ffmpeg` for encoder probes resolves via `capture_ffmpeg()`
//! (MOONCLIP_FFMPEG override, else PATH); the Phase 7 bundled sidecar plugs
//! into the same helper.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use super::super::TranscodeEncoder;

/// ffmpeg binary for capture/probe duties: explicit override, else PATH.
/// (The Phase 7 BtbN sidecar is wired here once `externalBin` ships.)
pub fn capture_ffmpeg() -> PathBuf {
    if let Ok(path) = std::env::var("MOONCLIP_FFMPEG") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("ffmpeg")
}

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
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
    };
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

/// Full monitor list via WGC enumeration. Empty if capture is unsupported.
pub async fn list_monitors(_bin: &Path) -> Vec<Monitor> {
    tokio::task::spawn_blocking(|| {
        let mut out = Vec::new();
        let Ok(monitors) = windows_capture::monitor::Monitor::enumerate() else {
            return out;
        };
        for m in monitors {
            let name = m
                .name()
                .unwrap_or_else(|_| format!("display-{}", out.len() + 1));
            if let (Ok(w), Ok(h)) = (m.width(), m.height()) {
                if w > 0 && h > 0 {
                    out.push(Monitor {
                        name,
                        width: w,
                        height: h,
                    });
                }
            }
        }
        out
    })
    .await
    .unwrap_or_default()
}

/// Resolve the WGC monitor for a `source` setting: exact name match,
/// otherwise the primary monitor. `None` when capture is unsupported.
pub fn resolve_monitor(source: &str) -> Option<windows_capture::monitor::Monitor> {
    let want = source.trim();
    if !want.is_empty() {
        if let Ok(monitors) = windows_capture::monitor::Monitor::enumerate() {
            for m in &monitors {
                if m.name().as_deref().unwrap_or("") == want {
                    let idx = m.index().ok()?;
                    return windows_capture::monitor::Monitor::from_index(idx).ok();
                }
            }
        }
    }
    windows_capture::monitor::Monitor::primary().ok()
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

/// Candidate ffmpeg encoder names per codec id for the probe.
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
    use super::{static_codecs_for_vendor, vendor_from_pci_id};
    use super::transcode_encoder;
    use super::super::super::TranscodeEncoder as TE;

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
