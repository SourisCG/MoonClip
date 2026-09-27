//! Staged export: per-segment video (copy when possible) -> concat ->
//! three-stem audio mix -> single AAC track. Progress and cancel are wired
//! through the session handle; the result is indexed as a new clip.
//!
//! Staged rendering keeps every ffmpeg command small and debuggable, lets the
//! encoder fall back to CPU independently per stage and makes cancellation
//! reliable (one child at a time).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};

use super::encoders;
use super::project::{EditProject, OutputSettings, Segment};
use super::session;

fn secs(ms: i64) -> String {
    format!("{:.3}", ms.max(0) as f64 / 1000.0)
}

fn even(v: u32) -> u32 {
    v.max(2) & !1
}

/// Target output size from the source and the preset. `out.height` is the
/// quality preset (the "1080" in 1080p): for 9:16 it is the WIDTH, so a
/// "1080 vertical" export is 1080x1920, like Medal.
pub fn output_dims(src_w: u32, src_h: u32, out: &OutputSettings) -> (u32, u32) {
    let quality = out.height;
    let (src_w, src_h) = (src_w.max(2), src_h.max(2));
    match out.aspect.as_str() {
        "16:9" => {
            let w = if quality > 0 {
                even((quality as f64 * 16.0 / 9.0) as u32)
            } else {
                src_w
            };
            (even(w), even((w as f64 * 9.0 / 16.0) as u32))
        }
        "9:16" => {
            let w = if quality > 0 { even(quality) } else { src_h };
            (even(w), even((w as f64 * 16.0 / 9.0) as u32))
        }
        "1:1" => {
            let s = if quality > 0 {
                even(quality)
            } else {
                even(src_w.min(src_h))
            };
            (s, s)
        }
        _ => {
            let h = if quality > 0 { even(quality) } else { src_h };
            (even((src_w as f64 * h as f64 / src_h as f64) as u32), h)
        }
    }
}

/// Stage A: one trimmed (optionally re-encoded/scaled) video-only segment.
#[allow(clippy::too_many_arguments)]
pub fn stage_a_args(
    input: &Path,
    output: &Path,
    segment: &Segment,
    dims: (u32, u32),
    src_dims: (u32, u32),
    fps: u32,
    encoder: &str,
    bitrate_kbps: u32,
) -> Vec<String> {
    let needs_scale = dims != src_dims;
    let needs_speed = (segment.speed - 1.0).abs() > f64::EPSILON;
    let needs_fps = fps > 0;
    let copy = !needs_scale && !needs_speed && !needs_fps && segment.freeze_ms <= 0;

    let mut a: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-nostdin".into(),
        "-loglevel".into(),
        "error".into(),
        "-ss".into(),
        secs(segment.in_ms),
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-t".into(),
        secs(segment.out_ms - segment.in_ms),
        "-an".into(),
    ];
    if copy {
        a.extend(["-c:v", "copy"].map(String::from));
    } else {
        let mut vf: Vec<String> = Vec::new();
        if needs_scale {
            // Cover + center crop (never letterbox a game clip).
            vf.push(format!(
                "scale={}:{}:force_original_aspect_ratio=increase,crop={}:{}",
                dims.0, dims.1, dims.0, dims.1
            ));
        }
        if needs_speed {
            vf.push(format!("setpts=PTS/{}", segment.speed));
        }
        if !vf.is_empty() {
            a.push("-vf".into());
            a.push(vf.join(","));
        }
        if needs_fps {
            a.push("-r".into());
            a.push(fps.to_string());
        }
        a.push("-pix_fmt".into());
        a.push("yuv420p".into());
        a.extend(encoders::quality_args(encoder, bitrate_kbps, dims.1));
        a.extend(["-c:v", encoder].map(String::from));
    }
    if output
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mp4"))
    {
        a.extend(["-movflags", "+faststart"].map(String::from));
    }
    a.push(output.to_string_lossy().into_owned());
    a
}

