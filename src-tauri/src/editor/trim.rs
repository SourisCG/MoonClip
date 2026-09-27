//! Quick trim (Phase 5 light path): one ffmpeg run, no editor, no extra libs.
//!
//! `lossless` copies streams (sub-second, cuts at the previous keyframe;
//! tolerance ~0–2 s). `precise` re-encodes video with libx264 and audio with
//! AAC (frame-exact, keeps all audio tracks of the clip). The gallery panel
//! picks the mode; this module owns the args and the run.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};

/// How the head of the cut is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrimMode {
    Lossless,
    Precise,
}

/// One trim job (input/output/timeline/mode) for `run_trim`.
pub struct TrimSpec<'a> {
    pub clip_id: &'a str,
    pub input: &'a Path,
    pub output: &'a Path,
    pub start_ms: i64,
    pub end_ms: i64,
    pub mode: TrimMode,
}

/// Progress event payload for the gallery trim panel.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditProgress {
    pub op: &'static str,
    pub clip_id: String,
    pub percent: f64,
    pub done: bool,
}

/// Seconds with 3 decimals (`1234` ms -> `"1.234"`).
fn secs_arg(ms: i64) -> String {
    format!("{:.3}", (ms.max(0) as f64) / 1000.0)
}

fn is_mp4(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()),
        Some(ref e) if e == "mp4" || e == "mov" || e == "m4v"
    )
}

/// Full ffmpeg argument list for a trim run (pure; unit-tested).
pub fn build_trim_args(
    input: &Path,
    output: &Path,
    start_ms: i64,
    end_ms: i64,
    mode: TrimMode,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-nostdin".into(),
        "-loglevel".into(),
        "error".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-ss".into(),
        secs_arg(start_ms),
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-t".into(),
        secs_arg(end_ms - start_ms),
    ];
    match mode {
        TrimMode::Lossless => {
            args.extend(["-c", "copy"].map(String::from));
            args.push("-avoid_negative_ts".into());
            args.push("make_zero".into());
        }
        TrimMode::Precise => {
            // Keep the video plus every audio track the clip has (Mix/Game/Mic).
            args.extend(["-map", "0:v:0", "-map", "0:a?"].map(String::from));
            args.extend(
                [
                    "-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p",
                    "-c:a", "aac", "-b:a", "192k",
                ]
                .map(String::from),
            );
        }
    }
    if is_mp4(output) {
        args.push("-movflags".into());
        args.push("+faststart".into());
    }
    args.push(output.to_string_lossy().into_owned());
    args
}

/// Run one trim, emitting `moonclip://edit-progress` while it works.
pub async fn run_trim(app: &AppHandle, ffmpeg: &Path, spec: &TrimSpec<'_>) -> Result<(), String> {
    let TrimSpec {
        clip_id,
        input,
        output,
        start_ms,
        end_ms,
        mode,
    } = *spec;
    let mut cmd = tokio::process::Command::new(ffmpeg);
    cmd.args(build_trim_args(input, output, start_ms, end_ms, mode))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot start ffmpeg: {e}"))?;

    let total = (end_ms - start_ms).max(1) as f64;
    let mut last_emit = 0.0f64;
    if let Some(stdout) = child.stdout.take() {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            // `-progress` emits `out_time_us=…`/`out_time_ms=…` (µs) each ~0.5s.
            let value = line
                .strip_prefix("out_time_us=")
                .or_else(|| line.strip_prefix("out_time_ms="));
            if let Some(v) = value {
                if let Ok(us) = v.trim().parse::<f64>() {
                    let percent = (us / 1000.0 / total * 100.0).clamp(0.0, 99.0);
                    if percent - last_emit >= 1.0 {
                        last_emit = percent;
                        let _ = app.emit(
                            "moonclip://edit-progress",
                            EditProgress {
                                op: "trim",
                                clip_id: clip_id.to_string(),
                                percent,
                                done: false,
                            },
                        );
                    }
                }
            }
        }
    }
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| format!("ffmpeg wait failed: {e}"))?;
    let _ = app.emit(
        "moonclip://edit-progress",
        EditProgress {
            op: "trim",
            clip_id: clip_id.to_string(),
            percent: 100.0,
            done: out.status.success(),
        },
    );
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: String = err
            .lines()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" | ");
        return Err(format!("ffmpeg trim failed: {tail}"));
    }
    Ok(())
}

