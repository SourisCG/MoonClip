//! Linux video discovery from OUR bundled GSR binary.

use super::super::TranscodeEncoder;
use std::path::Path;

/// GSR's live scaler proved soft on text at non-integer ratios, so this
/// backend buffers at source resolution and downscales with lanczos at save
/// (see `commands.rs` capture plan). Windows scales on the GPU live.
pub fn scales_live() -> bool {
    false
}

/// GPU vendor, lowercase (`nvidia`/`amd`/`intel`/…), from `--info`.
pub async fn vendor(bin: &Path) -> String {
    let Ok(out) = tokio::process::Command::new(bin)
        .arg("--info")
        .output()
        .await
    else {
        return "unknown".into();
    };
    parse_vendor(&String::from_utf8_lossy(&out.stdout))
}

fn parse_vendor(info: &str) -> String {
    let mut in_gpu = false;
    for line in info.lines() {
        let line = line.trim();
        if line.starts_with("section=") {
            in_gpu = line == "section=gpu_info";
            continue;
        }
        if in_gpu && line.starts_with("vendor|") {
            return line["vendor|".len()..].trim().to_lowercase();
        }
    }
    "unknown".into()
}

/// Codec ids from `--info` (`section=video_codecs`), mapped to app ids.
/// The ffmpeg path is Windows-only surface (probes); Linux ignores it.
pub async fn offered_codecs(bin: &Path, _ffmpeg: &Path) -> Vec<String> {
    let Ok(out) = tokio::process::Command::new(bin)
        .arg("--info")
        .output()
        .await
    else {
        return vec![];
    };
    if !out.status.success() {
        return vec![];
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut in_codecs = false;
    let mut ids = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("section=") {
            in_codecs = line == "section=video_codecs";
            continue;
        }
        if in_codecs && !line.is_empty() {
            if let Some(id) = app_codec_id(line) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
    }
    ids
}

/// GSR `--info` codec id -> app codec id. The H.264 software encoder is our
/// `x264` (spawned as `-k h264 -encoder cpu`); other `_software` variants
/// have no app id yet, so `None` keeps dead options out of the UI.
fn app_codec_id(gsr_id: &str) -> Option<String> {
    match gsr_id {
        "h264_software" => Some("x264".to_string()),
        other if other.ends_with("_software") => None,
        other => Some(other.to_string()),
    }
}

/// One capture monitor (`--list-monitors`, `NAME|WxH` lines).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Monitor {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

/// Full monitor list. Empty if the backend cannot enumerate.
pub async fn list_monitors(bin: &Path) -> Vec<Monitor> {
    let Ok(out) = tokio::process::Command::new(bin)
        .arg("--list-monitors")
        .output()
        .await
    else {
        return vec![];
    };
    parse_monitors(&String::from_utf8_lossy(&out.stdout))
}

fn parse_monitors(out: &str) -> Vec<Monitor> {
    out.lines()
        .filter_map(|l| {
            let (name, dims) = l.split_once('|')?;
            let (w, h) = dims.split_once('x')?;
            Some(Monitor {
                name: name.trim().to_string(),
                width: w.trim().parse().ok()?,
                height: h.trim().parse().ok()?,
            })
        })
        .collect()
}

/// Save-time transcode encoder for (`vendor`, `codec`).
/// Intel/AMD capture works through backend defaults (VAAPI); this only picks
/// the re-encode path. AMD/VAAPI transcode needs render-node plumbing that is
/// only validated on real HW, so it returns None (caller keeps source file).
pub fn transcode_encoder(vendor: &str, codec: &str) -> Option<TranscodeEncoder> {
    // Software capture (x264) stays software on save-scale: the user picked
    // CPU on purpose and the source pixels are already software-encoded.
    if codec == "x264" {
        return Some(TranscodeEncoder::X264);
    }
    match vendor {
        "nvidia" => Some(TranscodeEncoder::Nvenc),
        "intel" => Some(TranscodeEncoder::Qsv),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_monitors, parse_vendor};

    #[test]
    fn parses_vendor() {
        let info = "section=system_info\ndisplay_server|wayland\nsection=gpu_info\nvendor|nvidia\n";
        assert_eq!(parse_vendor(info), "nvidia");
    }

    #[test]
    fn transcode_mapping() {
        use super::super::super::TranscodeEncoder;
        use super::transcode_encoder;
        assert_eq!(transcode_encoder("nvidia", "h264"), Some(TranscodeEncoder::Nvenc));
        assert_eq!(transcode_encoder("nvidia", "hevc"), Some(TranscodeEncoder::Nvenc));
        assert_eq!(transcode_encoder("intel", "h264"), Some(TranscodeEncoder::Qsv));
        assert_eq!(transcode_encoder("amd", "h264"), None);
        assert_eq!(transcode_encoder("unknown", "h264"), None);
        // CPU capture stays CPU on save-scale, regardless of vendor.
        assert_eq!(transcode_encoder("nvidia", "x264"), Some(TranscodeEncoder::X264));
    }

    #[test]
    fn software_codecs_map_to_app_ids() {
        use super::app_codec_id;
        assert_eq!(app_codec_id("h264"), Some("h264".to_string()));
        assert_eq!(app_codec_id("h264_software"), Some("x264".to_string()));
        assert_eq!(app_codec_id("hevc"), Some("hevc".to_string()));
        assert_eq!(app_codec_id("hevc_software"), None);
    }

    #[test]
    fn parses_monitors() {
        let out = "DP-1|1920x1080\nDP-2|1280x720\n";
        let ms = parse_monitors(out);
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[0].name, "DP-1");
        assert_eq!(ms[0].height, 1080);
        assert_eq!(ms.iter().map(|m| m.height).max(), Some(1080));
    }
}
