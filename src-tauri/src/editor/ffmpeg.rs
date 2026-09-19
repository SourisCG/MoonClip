//! FFmpeg helpers (Phase 3: thumbnails only; trim presets land in Phase 5).
//! Binary resolution: MOONCLIP_FFMPEG override -> app-bundled sidecar
//! (BtbN static, see docs/THIRD_PARTY.md) -> PATH fallback (dev only).

use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

/// Triple-aware sidecar file name, e.g.
/// `ffmpeg-x86_64-pc-windows-msvc.exe` / `ffmpeg-x86_64-unknown-linux-gnu`.
pub fn sidecar_name() -> String {
    let ext = std::env::consts::EXE_EXTENSION;
    if ext.is_empty() {
        format!("ffmpeg-{}", crate::sidecar::host_triple())
    } else {
        format!("ffmpeg-{}.{}", crate::sidecar::host_triple(), ext)
    }
}

pub fn resolve_ffmpeg(app: &AppHandle) -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("MOONCLIP_FFMPEG") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return Ok(p);
        }
    }
    // Bundled sidecar via the shared layout walker (dev staging +
    // production resources, triple-scoped). This also covers the Phase 7
    // `bundle.resources` shipment with no further code changes.
    if let Some((p, source)) = crate::sidecar::search_bundled(&sidecar_name(), app) {
        eprintln!("[moonclip] ffmpeg: {} ({})", p.display(), source);
        return Ok(p);
    }
    // Legacy resource scan (flat `binaries/ffmpeg*` layout).
    if let Ok(res) = app.path().resource_dir() {
        let dir = res.join("binaries");
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut cands: Vec<PathBuf> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("ffmpeg"))
                        .unwrap_or(false)
                })
                .collect();
            cands.sort();
            if let Some(p) = cands.into_iter().next() {
                return Ok(p);
            }
        }
    }
    // Dev fallback: system ffmpeg. Production always ships the pinned sidecar.
    eprintln!("[moonclip] ffmpeg: no bundled sidecar, falling back to PATH (dev only)");
    Ok(PathBuf::from("ffmpeg"))
}

/// Measure real duration in ms via `ffmpeg -i` stderr (no ffprobe needed —
/// the static sidecar does not ship ffprobe). Returns None on parse failure.
pub async fn probe_duration_ms(ffmpeg: &Path, input: &Path) -> Option<i64> {
    let out = tokio::process::Command::new(ffmpeg)
        .args(["-hide_banner", "-i", &input.to_string_lossy()])
        .output()
        .await
        .ok()?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Line looks like: Duration: 00:01:20.65, start: 0.000000, bitrate: ...
    let line = stderr
        .lines()
        .find(|l| l.trim_start().starts_with("Duration:"))?;
    let time = line.split(',').next()?.split("Duration:").nth(1)?.trim();
    let mut parts = time.split(':');
    let h: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let s: f64 = parts.next()?.parse().ok()?;
    Some(((h * 3600 + m * 60) as f64 * 1000.0 + s * 1000.0) as i64)
}

/// Extract one JPEG thumbnail at `seek_secs`. Fast (no re-encode of the clip).
/// `-strict unofficial`: capture pixels are limited-range yuv420p (NVENC /
/// swscale default) and ffmpeg 9's mjpeg encoder rejects them otherwise.
/// Pixels are untouched — this is only a gallery preview.
pub async fn make_thumbnail(
    ffmpeg: &Path,
    input: &Path,
    output: &Path,
    seek_secs: f32,
) -> Result<(), String> {
    let seek = format!("{:.2}", seek_secs.clamp(0.05, 3600.0));
    let status = tokio::process::Command::new(ffmpeg)
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-ss",
            &seek,
            "-i",
            &input.to_string_lossy(),
            "-vframes",
            "1",
            "-q:v",
            "2",
            "-strict",
            "unofficial",
        ])
        .arg(output)
        .status()
        .await
        .map_err(|e| format!("ffmpeg thumbnail failed: {e}"))?;
    if !status.success() {
        return Err("ffmpeg thumbnail failed".into());
    }
    Ok(())
}

/// Real video stream of a file, parsed from `ffmpeg -i` stderr (the static
/// sidecar does not ship ffprobe). Used by the hardware test to prove OBS
/// really encoded what was requested (codec / resolution / fps).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct VideoProbe {
    pub codec_name: String,
    pub profile: String,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
}