/// Free `<stem>_trim.<ext>` (then `_trim_2`, `_trim_3`…) in `base`.
pub fn unique_trim_path(
    base: &Path,
    stem: &str,
    ext: &str,
    taken: &std::collections::HashSet<String>,
) -> PathBuf {
    for n in 1u32..1000 {
        let name = if n == 1 {
            format!("{stem}_trim.{ext}")
        } else {
            format!("{stem}_trim_{n}.{ext}")
        };
        let cand = base.join(&name);
        if !taken.contains(&name) && !cand.exists() {
            return cand;
        }
    }
    base.join(format!("{stem}_trim_{}.{}", uuid::Uuid::new_v4().simple(), ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(input: &str, output: &str, mode: TrimMode) -> Vec<String> {
        build_trim_args(Path::new(input), Path::new(output), 1_234, 5_678, mode)
    }

    #[test]
    fn lossless_copies_streams_and_keeps_audio() {
        let a = args("/clips/in.mp4", "/clips/out.mp4", TrimMode::Lossless);
        assert!(a.windows(2).any(|w| w == ["-ss", "1.234"]));
        assert!(a.windows(2).any(|w| w == ["-t", "4.444"]));
        assert!(a.windows(2).any(|w| w == ["-c", "copy"]));
        // copy keeps every stream: no explicit map/codec overrides.
        assert!(!a.iter().any(|x| x == "-map"));
        assert!(a.windows(2).any(|w| w == ["-movflags", "+faststart"]));
        assert_eq!(a.last().unwrap(), "/clips/out.mp4");
    }

    #[test]
    fn precise_reencodes_and_keeps_all_audio_tracks() {
        let a = args("/clips/in.mkv", "/clips/out.mkv", TrimMode::Precise);
        assert!(a.windows(2).any(|w| w == ["-map", "0:v:0"]));
        assert!(a.windows(2).any(|w| w == ["-map", "0:a?"]));
        assert!(a.windows(2).any(|w| w == ["-c:v", "libx264"]));
        assert!(a.windows(2).any(|w| w == ["-c:a", "aac"]));
        // mkv does not take movflags.
        assert!(!a.iter().any(|x| x == "-movflags"));
    }

    /// Acceptance (E1): trims a real 3 s clip with the bundled sidecar.
    /// Lossless cuts are keyframe-aligned (a small head bonus is expected,
    /// documented as ~0–2 s); precise ones are frame-exact. Skips when the
    /// sidecar is not staged.
    #[tokio::test]
    async fn live_trim_with_bundled_ffmpeg() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let ff = manifest
            .join("binaries")
            .join(crate::sidecar::host_triple())
            .join(crate::editor::ffmpeg::sidecar_name());
        if !ff.exists() {
            eprintln!("skip: bundled ffmpeg not staged at {}", ff.display());
            return;
        }
        let dir = std::env::temp_dir().join(format!("moonclip-trim-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.mp4");
        let gen = tokio::process::Command::new(&ff)
            .args([
                "-y", "-hide_banner", "-loglevel", "error",
                "-f", "lavfi", "-i", "testsrc=size=320x240:rate=30",
                "-f", "lavfi", "-i", "sine=frequency=440",
                "-t", "3", "-c:v", "libx264", "-preset", "ultrafast",
                "-g", "30", "-c:a", "aac",
            ])
            .arg(&src)
            .status()
            .await
            .unwrap();
        assert!(gen.success(), "cannot generate fixture");

        let lossless = dir.join("lossless.mp4");
        let status = tokio::process::Command::new(&ff)
            .args(build_trim_args(&src, &lossless, 500, 1500, TrimMode::Lossless))
            .status()
            .await
            .unwrap();
        assert!(status.success(), "lossless trim failed");
        let ms = crate::editor::ffmpeg::probe_duration_ms(&ff, &lossless)
            .await
            .expect("probe lossless");
        // Selection is 1000 ms; copy snaps the head to the previous keyframe
        // (≤1 s here), so anything between exact and selection+1.5 s is valid.
        assert!((900..=2500).contains(&ms), "lossless duration {ms} ms");

        let precise = dir.join("precise.mp4");
        let status = tokio::process::Command::new(&ff)
            .args(build_trim_args(&src, &precise, 500, 1500, TrimMode::Precise))
            .status()
            .await
            .unwrap();
        assert!(status.success(), "precise trim failed");
        let ms = crate::editor::ffmpeg::probe_duration_ms(&ff, &precise)
            .await
            .expect("probe precise");
        assert!((850..=1200).contains(&ms), "precise duration {ms} ms");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unique_name_avoids_db_and_disk_collisions() {
        let base = std::env::temp_dir().join("moonclip-trim-test");
        let mut taken = std::collections::HashSet::new();
        taken.insert("clip_trim.mp4".to_string());
        let p = unique_trim_path(&base, "clip", "mp4", &taken);
        assert_eq!(p.file_name().unwrap(), "clip_trim_2.mp4");
    }
}
