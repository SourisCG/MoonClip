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

#[cfg(test)]
mod tests {
    use super::vendor_from_id;

    #[test]
    fn sysfs_ids_map() {
        assert_eq!(vendor_from_id("0x10de\n"), "nvidia");
        assert_eq!(vendor_from_id("0x1002\n"), "amd");
        assert_eq!(vendor_from_id("0x8086\n"), "intel");
        assert_eq!(vendor_from_id("0x1234\n"), "unknown");
    }
}