/// concat demuxer file body (one `file '...'` per segment).
pub fn concat_file(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| format!("file '{}'", p.to_string_lossy().replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Audio filtergraph: every segment contributes its available stems at its
/// timeline position, gains preserved (`normalize=0`), then a single track.
/// Returns the graph and whether any audio exists.
pub fn audio_filter(
    segments: &[Segment],
    input_index: &[usize],
    tracks_per_input: &[usize],
    total_ms: i64,
) -> (String, bool) {
    let mut chains: Vec<String> = Vec::new();
    let mut labels: Vec<String> = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        let idx = input_index[i];
        let tracks = tracks_per_input[i].min(3);
        let gains = [seg.gain_mix, seg.gain_game, seg.gain_mic];
        let start = seg.timeline_start_ms.max(0);
        if tracks == 0 {
            continue;
        }
        for (t, gain) in gains.iter().enumerate().take(tracks) {
            let label = format!("a{i}_{t}");
            let gain = gain.clamp(0.0, 4.0);
            let delay = if start > 0 {
                format!(",adelay={}|{}", start, start)
            } else {
                String::new()
            };
            chains.push(format!(
                "[{idx}:a:{t}]atrim=start={}:end={},asetpts=PTS-STARTPTS,volume={gain:.3}{delay}[{label}]",
                secs(seg.in_ms),
                secs(seg.out_ms),
            ));
            labels.push(format!("[{label}]"));
        }
    }
    if labels.is_empty() {
        return (String::new(), false);
    }
    chains.push(format!(
        "{}amix=inputs={}:duration=longest:normalize=0,atrim=0:{},asetpts=PTS-STARTPTS[aout]",
        labels.join(""),
        labels.len(),
        secs(total_ms)
    ));
    (chains.join(";"), true)
}

/// Free `<stem>_edit.mp4` (then `_edit_2`…) in the clips dir.
pub fn unique_edit_name(
    base: &Path,
    stem: &str,
    taken: &std::collections::HashSet<String>,
) -> String {
    for n in 1u32..1000 {
        let name = if n == 1 {
            format!("{stem}_edit.mp4")
        } else {
            format!("{stem}_edit_{n}.mp4")
        };
        if !taken.contains(&name) && !base.join(&name).exists() {
            return name;
        }
    }
    format!("{stem}_edit_{}.mp4", uuid::Uuid::new_v4().simple())
}

/// Run ffmpeg with `-progress` parsing; the child is stored in the session
/// handle so `editor_cancel_export` can kill it. `total_seconds` maps
/// `out_time_us` to percent.
async fn run_ffmpeg(
    ffmpeg: &Path,
    args: &[String],
    handle: &Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>,
    total_seconds: f64,
    mut on_percent: impl FnMut(f64),
) -> Result<(), String> {
    let mut cmd = tokio::process::Command::new(ffmpeg);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("cannot start ffmpeg: {e}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let errbuf: Arc<std::sync::Mutex<String>> = Arc::new(std::sync::Mutex::new(String::new()));
    if let Some(err) = stderr {
        let buf = errbuf.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(mut b) = buf.lock() {
                    if b.len() < 16_384 {
                        b.push_str(&line);
                        b.push('\n');
                    }
                }
            }
        });
    }

    {
        let mut guard = handle.lock().await;
        if let Some(mut prev) = guard.take() {
            let _ = prev.kill().await;
        }
        *guard = Some(child);
    }

    if let Some(out) = stdout {
        let mut lines = BufReader::new(out).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let value = line
                .strip_prefix("out_time_us=")
                .or_else(|| line.strip_prefix("out_time_ms="));
            if let Some(v) = value {
                if let Ok(us) = v.trim().parse::<f64>() {
                    let pct = (us / 1_000_000.0 / total_seconds * 100.0).clamp(0.0, 99.9);
                    on_percent(pct);
                }
            }
        }
    }

    let status = {
        let mut guard = handle.lock().await;
        match guard.as_mut() {
            Some(c) => c.wait().await.map_err(|e| format!("ffmpeg wait failed: {e}"))?,
            None => return Err("export cancelled".into()),
        }
    };
    {
        let mut guard = handle.lock().await;
        *guard = None;
    }
    if !status.success() {
        let tail: String = errbuf
            .lock()
            .map(|b| {
                b.lines()
                    .rev()
                    .take(3)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join(" | ")
            })
            .unwrap_or_default();
        return Err(format!("ffmpeg failed: {tail}"));
    }
    Ok(())
}

