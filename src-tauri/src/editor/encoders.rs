//! ffmpeg encoder selection with a guaranteed CPU fallback.
//!
//! Hardware encoders (NVENC / Quick Sync / AMF / VAAPI) are used when the
//! shipped ffmpeg can actually initialize them (1-frame probe, so a broken
//! driver never aborts an export); otherwise `libx264` — always present in
//! both bundled GPL sidecars — does the job. Machines without any GPU work
//! out of the box.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EncoderInfo {
    pub id: String,
    pub label: String,
    pub hw: bool,
    pub available: bool,
}

const CPU: &str = "libx264";

fn candidates() -> &'static [&'static str] {
    #[cfg(target_os = "windows")]
    {
        &["h264_nvenc", "h264_qsv", "h264_amf"]
    }
    #[cfg(target_os = "linux")]
    {
        &["h264_nvenc", "h264_qsv", "h264_vaapi"]
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        &[]
    }
}

fn label_for(id: &str) -> &'static str {
    match id {
        "h264_nvenc" => "NVIDIA NVENC (GPU)",
        "h264_qsv" => "Intel Quick Sync (GPU)",
        "h264_amf" => "AMD AMF (GPU)",
        "h264_vaapi" => "VAAPI (GPU)",
        _ => "x264 (CPU)",
    }
}

/// Parse `ffmpeg -encoders` output into the set of encoder ids.
fn parse_encoder_ids(text: &str) -> HashSet<String> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let flags = parts.next()?;
            if flags.len() < 6 || !flags.starts_with('V') {
                return None;
            }
            let id = parts.next()?;
            (!id.starts_with('=')).then(|| id.to_string())
        })
        .collect()
}

fn vaapi_device() -> Option<PathBuf> {
    let dir = Path::new("/dev/dri");
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("renderD"))
        })
        .collect();
    entries.sort();
    entries.into_iter().next()
}

