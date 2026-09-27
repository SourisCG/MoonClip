//! Linux discovery for the Settings UI: GPU vendor and the codec offer. The
//! actual capture is owned by the embedded OBS through the XDG portal, so the
//! monitor list is intentionally empty (the portal dialog picks the screen).

use std::path::Path;

/// sysfs vendor id -> slug (mirrors the Windows PCI mapping).
fn vendor_from_id(id: &str) -> &'static str {
    match id.trim().to_lowercase().as_str() {
        "0x10de" | "10de" => "nvidia",
        "0x1002" | "1002" | "0x1022" | "1022" => "amd",
        "0x8086" | "8086" => "intel",
        _ => "unknown",
    }
}

fn read_sysfs(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// GPU vendor from sysfs; prefers the boot VGA card on hybrid systems.
pub async fn vendor() -> String {
    tokio::task::spawn_blocking(|| {
        let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
            return "unknown".to_string();
        };
        let mut fallback: Option<String> = None;
        for e in entries.flatten() {
            let name = e.file_name();
            let Some(name) = name.to_str() else { continue };
            if !(name.starts_with("card") && name[4..].chars().all(|c| c.is_ascii_digit())) {
                continue;
            }
            let dir = e.path().join("device");
            let Some(id) = read_sysfs(&dir.join("vendor")) else {
                continue;
            };
            let slug = vendor_from_id(&id);
            if slug == "unknown" {
                continue;
            }
            let boot = read_sysfs(&dir.join("boot_vga"))
                .map(|v| v.trim() == "1")
                .unwrap_or(false);
            if boot {
                return slug.to_string();
            }
            if fallback.is_none() {
                fallback = Some(slug.to_string());
            }
        }
        fallback.unwrap_or_else(|| "unknown".to_string())
    })
    .await
    .unwrap_or_else(|_| "unknown".into())
}

/// One capture monitor (empty on Linux: the portal dialog selects the screen).
/// Same field surface as the Windows backend so shared code stays OS-free.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Monitor {
    pub name: String,
    pub alt_id: String,
    pub label: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

pub async fn list_monitors() -> Vec<Monitor> {
    vec![]
}

/// No monitor list on Linux (the XDG portal picks the screen); kept for the
/// shared call site in commands.rs.
pub fn resolve_monitor<'a>(_monitors: &'a [Monitor], _stored: &str) -> Option<&'a Monitor> {
    None
}

/// Codec ids offered per vendor. OBS encoder availability cannot be probed
/// from outside, so this is the conservative static table (a listed-but-
/// broken codec fails loudly at start with the OBS log tail, never silently).
/// `x264` (CPU) is always offered.
pub async fn offered_codecs(_ffmpeg: &Path) -> Vec<String> {
    let v = vendor().await;
    let mut ids: Vec<String> = match v.as_str() {
        "nvidia" => vec!["h264", "hevc", "av1"],
        "amd" | "intel" => vec!["h264", "hevc"],
        _ => vec!["h264"],
    }
    .iter()
    .map(|s| s.to_string())
    .collect();
    ids.push("x264".into());
    ids
}

/// Playback decode capability. Hardware decode is strictly OPTIONAL: the
/// bundled ffmpeg always ships software decoders (`avdec_h264`, `dav1d`,
/// `openh264`), so a machine without a VA-API driver plays everything, just
/// with more CPU. The UI only ever shows the missing package NAME; each
/// distro's install command lives in docs/10_DEPENDENCIES.md.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DecodeStatus {
    /// GPU vendor slug (`nvidia`/`amd`/`intel`/`unknown`).
    pub vendor: String,
    /// Distro family (`fedora`/`debian`/`arch`/`suse`/id/`unknown`).
    pub distro: String,
    /// A VA-API driver for this vendor is installed.
    pub hardware: bool,
    /// Driver file found (e.g. `nvidia_drv_video.so`).
    pub driver: Option<String>,
    /// Package that would provide it (name only, when missing).
    pub missing_package: Option<String>,
}

/// Distro family from os-release content (pure: testable).
fn distro_family_from(os_release: &str) -> String {
    let id = os_release
        .lines()
        .find_map(|l| l.strip_prefix("ID="))
        .map(|v| v.trim().trim_matches('"').to_lowercase())
        .unwrap_or_default();
    match id.as_str() {
        "fedora" | "rhel" | "centos" | "rocky" | "almalinux" => "fedora".into(),
        "debian" | "ubuntu" | "linuxmint" | "pop" | "zorin" | "elementary" => "debian".into(),
        "arch" | "endeavouros" | "manjaro" | "cachyos" | "garuda" => "arch".into(),
        "opensuse" | "opensuse-leap" | "opensuse-tumbleweed" | "sles" => "suse".into(),
        "" => "unknown".into(),
        other => other.into(),
    }
}

fn distro_family() -> String {
    distro_family_from(&std::fs::read_to_string("/etc/os-release").unwrap_or_default())
}

/// VA-API driver file candidates per vendor. NVIDIA needs the separate
/// `nvidia-vaapi-driver` (RPM Fusion on Fedora); AMD/Intel drivers ship with
/// Mesa/the media driver.
fn va_driver_names(vendor: &str) -> &'static [&'static str] {
    match vendor {
        "nvidia" => &["nvidia_drv_video.so"],
        "amd" => &["radeonsi_drv_video.so"],
        "intel" => &["iHD_drv_video.so", "i965_drv_video.so"],
        _ => &[
            "nvidia_drv_video.so",
            "radeonsi_drv_video.so",
            "iHD_drv_video.so",
            "i965_drv_video.so",
        ],
    }
}