/// Export `project` for a session. Emits `moonclip://edit-progress` and
/// indexes the result like a normal clip.
pub async fn run(app: &AppHandle, session_id: &str, project: &EditProject) -> Result<(), String> {
    let (_id, dir) = session::get(session_id).ok_or("editor session not found")?;
    let (base, clips, ffmpeg) = {
        let db = app.state::<crate::storage::DbState>();
        let base = db.clips_dir()?;
        let clips = db.list_clips()?;
        let ffmpeg = crate::editor::ffmpeg::resolve_ffmpeg(app)?;
        (base, clips, ffmpeg)
    };
    if project.segments.is_empty() {
        return Err("the project has no clips".into());
    }

    // Resolve every source clip referenced by the project (positional).
    let mut sources: Vec<(String, PathBuf, i64, u32, u32, f64, usize)> = Vec::new();
    for seg in &project.segments {
        if sources.iter().any(|(id, ..)| id == &seg.source_clip_id) {
            continue;
        }
        let clip = clips
            .iter()
            .find(|c| c.id == seg.source_clip_id)
            .ok_or_else(|| format!("source clip {} not found", seg.source_clip_id))?;
        let path = crate::commands::validated_media_path(&base, &clip.file_name)?;
        let probe = crate::editor::ffmpeg::probe_video_stream(&ffmpeg, &path).await;
        let (w, h, fps) = probe
            .map(|p| (p.width, p.height, p.fps))
            .unwrap_or((1920, 1080, 60.0));
        let stderr = tokio::process::Command::new(&ffmpeg)
            .args(["-hide_banner", "-i"])
            .arg(&path)
            .output()
            .await
            .map(|o| String::from_utf8_lossy(&o.stderr).to_string())
            .unwrap_or_default();
        let tracks = session::parse_audio_track_count(&stderr).min(3);
        sources.push((seg.source_clip_id.clone(), path, clip.duration_ms, w, h, fps, tracks));
    }
    let (src_w, src_h) = (sources[0].3, sources[0].4);
    let (out_w, out_h) = output_dims(src_w, src_h, &project.output);
    let fps = project.output.fps;
    let encoder = encoders::pick(&ffmpeg, &project.output.encoder).await;
    eprintln!(
        "[moonclip] editor export: {out_w}x{out_h} encoder={encoder} segments={}",
        project.segments.len()
    );

    let total_ms = project.duration_ms().max(100);
    let emit = |percent: f64, stage: &str, done: bool| {
        let _ = app.emit(
            "moonclip://edit-progress",
            serde_json::json!({
                "op": "export",
                "clipId": project.source_clip_id,
                "sessionId": session_id,
                "stage": stage,
                "percent": percent.clamp(0.0, 100.0),
                "done": done,
            }),
        );
    };
    emit(0.0, "segments", false);
    let handle = session::export_handle(session_id).ok_or("editor session not found")?;

    // ---- Stage A: one video-only file per segment ------------------------
    let mut segment_files: Vec<PathBuf> = Vec::new();
    for (i, seg) in project.segments.iter().enumerate() {
        let src = sources
            .iter()
            .find(|(id, ..)| id == &seg.source_clip_id)
            .ok_or("source clip not resolved")?;
        let out = dir.join(format!("seg_{i}.mp4"));
        let args = stage_a_args(
            &src.1,
            &out,
            seg,
            (out_w, out_h),
            (src_w, src_h),
            fps,
            &encoder,
            project.output.bitrate_kbps,
        );
        let seg_seconds = (seg.out_ms - seg.in_ms).max(1) as f64 / 1000.0;
        let seg_count = project.segments.len() as f64;
        run_ffmpeg(&ffmpeg, &args, &handle, seg_seconds, |p| {
            emit((i as f64 + p / 100.0) / seg_count * 55.0, "segments", false);
        })
        .await
        .map_err(|e| format!("segment {} failed: {e}", i + 1))?;
        if !out.is_file() {
            return Err(format!("segment {} produced no file", i + 1));
        }
        segment_files.push(out);
    }

    // ---- Stage B: concat + audio mix ------------------------------------
    emit(60.0, "timeline", false);
    let video_in: PathBuf = if segment_files.len() == 1 {
        segment_files[0].clone()
    } else {
        let list = dir.join("concat.txt");
        std::fs::write(&list, concat_file(&segment_files))
            .map_err(|e| format!("cannot write concat list: {e}"))?;
        let out = dir.join("joined.mp4");
        let status = tokio::process::Command::new(&ffmpeg)
            .args([
                "-y", "-hide_banner", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i",
            ])
            .arg(&list)
            .args(["-c", "copy"])
            .arg(&out)
            .status()
            .await
            .map_err(|e| format!("concat failed: {e}"))?;
        if !status.success() || !out.is_file() {
            return Err("video concat failed".into());
        }
        out
    };

    let input_index: Vec<usize> = project
        .segments
        .iter()
        .map(|seg| {
            1 + sources
                .iter()
                .position(|(id, ..)| id == &seg.source_clip_id)
                .unwrap_or(0)
        })
        .collect();
    let tracks_per_input: Vec<usize> = project
        .segments
        .iter()
        .map(|seg| {
            let idx = sources
                .iter()
                .position(|(id, ..)| id == &seg.source_clip_id)
                .unwrap_or(0);
            sources[idx].6
        })
        .collect();
    let (filter, has_audio) =
        audio_filter(&project.segments, &input_index, &tracks_per_input, total_ms);

    let stem = clips
        .iter()
        .find(|c| c.id == sources[0].0)
        .and_then(|c| Path::new(&c.file_name).file_stem().and_then(|s| s.to_str()))
        .unwrap_or("clip")
        .to_string();
    let taken: std::collections::HashSet<String> =
        clips.iter().map(|c| c.file_name.clone()).collect();
    let out_name = unique_edit_name(&base, &stem, &taken);
    let output = base.join(&out_name);

    let mut args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-nostdin".into(),
        "-loglevel".into(),
        "error".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-i".into(),
        video_in.to_string_lossy().into_owned(),
    ];
    for (_, path, ..) in &sources {
        args.push("-i".into());
        args.push(path.to_string_lossy().into_owned());
    }
    if has_audio {
        args.extend(["-filter_complex", &filter, "-map", "0:v:0", "-map", "[aout]"].map(String::from));
    } else {
        args.extend(["-map", "0:v:0", "-an"].map(String::from));
    }
    args.extend(["-c:v", "copy"].map(String::from));
    if has_audio {
        args.extend(["-c:a", "aac", "-b:a", "192k"].map(String::from));
    }
    if project.output.container != "mkv" {
        args.extend(["-movflags", "+faststart"].map(String::from));
    }
    args.push(output.to_string_lossy().into_owned());

    run_ffmpeg(
        &ffmpeg,
        &args,
        &handle,
        total_ms as f64 / 1000.0,
        |p| emit(60.0 + p * 0.4, "timeline", false),
    )
    .await?;
    if !output.is_file() {
        return Err("export produced no output file".into());
    }

    // ---- Index the result as a new clip ---------------------------------
    let out_stem = output
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("bad output name")?
        .to_string();
    let thumb_name = format!("thumb_{out_stem}.jpg");
    let thumb_path = base.join(&thumb_name);
    if let Err(e) =
        crate::editor::ffmpeg::make_thumbnail(&ffmpeg, &output, &thumb_path, 0.3).await
    {
        let _ = tokio::fs::remove_file(&output).await;
        return Err(e);
    }
    let size = tokio::fs::metadata(&output)
        .await
        .map_err(|e| format!("cannot stat export: {e}"))?
        .len() as i64;
    let duration_ms = crate::editor::ffmpeg::probe_duration_ms(&ffmpeg, &output)
        .await
        .unwrap_or(total_ms);
    let game_title = clips
        .iter()
        .find(|c| c.id == sources[0].0)
        .map(|c| c.game_title.clone())
        .unwrap_or_else(|| "Unknown".into());
    let db = app.state::<crate::storage::DbState>();
    let record = db.insert_clip(&out_name, &thumb_name, &game_title, duration_ms, size)?;
    emit(100.0, "timeline", true);
    let _ = app.emit("moonclip://clip-saved", &record);
    eprintln!("[moonclip] editor export done: {out_name}");
    crate::cue::play_ding();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::project::default_project;

    fn seg(in_ms: i64, out_ms: i64) -> Segment {
        default_project("c", "n", 1000).segments.remove(0).tap(|s| {
            s.in_ms = in_ms;
            s.out_ms = out_ms;
        })
    }

    trait Tap: Sized {
        fn tap(mut self, f: impl FnOnce(&mut Self)) -> Self {
            f(&mut self);
            self
        }
    }
    impl<T> Tap for T {}

    #[test]
    fn output_dims_follow_preset_and_aspect() {
        let mut o = OutputSettings::default();
        assert_eq!(output_dims(2560, 1440, &o), (2560, 1440));
        o.height = 1080;
        assert_eq!(output_dims(2560, 1440, &o), (1920, 1080));
        o.aspect = "9:16".into();
        assert_eq!(output_dims(2560, 1440, &o), (1080, 1920));
        o.aspect = "1:1".into();
        assert_eq!(output_dims(2560, 1440, &o), (1080, 1080));
    }

    #[test]
    fn stage_a_copies_when_nothing_changes() {
        let a = stage_a_args(
            Path::new("/clips/in.mp4"),
            Path::new("/tmp/seg.mp4"),
            &seg(1000, 2500),
            (1920, 1080),
            (1920, 1080),
            0,
            "libx264",
            0,
        );
        assert!(a.windows(2).any(|w| w == ["-c:v", "copy"]));
        assert!(a.windows(2).any(|w| w == ["-ss", "1.000"]));
        assert!(a.windows(2).any(|w| w == ["-t", "1.500"]));
        assert!(!a.iter().any(|x| x == "-vf"));
    }

    #[test]
    fn stage_a_reencodes_for_scale_speed_and_fps() {
        let mut s = seg(0, 2000);
        s.speed = 2.0;
        let a = stage_a_args(
            Path::new("/clips/in.mp4"),
            Path::new("/tmp/seg.mp4"),
            &s,
            (1080, 1920),
            (1920, 1080),
            30,
            "h264_nvenc",
            8000,
        );
        let vf = a
            .windows(2)
            .find(|w| w[0] == "-vf")
            .map(|w| w[1].clone())
            .unwrap();
        assert!(vf.contains("scale=1080:1920"));
        assert!(vf.contains("crop=1080:1920"));
        assert!(vf.contains("setpts=PTS/2"));
        assert!(a.windows(2).any(|w| w == ["-r", "30"]));
        assert!(a.windows(2).any(|w| w == ["-c:v", "h264_nvenc"]));
    }

    #[test]
    fn audio_filter_mixes_three_stems_with_gains_and_delay() {
        let mut s = seg(1000, 2500);
        s.timeline_start_ms = 500;
        s.gain_mic = 1.5;
        let (graph, has) = audio_filter(&[s], &[1], &[3], 3000);
        assert!(has);
        assert!(graph.contains("[1:a:0]atrim=start=1.000:end=2.500"));
        assert!(graph.contains("volume=1.500,adelay=500|500[a0_2]"));
        assert!(graph.contains("amix=inputs=3:duration=longest:normalize=0"));
        assert!(graph.contains("atrim=0:3.000"));
    }

    #[test]
    fn audio_filter_without_tracks_reports_no_audio() {
        let (graph, has) = audio_filter(&[seg(0, 1000)], &[1], &[0], 1000);
        assert!(!has);
        assert!(graph.is_empty());
    }

    #[test]
    fn concat_file_escapes_quotes() {
        let body = concat_file(&[PathBuf::from("/tmp/a'b.mp4")]);
        assert!(body.contains("a'\\''b"));
    }

    /// Acceptance (E2): the real ffmpeg sidecar runs stage A + stage B and the
    /// result has one video + exactly one (mixed) audio track. Skips when the
    /// sidecar is not staged.
    #[tokio::test]
    async fn live_export_mixes_stems_to_one_track() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let ff = manifest
            .join("binaries")
            .join(crate::sidecar::host_triple())
            .join(crate::editor::ffmpeg::sidecar_name());
        if !ff.exists() {
            eprintln!("skip: bundled ffmpeg not staged");
            return;
        }
        let dir = std::env::temp_dir().join(format!("moonclip-export-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.mp4");
        let status = tokio::process::Command::new(&ff)
            .args([
                "-y", "-hide_banner", "-loglevel", "error",
                "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30",
                "-f", "lavfi", "-i", "sine=frequency=440",
                "-f", "lavfi", "-i", "sine=frequency=880",
                "-f", "lavfi", "-i", "sine=frequency=1320",
                "-map", "0:v", "-map", "1:a", "-map", "2:a", "-map", "3:a",
                "-t", "3", "-c:v", "libx264", "-preset", "ultrafast", "-g", "30",
                "-c:a", "aac",
            ])
            .arg(&src)
            .status()
            .await
            .unwrap();
        assert!(status.success(), "fixture encode failed");
        let stderr = tokio::process::Command::new(&ff)
            .args(["-hide_banner", "-i"])
            .arg(&src)
            .output()
            .await
            .unwrap();
        let tracks = session::parse_audio_track_count(&String::from_utf8_lossy(&stderr.stderr));
        assert_eq!(tracks, 3, "fixture must have 3 audio tracks");

        // Stage A: lossless trim slice.
        let seg_file = dir.join("seg.mp4");
        let a = stage_a_args(
            &src,
            &seg_file,
            &seg(1000, 2500),
            (640, 360),
            (640, 360),
            0,
            "libx264",
            0,
        );
        let ok = tokio::process::Command::new(&ff)
            .args(&a)
            .status()
            .await
            .unwrap();
        assert!(ok.success(), "stage A failed");

        // Stage B: mix the three stems into ONE track.
        let out = dir.join("out.mp4");
        let (filter, has) = audio_filter(&[seg(1000, 2500)], &[1], &[3], 1500);
        assert!(has);
        let ok = tokio::process::Command::new(&ff)
            .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(&seg_file)
            .args(["-i"])
            .arg(&src)
            .args(["-filter_complex", &filter, "-map", "0:v:0", "-map", "[aout]"])
            .args(["-c:v", "copy", "-c:a", "aac", "-b:a", "192k"])
            .arg(&out)
            .status()
            .await
            .unwrap();
        assert!(ok.success(), "stage B failed");

        let stderr = tokio::process::Command::new(&ff)
            .args(["-hide_banner", "-i"])
            .arg(&out)
            .output()
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&stderr.stderr);
        assert_eq!(session::parse_audio_track_count(&text), 1, "{text}");
        assert!(crate::editor::ffmpeg::parse_video_stream_line(&text).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unique_edit_name_avoids_collisions() {
        let taken: std::collections::HashSet<String> = ["clip_edit.mp4".to_string()].into();
        let name = unique_edit_name(Path::new("/tmp"), "clip", &taken);
        assert_eq!(name, "clip_edit_2.mp4");
    }
}