/// Real 1-frame encode to prove the encoder can initialize on this machine.
async fn validates(ffmpeg: &Path, encoder: &str) -> bool {
    let mut cmd = tokio::process::Command::new(ffmpeg);
    if encoder == "h264_vaapi" {
        let Some(dev) = vaapi_device() else {
            return false;
        };
        cmd.args(["-hide_banner", "-loglevel", "error", "-vaapi_device"])
            .arg(&dev)
            .args(["-f", "lavfi", "-i", "color=c=black:s=64x64:r=1", "-frames:v", "1"])
            .args(["-vf", "format=nv12,hwupload", "-c:v", encoder]);
    } else {
        cmd.args([
            "-hide_banner", "-loglevel", "error",
            "-f", "lavfi", "-i", "color=c=black:s=64x64:r=1",
            "-frames:v", "1", "-an", "-c:v", encoder,
        ]);
    }
    cmd.args(["-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

static CACHE: OnceLock<Mutex<HashMap<PathBuf, Vec<EncoderInfo>>>> = OnceLock::new();

/// Available encoders, best first (probed once per ffmpeg binary).
pub async fn detect(ffmpeg: &Path) -> Vec<EncoderInfo> {
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().ok().and_then(|m| m.get(ffmpeg).cloned()) {
        return hit;
    }
    let listed: HashSet<String> = tokio::process::Command::new(ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .output()
        .await
        .map(|o| parse_encoder_ids(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default();

    let mut out = Vec::new();
    for id in candidates() {
        if !listed.contains(*id) {
            out.push(EncoderInfo {
                id: (*id).into(),
                label: label_for(id).into(),
                hw: true,
                available: false,
            });
            continue;
        }
        let available = validates(ffmpeg, id).await;
        if available {
            eprintln!("[moonclip] editor encoder available: {id}");
        }
        out.push(EncoderInfo {
            id: (*id).into(),
            label: label_for(id).into(),
            hw: true,
            available,
        });
    }
    out.push(EncoderInfo {
        id: CPU.into(),
        label: label_for(CPU).into(),
        hw: false,
        available: listed.contains(CPU),
    });
    if let Ok(mut m) = cache.lock() {
        m.insert(ffmpeg.to_path_buf(), out.clone());
    }
    out
}

/// Encoder id for an export/proxy. `preference`: "auto" | "cpu" | explicit id.
pub async fn pick(ffmpeg: &Path, preference: &str) -> String {
    match preference {
        "cpu" => CPU.to_string(),
        "" | "auto" => detect(ffmpeg)
            .await
            .iter()
            .find(|e| e.hw && e.available)
            .map(|e| e.id.clone())
            .unwrap_or_else(|| CPU.to_string()),
        explicit => {
            let ok = detect(ffmpeg)
                .await
                .iter()
                .any(|e| e.id == explicit && e.available);
            if ok {
                explicit.to_string()
            } else {
                CPU.to_string()
            }
        }
    }
}

/// Quality args for the chosen encoder (CRF for x264, CQ for NVENC, etc.).
pub fn quality_args(encoder: &str, bitrate_kbps: u32, height: u32) -> Vec<String> {
    let bitrate = if bitrate_kbps > 0 {
        bitrate_kbps
    } else {
        // Conservative ladder by output height when the user left it auto.
        match height {
            0..=360 => 2_500,
            361..=540 => 4_000,
            541..=720 => 6_000,
            721..=1080 => 10_000,
            1081..=1440 => 16_000,
            _ => 24_000,
        }
    };
    let mut a = vec!["-b:v".to_string(), format!("{bitrate}k")];
    match encoder {
        "libx264" => {
            // Keep a CQ-ish fallback plus the bitrate cap for size control.
            a.extend(["-preset".into(), "veryfast".into(), "-crf".into(), "20".into()]);
        }
        "h264_nvenc" => {
            a.extend(["-preset".into(), "p4".into(), "-rc".into(), "vbr".into()]);
        }
        "h264_qsv" => {
            a.extend(["-preset".into(), "medium".into()]);
        }
        "h264_amf" => {
            a.extend(["-quality".into(), "balanced".into()]);
        }
        "h264_vaapi" => {}
        _ => {}
    }
    a.push("-maxrate".into());
    a.push(format!("{}k", bitrate * 3 / 2));
    a.push("-bufsize".into());
    a.push(format!("{}k", bitrate * 2));
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_encoder_listing() {
        let text = "\
Encoders:
 V..... = Video
 ------
 V....D libx264              libx264 H.264 / AVC (codec h264)
 V....D h264_nvenc           NVIDIA NVENC H.264 encoder (codec h264)
 A....D aac                  AAC (Advanced Audio Coding)
 V....D h264_vaapi           H.264/AVC (VAAPI) (codec h264)
";
        let ids = parse_encoder_ids(text);
        assert!(ids.contains("libx264"));
        assert!(ids.contains("h264_nvenc"));
        assert!(ids.contains("h264_vaapi"));
        assert!(!ids.contains("aac")); // audio-only line
        assert!(!ids.contains("="));
    }

    #[test]
    fn cpu_is_always_a_candidate_fallback() {
        assert_eq!(CPU, "libx264");
        assert!(!candidates().contains(&CPU));
    }

    #[tokio::test]
    async fn pick_cpu_never_probes_hardware() {
        // "cpu" short-circuits before any ffmpeg run, even with a bad path.
        assert_eq!(pick(Path::new("/nonexistent/ffmpeg"), "cpu").await, CPU);
    }

    #[test]
    fn quality_args_use_default_ladder_when_bitrate_absent() {
        let a = quality_args("libx264", 0, 1080);
        assert!(a.windows(2).any(|w| w == ["-b:v", "10000k"]));
        let a = quality_args("h264_nvenc", 8_000, 720);
        assert!(a.windows(2).any(|w| w == ["-b:v", "8000k"]));
    }
}