/// Parse one `ffmpeg -i` stream line, e.g.
/// `Stream #0:0(und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(tv, bt709, progressive), 1920x1080 [SAR 1:1 DAR 16:9], 19427 kb/s, 60 fps, 60 tbr, ...`
pub fn parse_video_stream_line(line: &str) -> Option<VideoProbe> {
    let after = line.trim().split_once("Video:")?.1.trim();
    let codec_name = after
        .split([' ', '('])
        .next()?
        .trim()
        .trim_end_matches(',')
        .to_string();
    if codec_name.is_empty() {
        return None;
    }
    // Profile: first "(...)" group WITHOUT "/" inside the codec descriptor
    // (before the first comma). Pixel/tag groups like "(avc1 / ...)" or the
    // "(tv, bt709, ...)" pixel descriptor are skipped.
    let head = after.split(',').next().unwrap_or(after);
    let mut profile = String::new();
    let mut rest = head;
    while let Some(open) = rest.find('(') {
        let inner = &rest[open + 1..];
        let Some(close) = inner.find(')') else {
            break;
        };
        let group = &inner[..close];
        if !group.contains('/') && !group.trim().is_empty() {
            profile = group.trim().to_string();
            break;
        }
        rest = &inner[close + 1..];
    }
    // Dimensions: first WxH token.
    let (mut width, mut height) = (0u32, 0u32);
    for tok in after.split([' ', ',', '[']) {
        let t = tok.trim();
        if let Some((w, h)) = t.split_once('x') {
            if let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.parse::<u32>()) {
                if w > 0 && h > 0 {
                    width = w;
                    height = h;
                    break;
                }
            }
        }
    }
    if width == 0 || height == 0 {
        return None;
    }
    // Frame rate: number right before a standalone "fps" token.
    let toks: Vec<&str> = after.split([' ', ',']).collect();
    let mut fps = 0.0f64;
    for (i, t) in toks.iter().enumerate() {
        if t.trim_matches(|c: char| !c.is_alphanumeric()) == "fps" && i > 0 {
            if let Ok(f) = toks[i - 1].trim().trim_end_matches(',').parse::<f64>() {
                fps = f;
                break;
            }
        }
    }
    if fps <= 0.0 {
        return None;
    }
    Some(VideoProbe {
        codec_name,
        profile,
        width,
        height,
        fps,
    })
}

/// Read the first video stream of `input` via `ffmpeg -i` (None = unparsable).
pub async fn probe_video_stream(ffmpeg: &Path, input: &Path) -> Option<VideoProbe> {
    let out = tokio::process::Command::new(ffmpeg)
        .args(["-hide_banner", "-i", &input.to_string_lossy()])
        .output()
        .await
        .ok()?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    stderr
        .lines()
        .filter(|l| l.contains("Video:"))
        .find_map(parse_video_stream_line)
}

#[cfg(test)]
mod tests {
    use super::{make_thumbnail, parse_video_stream_line};

    #[test]
    fn video_stream_line_parses_obs_clips() {
        let p = parse_video_stream_line(
            "  Stream #0:0(und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(tv, bt709, progressive), 1920x1080 [SAR 1:1 DAR 16:9], 19427 kb/s, 60 fps, 60 tbr, 15360 tbn (default)",
        )
        .unwrap();
        assert_eq!(p.codec_name, "h264");
        assert_eq!(p.profile, "High");
        assert_eq!((p.width, p.height), (1920, 1080));
        assert!((p.fps - 60.0).abs() < 1e-9);

        let p = parse_video_stream_line(
            "Stream #0:0: Video: hevc (Main 10), yuv420p10le(tv, bt2020nc/bt2020/smpte2084), 3840x2160, 29.97 fps, 29.97 tbr, 1k tbn",
        )
        .unwrap();
        assert_eq!(p.codec_name, "hevc");
        assert_eq!(p.profile, "Main 10");
        assert_eq!((p.width, p.height), (3840, 2160));
        assert!((p.fps - 29.97).abs() < 1e-9);

        // No profile group: profile stays empty, dims/fps still parse.
        let p = parse_video_stream_line(
            "Stream #0:0: Video: av1, yuv420p(tv, bt709, progressive), 2560x1440, 120 fps, 120 tbr, 15360 tbn",
        )
        .unwrap();
        assert_eq!(p.codec_name, "av1");
        assert_eq!(p.profile, "");
        assert_eq!((p.width, p.height), (2560, 1440));

        // Audio lines and garbage never parse as video.
        assert!(parse_video_stream_line(
            "Stream #0:1(und): Audio: aac (mp4a / 0x6134706D), 48000 Hz, stereo, fltp, 320 kb/s (default)"
        )
        .is_none());
        assert!(parse_video_stream_line("Duration: 00:00:10.00").is_none());
        assert!(parse_video_stream_line("Stream #0:0: Video: vp9, 640x480").is_none());
    }

    /// Regression: thumbnails must work on limited-range yuv420p (what NVENC
    /// and swscale produce), where ffmpeg 9's mjpeg encoder is strict.
    /// Hermetic (lavfi + libx264, no HW); skips loudly without ffmpeg.
    #[tokio::test]
    async fn thumbnail_limited_range() {
        let ffmpeg = super::PathBuf::from("ffmpeg");
        let probe = tokio::process::Command::new(&ffmpeg)
            .args(["-hide_banner", "-version"])
            .output()
            .await;
        if !probe.map(|o| o.status.success()).unwrap_or(false) {
            eprintln!("[moonclip-test] ffmpeg missing from PATH, skipping thumbnail test");
            return;
        }
        let dir = std::env::temp_dir().join(format!("moonclip-thumb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("limited.mp4");
        let thumb = dir.join("thumb.jpg");
        // 3 s of limited-range yuv420p h264 (swscale default range, like NVENC).
        let st = tokio::process::Command::new(&ffmpeg)
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=320x240:d=3",
                "-vf",
                "format=yuv420p",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-color_range",
                "tv",
            ])
            .arg(&clip)
            .status()
            .await
            .expect("fixture encode");
        assert!(st.success(), "fixture encode failed");
        make_thumbnail(&ffmpeg, &clip, &thumb, 1.0)
            .await
            .expect("thumbnail on limited-range input");
        let size = std::fs::metadata(&thumb).map(|m| m.len()).unwrap_or(0);
        assert!(size > 0, "thumbnail is empty");
        // Sub-second clip: adaptive seek must still deliver.
        make_thumbnail(&ffmpeg, &clip, &thumb, 0.2)
            .await
            .expect("thumbnail at 0.2 s");
        std::fs::remove_dir_all(&dir).ok();
    }
}