/// Common DRI driver directories (Fedora/RHEL, Debian/Ubuntu, Arch).
const DRI_DIRS: &[&str] = &[
    "/usr/lib64/dri",
    "/usr/lib/x86_64-linux-gnu/dri",
    "/usr/lib/dri",
];

/// First VA-API driver file present in `dirs` (pure: testable with temp dirs).
fn va_driver_in(vendor: &str, dirs: &[&str]) -> Option<String> {
    for name in va_driver_names(vendor) {
        if dirs.iter().any(|dir| Path::new(dir).join(name).is_file()) {
            return Some((*name).to_string());
        }
    }
    None
}

fn va_driver_for(vendor: &str) -> Option<String> {
    va_driver_in(vendor, DRI_DIRS)
}

/// Package providing the VA-API driver per vendor+distro (name only).
fn decode_package(vendor: &str, distro: &str) -> Option<String> {
    let name = match (vendor, distro) {
        ("nvidia", "arch") => "libva-nvidia-driver",
        ("nvidia", _) => "nvidia-vaapi-driver",
        ("amd", "arch") | ("amd", "suse") => "libva-mesa-driver",
        ("amd", _) => "mesa-va-drivers",
        ("intel", "debian") => "intel-media-va-driver",
        ("intel", _) => "intel-media-driver",
        _ => return None,
    };
    Some(name.to_string())
}

/// Detect the decode path for the Settings notice.
pub async fn decode_status() -> DecodeStatus {
    let vendor = vendor().await;
    tokio::task::spawn_blocking(move || {
        let distro = distro_family();
        let driver = va_driver_for(&vendor);
        let missing_package = if driver.is_none() {
            decode_package(&vendor, &distro)
        } else {
            None
        };
        DecodeStatus {
            hardware: driver.is_some(),
            vendor,
            distro,
            driver,
            missing_package,
        }
    })
    .await
    .unwrap_or(DecodeStatus {
        vendor: "unknown".into(),
        distro: "unknown".into(),
        hardware: false,
        driver: None,
        missing_package: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{decode_package, distro_family_from, va_driver_names, vendor_from_id};

    #[test]
    fn sysfs_ids_map() {
        assert_eq!(vendor_from_id("0x10de\n"), "nvidia");
        assert_eq!(vendor_from_id("0x1002\n"), "amd");
        assert_eq!(vendor_from_id("0x8086\n"), "intel");
        assert_eq!(vendor_from_id("0x1234\n"), "unknown");
    }

    #[test]
    fn os_release_maps_to_families() {
        assert_eq!(distro_family_from("ID=fedora\n"), "fedora");
        assert_eq!(distro_family_from("ID=\"ubuntu\"\n"), "debian");
        assert_eq!(distro_family_from("ID=manjaro\n"), "arch");
        assert_eq!(distro_family_from("ID=opensuse-tumbleweed\n"), "suse");
        assert_eq!(distro_family_from("NAME=Weird\n"), "unknown");
        assert_eq!(distro_family_from("ID=nixos\n"), "nixos");
    }

    #[test]
    fn vendor_driver_names() {
        assert_eq!(va_driver_names("nvidia"), &["nvidia_drv_video.so"]);
        assert!(va_driver_names("intel").contains(&"iHD_drv_video.so"));
        // Unknown vendors accept any known driver (hybrid machines).
        assert!(va_driver_names("unknown").len() >= 4);
    }

    #[test]
    fn driver_lookup_scans_the_dirs() {
        use super::va_driver_in;
        let dir = std::env::temp_dir().join(format!(
            "moonclip-dri-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dirs = [dir.to_str().unwrap()];
        assert_eq!(va_driver_in("nvidia", &dirs), None);
        std::fs::write(dir.join("radeonsi_drv_video.so"), b"").unwrap();
        assert_eq!(
            va_driver_in("amd", &dirs).as_deref(),
            Some("radeonsi_drv_video.so")
        );
        // A Radeon driver never satisfies NVIDIA.
        assert_eq!(va_driver_in("nvidia", &dirs), None);
        // Unknown vendors accept whatever is there.
        assert_eq!(
            va_driver_in("unknown", &dirs).as_deref(),
            Some("radeonsi_drv_video.so")
        );
        std::fs::write(dir.join("iHD_drv_video.so"), b"").unwrap();
        assert_eq!(
            va_driver_in("intel", &dirs).as_deref(),
            Some("iHD_drv_video.so")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn package_names_per_distro() {
        assert_eq!(
            decode_package("nvidia", "fedora").as_deref(),
            Some("nvidia-vaapi-driver")
        );
        assert_eq!(
            decode_package("nvidia", "arch").as_deref(),
            Some("libva-nvidia-driver")
        );
        assert_eq!(
            decode_package("amd", "debian").as_deref(),
            Some("mesa-va-drivers")
        );
        assert_eq!(
            decode_package("intel", "debian").as_deref(),
            Some("intel-media-va-driver")
        );
        assert_eq!(decode_package("unknown", "fedora"), None);
    }
}
