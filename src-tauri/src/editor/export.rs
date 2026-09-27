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
/// Which piece of a (possibly frozen) segment stage A must render.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FreezePart {
    /// Whole segment, no freeze.
    Full,
    /// From `in` up to the freeze point, holding that frame for `len_ms`.
    Before { at_ms: i64, len_ms: i64 },
    /// From the freeze point to the end of the segment.
    After { at_ms: i64 },
}

#[allow(clippy::too_many_arguments)]
pub fn stage_a_args(
    input: &Path,
    output: &Path,
    segment: &Segment,
    dims: (u32, u32),
    src_dims: (u32, u32),
    fps: u32,
    freeze: FreezePart,
    force_scale: bool,
    encoder: &str,
    bitrate_kbps: u32,
) -> Vec<String> {
    // `force_scale` normalizes every segment of a multi-source project to the
    // output frame and fps so the concat never mixes resolutions/timebases.
    let needs_scale = dims != src_dims || force_scale;
    let needs_speed = (segment.speed - 1.0).abs() > f64::EPSILON;
    let needs_fps = fps > 0;
    // Clip adjustments (E4.3/E4.5/E4.7).
    let has_crop = segment.crop_left > 0.0
        || segment.crop_top > 0.0
        || segment.crop_right > 0.0
        || segment.crop_bottom > 0.0;
    let has_zoom = (segment.zoom - 1.0).abs() > f64::EPSILON;
    let has_pan = segment.offset_x.abs() > f64::EPSILON || segment.offset_y.abs() > f64::EPSILON;
    let has_rotation = segment.rotation.abs() > f64::EPSILON;
    let has_opacity = (segment.opacity - 1.0).abs() > f64::EPSILON;
    let has_eq = segment.brightness.abs() > f64::EPSILON
        || (segment.contrast - 1.0).abs() > f64::EPSILON
        || (segment.saturation - 1.0).abs() > f64::EPSILON
        || (segment.gamma - 1.0).abs() > f64::EPSILON;
    let has_temperature = segment.temperature.abs() > f64::EPSILON;
    let has_vignette = segment.vignette > 0.0;
    let has_adjust = has_crop
        || has_zoom
        || has_pan
        || has_rotation
        || has_opacity
        || has_eq
        || has_temperature
        || has_vignette;
    let speed = segment.speed.max(0.05);
    // Trim window and OUTPUT duration for this piece. `-t` is an output
    // option: with speed it must be the sped-up duration (and add the freeze
    // hold for the "before" piece), otherwise slow clips get truncated.
    let (ss_ms, out_seconds, freeze_len) = match freeze {
        FreezePart::Full => (
            segment.in_ms,
            (segment.out_ms - segment.in_ms).max(1) as f64 / speed / 1000.0,
            None,
        ),
        FreezePart::Before { at_ms, len_ms } => {
            let at = at_ms.clamp(segment.in_ms, segment.out_ms);
            (
                segment.in_ms,
                (at - segment.in_ms).max(0) as f64 / speed / 1000.0 + len_ms.max(0) as f64 / 1000.0,
                Some(len_ms.max(0)),
            )
        }
        FreezePart::After { at_ms } => {
            let at = at_ms.clamp(segment.in_ms, segment.out_ms);
            (
                at,
                (segment.out_ms - at).max(1) as f64 / speed / 1000.0,
                None,
            )
        }
    };
    let copy = !needs_scale
        && !needs_speed
        && !needs_fps
        && !has_adjust
        && !force_scale
        && freeze == FreezePart::Full;

    let mut a: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-nostdin".into(),
        "-loglevel".into(),
        "error".into(),
        "-ss".into(),
        secs(ss_ms),
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-t".into(),
        format!("{out_seconds:.3}"),
        "-an".into(),
    ];
    if copy {
        a.extend(["-c:v", "copy"].map(String::from));
    } else {
        let mut vf: Vec<String> = Vec::new();
        // 1) Source crop (fractions).
        if has_crop {
            let l = segment.crop_left.clamp(0.0, 0.49);
            let t = segment.crop_top.clamp(0.0, 0.49);
            let r = segment.crop_right.clamp(0.0, 0.49);
            let b = segment.crop_bottom.clamp(0.0, 0.49);
            vf.push(format!(
                "crop=iw*{:.4}:ih*{:.4}:iw*{l:.4}:ih*{t:.4}",
                1.0 - l - r,
                1.0 - t - b
            ));
        }
        // 2) Fit the output frame (crop to cover, never letterbox).
        if needs_scale || has_adjust {
            vf.push(format!(
                "scale={}:{}:force_original_aspect_ratio=increase,crop={}:{}",
                dims.0, dims.1, dims.0, dims.1
            ));
        }
        // 3) Zoom + pan around the center.
        if has_zoom || has_pan {
            let z = segment.zoom.clamp(1.0, 4.0);
            vf.push(format!("scale=iw*{z:.4}:ih*{z:.4}"));
            // The zoomed frame can shift at most half the extra size.
            let max_off_x = dims.0 as f64 * (z - 1.0) / 2.0;
            let max_off_y = dims.1 as f64 * (z - 1.0) / 2.0;
            let ox = (segment.offset_x.clamp(-1.0, 1.0) * max_off_x).clamp(-max_off_x, max_off_x);
            let oy = (segment.offset_y.clamp(-1.0, 1.0) * max_off_y).clamp(-max_off_y, max_off_y);
            vf.push(format!(
                "crop={}:{}:(iw-{})/2+({ox:.2}):(ih-{})/2+({oy:.2})",
                dims.0, dims.1, dims.0, dims.1
            ));
        }
        // 4) Rotation (same canvas, black corners).
        if has_rotation {
            let rad = segment.rotation * std::f64::consts::PI / 180.0;
            vf.push(format!("rotate={rad:.6}:ow=iw:oh=ih:c=black"));
        }
        // 5) Opacity (fades to black, which is what the preview shows).
        if has_opacity {
            vf.push("format=rgba".into());
            vf.push(format!(
                "colorchannelmixer=aa={:.4}",
                segment.opacity.clamp(0.0, 1.0)
            ));
            vf.push("format=yuv420p".into());
        }
        // 6) Adjustments.
        if has_eq {
            vf.push(format!(
                "eq=brightness={:.4}:contrast={:.4}:saturation={:.4}:gamma={:.4}",
                segment.brightness.clamp(-1.0, 1.0),
                segment.contrast.clamp(0.0, 3.0),
                segment.saturation.clamp(0.0, 3.0),
                segment.gamma.clamp(0.1, 10.0),
            ));
        }
        if has_temperature {
            let tt = segment.temperature.clamp(-1.0, 1.0) * 0.3;
            vf.push(format!("colorbalance=rs={tt:.4}:bs={:.4}", -tt));
        }
        if has_vignette {
            let angle = std::f64::consts::PI * 0.45 * segment.vignette.clamp(0.0, 1.0);
            vf.push(format!("vignette=angle={angle:.4}"));
        }
        if needs_speed {
            vf.push(format!("setpts=PTS/{}", segment.speed));
        }
        // Freeze hold: clone the last frame (the freeze point) for `len_ms`.
        if let Some(len) = freeze_len {
            if len > 0 {
                vf.push(format!(
                    "tpad=stop_mode=clone:stop_duration={:.3}",
                    len as f64 / 1000.0
                ));
            }
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

/// One ordered piece of the exported video timeline: a segment or a black
/// filler for a gap the user left between clips.
#[derive(Debug, PartialEq)]
pub enum TimelinePiece {
    Seg(usize),
    Black { duration_ms: i64 },
}

/// Expand the segments into timeline pieces, inserting black fillers for the
/// gaps (dragging a clip right leaves one; the preview shows black there and
/// the audio already respects it via `adelay`).
pub fn timeline_pieces(segments: &[Segment]) -> Vec<TimelinePiece> {
    let mut order: Vec<(i64, usize)> = segments
        .iter()
        .enumerate()
        .map(|(i, s)| (s.timeline_start_ms.max(0), i))
        .collect();
    order.sort_by_key(|(start, _)| *start);
    let mut pieces = Vec::new();
    let mut cursor = 0i64;
    for (start, i) in order {
        if start > cursor {
            pieces.push(TimelinePiece::Black {
                duration_ms: start - cursor,
            });
        }
        pieces.push(TimelinePiece::Seg(i));
        cursor = start + segments[i].timeline_duration_ms();
    }
    pieces
}

/// ffmpeg args for a silent black filler clip of `duration_ms`.
pub fn black_filler_args(
    output: &Path,
    dims: (u32, u32),
    fps: u32,
    duration_ms: i64,
    encoder: &str,
    bitrate_kbps: u32,
) -> Vec<String> {
    let fps = fps.max(1);
    let mut a: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-nostdin".into(),
        "-loglevel".into(),
        "error".into(),
        "-f".into(),
        "lavfi".into(),
        "-i".into(),
    ];
    a.push(format!(
        "color=c=black:s={}x{}:r={fps}",
        dims.0.max(2),
        dims.1.max(2)
    ));
    a.push("-t".into());
    a.push(secs(duration_ms));
    a.push("-an".into());
    a.extend(encoders::quality_args(encoder, bitrate_kbps, dims.1));
    a.extend(["-c:v", encoder, "-pix_fmt", "yuv420p"].map(String::from));
    a.push(output.to_string_lossy().into_owned());
    a
}

/// Bundled font for text overlays (OFL/redistributable). Searches the
/// packaged resource dir first, then the dev tree, then common system paths.
pub fn resolve_font(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(res) = app.path().resource_dir() {
        candidates.push(res.join("fonts").join("DejaVuSans.ttf"));
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fonts")
            .join("DejaVuSans.ttf"),
    );
    candidates.push(PathBuf::from(
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
    ));
    candidates.push(PathBuf::from(
        "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
    ));
    candidates.into_iter().find(|p| p.is_file())
}

/// Escape a value used inside an ffmpeg filter option (`'...'` quoting).
pub fn escape_filter_value(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// ASS color: `&HAABBGGRR` (ASS is alpha-first, BGR order). `opacity` is
/// the element opacity (1 = fully visible).
fn ass_color(color: &str, opacity: f64) -> String {
    let hex: String = color
        .trim_start_matches('#')
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(6)
        .collect();
    let (r, g, b) = if hex.len() == 6 {
        (
            u8::from_str_radix(&hex[0..2], 16).unwrap_or(255),
            u8::from_str_radix(&hex[2..4], 16).unwrap_or(255),
            u8::from_str_radix(&hex[4..6], 16).unwrap_or(255),
        )
    } else {
        (255, 255, 255)
    };
    let alpha = ((1.0 - opacity.clamp(0.0, 1.0)) * 255.0).round() as u8;
    format!("&H{alpha:02X}{b:02X}{g:02X}{r:02X}")
}

/// ASS timestamp `H:MM:SS.cc` (centiseconds).
fn ass_time(ms: i64) -> String {
    let total = ms.max(0);
    let cs = (total % 1000) / 10;
    let s = (total / 1000) % 60;
    let m = (total / 60_000) % 60;
    let h = total / 3_600_000;
    format!("{h}:{m:02}:{s:02}.{cs:02}")
}

/// Escape literal text for an ASS dialogue: newlines become hard breaks and
/// braces (override blocks) are escaped.
fn ass_text(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('{', "\\{")
        .replace('}', "\\}")
        .replace("\r\n", "\n")
        .replace(['\n', '\r'], "\\N")
}

/// Generate the ASS subtitle file for the text overlays: position, size,
/// color, outline, shadow, opacity and ROTATION (which `drawtext` cannot do).
pub fn ass_subtitles(
    overlays: &[&crate::editor::project::Overlay],
    out_w: u32,
    out_h: u32,
) -> String {
    let w = out_w.max(2);
    let h = out_h.max(2);
    let k = h as f64 / 1080.0;
    let mut s = String::new();
    s.push_str("[Script Info]\nScriptType: v4.00+\n");
    s.push_str(&format!("PlayResX: {w}\nPlayResY: {h}\nWrapStyle: 2\n\n"));
    s.push_str("[V4+ Styles]\n");
    s.push_str("Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n");
    // Alignment 5 = centered horizontally and vertically (\pos is the center).
    s.push_str("Style: Clip,DejaVu Sans,48,&H00FFFFFF,&H00FFFFFF,&H00000000,&H64000000,-1,0,0,0,100,100,0,0,1,0,0,5,0,0,0,1\n\n");
    s.push_str("[Events]\n");
    s.push_str("Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");
    for o in overlays {
        let text = o.text.as_deref().unwrap_or("");
        if text.trim().is_empty() {
            continue;
        }
        let scale = o.scale.clamp(0.05, 8.0);
        let fontsize = (o.font_size as f64 * k * scale).round().max(8.0);
        let outline = (o.stroke_width * k * scale).round().max(0.0);
        let shadow = if o.shadow { 2.0 } else { 0.0 };
        let x = o.x.clamp(0.0, 1.0) * w as f64;
        let y = o.y.clamp(0.0, 1.0) * h as f64;
        let start = ass_time(o.start_ms);
        let end = ass_time(o.start_ms.max(0) + o.duration_ms.max(100));
        // Inline override: size/colors per event (the style stays neutral).
        let alpha = ((1.0 - o.opacity.clamp(0.0, 1.0)) * 255.0).round() as u8;
        let tag = format!(
            "{{\\pos({x:.1},{y:.1})\\frz({:.1})\\fs{fontsize:.0}\\1c{}\\3c{}\\bord{outline:.0}\\shad{shadow:.0}\\alpha&H{alpha:02X}&}}",
            o.rotation,
            ass_color(&o.color, 1.0),
            ass_color(&o.stroke_color, 1.0),
        );
        s.push_str(&format!(
            "Dialogue: 0,{start},{end},Clip,,0,0,0,,{tag}{}\n",
            ass_text(text)
        ));
    }
    s
}

/// concat demuxer file body (one `file '...'` per segment).
pub fn concat_file(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| format!("file '{}'", p.to_string_lossy().replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Audio filtergraph matching the editor mixer: Mix, Game and Mic are three
/// independent channels, each contribution carrying its per-clip gain at its
/// timeline position.
///
/// Returns the graph plus the output labels to `-map`. `mode` is:
/// - `"mix"`: ONE AAC track with the three channels summed, exactly like the
///   preview plays them.
/// - `"tracks"`: three AAC tracks — Mix, Game, Mic — for re-editing.
pub fn audio_filter(
    segments: &[Segment],
    input_index: &[usize],
    tracks_per_input: &[usize],
    total_ms: i64,
    master: f64,
    mode: &str,
) -> (String, Vec<String>) {
    let tracks_mode = mode == "tracks";
    let mut chains: Vec<String> = Vec::new();
    let mut game_labels: Vec<String> = Vec::new();
    let mut mic_labels: Vec<String> = Vec::new();
    // Stream 0 of a multi-track recording (the Mix stem): only exported in
    // tracks mode, as its own third output.
    let mut mixstem_labels: Vec<String> = Vec::new();
    // Stream 0 of a single-track source: the only audio it has, used by both
    // modes.
    let mut fallback_labels: Vec<String> = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        let idx = input_index[i];
        let tracks = tracks_per_input[i].min(3);
        if tracks == 0 {
            continue;
        }
        let speed = seg.speed.clamp(0.05, 20.0);
        // Speed changes the visual length; the audio must follow (rubberband
        // keeps the pitch, unlike the preview's playbackRate).
        let tempo = if (speed - 1.0).abs() > f64::EPSILON {
            format!(",rubberband=tempo={speed:.4}")
        } else {
            String::new()
        };
        // Freeze splits the clip's audio in two contributions with a silent
        // hold in between (mirrors the preview and the video pieces).
        let freeze_at = seg.freeze_at_ms.min(seg.out_ms);
        let frozen = seg.freeze_ms > 0 && freeze_at > seg.in_ms;
        let parts: Vec<(i64, i64, i64)> = if frozen {
            vec![
                (seg.in_ms, freeze_at, seg.timeline_start_ms.max(0)),
                (
                    freeze_at,
                    seg.out_ms,
                    seg.timeline_start_ms.max(0)
                        + ((freeze_at - seg.in_ms) as f64 / speed) as i64
                        + seg.freeze_ms.max(0),
                ),
            ]
        } else {
            vec![(seg.in_ms, seg.out_ms, seg.timeline_start_ms.max(0))]
        };
        // (stream index, gain, output group) contributions for this clip.
        // Three INDEPENDENT channels: Mix, Game and Mic each carry their own
        // per-clip gain (the export mirrors exactly what the editor plays).
        let sources: Vec<(usize, f64, &str)> = if tracks == 1 {
            vec![(0, master * seg.gain_mix, "fallback")]
        } else {
            let mut v = vec![
                (0, master * seg.gain_mix, "mixstem"),
                (1, master * seg.gain_game, "game"),
            ];
            if tracks >= 3 {
                v.push((2, master * seg.gain_mic, "mic"));
            }
            v
        };
        for (p, (src_in, src_out, tl)) in parts.iter().enumerate() {
            let start = (*tl).max(0);
            let delay = if start > 0 {
                format!(",adelay={}|{}", start, start)
            } else {
                String::new()
            };
            for (stream, gain, group) in &sources {
                // Keep the historic label for unsplit clips (tests/goldens).
                let label = if frozen {
                    format!("a{i}p{p}_{stream}")
                } else {
                    format!("a{i}_{stream}")
                };
                chains.push(format!(
                    "[{idx}:a:{stream}]atrim=start={}:end={},asetpts=PTS-STARTPTS,volume={:.3}{tempo}{delay}[{label}]",
                    secs(*src_in),
                    secs(*src_out),
                    gain.clamp(0.0, 4.0),
                ));
                let lbl = format!("[{label}]");
                match *group {
                    "game" => game_labels.push(lbl),
                    "mic" => mic_labels.push(lbl),
                    "mixstem" => mixstem_labels.push(lbl),
                    _ => fallback_labels.push(lbl),
                }
            }
        }
    }
    if !tracks_mode {
        // ONE track with all three channels at their own per-clip gains (the
        // editor lets the user mix Mix/Game/Mic freely).
        let mut all = mixstem_labels;
        all.append(&mut fallback_labels);
        all.append(&mut game_labels);
        all.append(&mut mic_labels);
        if all.is_empty() {
            return (String::new(), Vec::new());
        }
        chains.push(amix_chain(&all, total_ms, "aout"));
        return (chains.join(";"), vec!["[aout]".into()]);
    }
    // Three separate outputs: Mix (the stem, plus single-track fallback),
    // Game and Mic. Only non-empty busses are mapped.
    let mut maps: Vec<String> = Vec::new();
    mixstem_labels.append(&mut fallback_labels);
    if !mixstem_labels.is_empty() {
        chains.push(amix_chain(&mixstem_labels, total_ms, "amix"));
        maps.push("[amix]".into());
    }
    if !game_labels.is_empty() {
        chains.push(amix_chain(&game_labels, total_ms, "agame"));
        maps.push("[agame]".into());
    }
    if !mic_labels.is_empty() {
        chains.push(amix_chain(&mic_labels, total_ms, "amic"));
        maps.push("[amic]".into());
    }
    (chains.join(";"), maps)
}

/// Sum every labelled contribution into `out`, keeping the timeline length.
fn amix_chain(labels: &[String], total_ms: i64, out: &str) -> String {
    format!(
        "{}amix=inputs={}:duration=longest:normalize=0,atrim=0:{},asetpts=PTS-STARTPTS[{out}]",
        labels.join(""),
        labels.len(),
        secs(total_ms)
    )
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
    let base_fps = sources[0].5.round().max(1.0) as u32;
    let (out_w, out_h) = output_dims(src_w, src_h, &project.output);
    // Timeline pieces: segments plus black fillers for gaps.
    let pieces = timeline_pieces(&project.segments);
    let has_gap = pieces.iter().any(|p| matches!(p, TimelinePiece::Black { .. }));
    let has_freeze = project.segments.iter().any(|s| {
        s.freeze_ms > 0 && s.freeze_at_ms > s.in_ms && s.freeze_at_ms < s.out_ms
    });
    // Uniform encoding is mandatory when several sources, gaps or freezes
    // coexist (the concat demuxer cannot mix resolutions/timebases/codecs).
    let mixed = sources.len() > 1;
    let force_scale = mixed || has_gap || has_freeze;
    let fps = if project.output.fps > 0 {
        project.output.fps
    } else if force_scale {
        base_fps
    } else {
        0
    };
    let encoder = encoders::pick(&ffmpeg, &project.output.encoder).await;
    eprintln!(
        "[moonclip] editor export: {out_w}x{out_h} encoder={encoder} segments={} pieces={}",
        project.segments.len(),
        pieces.len()
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

    // ---- Stage A: one video-only file per timeline piece -----------------
    let mut segment_files: Vec<PathBuf> = Vec::new();
    let piece_count = pieces.len() as f64;
    for (i, piece) in pieces.iter().enumerate() {
        // One piece may need more than one file: a frozen segment renders as
        // "before (with the held frame)" + "after".
        let mut jobs: Vec<(PathBuf, Vec<String>, f64)> = Vec::new();
        match piece {
            TimelinePiece::Black { duration_ms } => {
                let out = dir.join(format!("seg_{i}.mp4"));
                jobs.push((
                    out.clone(),
                    black_filler_args(
                        &out,
                        (out_w, out_h),
                        if fps > 0 { fps } else { base_fps },
                        *duration_ms,
                        &encoder,
                        project.output.bitrate_kbps,
                    ),
                    (*duration_ms).max(1) as f64 / 1000.0,
                ));
            }
            TimelinePiece::Seg(idx) => {
                let seg = &project.segments[*idx];
                let src = sources
                    .iter()
                    .find(|(id, ..)| id == &seg.source_clip_id)
                    .ok_or("source clip not resolved")?;
                let speed = seg.speed.max(0.05);
                let frozen = seg.freeze_ms > 0
                    && seg.freeze_at_ms > seg.in_ms
                    && seg.freeze_at_ms < seg.out_ms;
                let parts: Vec<(FreezePart, f64)> = if frozen {
                    vec![
                        (
                            FreezePart::Before {
                                at_ms: seg.freeze_at_ms,
                                len_ms: seg.freeze_ms,
                            },
                            ((seg.freeze_at_ms - seg.in_ms).max(0) as f64 / speed
                                + seg.freeze_ms.max(0) as f64)
                                / 1000.0,
                        ),
                        (
                            FreezePart::After {
                                at_ms: seg.freeze_at_ms,
                            },
                            (seg.out_ms - seg.freeze_at_ms).max(1) as f64 / speed / 1000.0,
                        ),
                    ]
                } else {
                    vec![(
                        FreezePart::Full,
                        (seg.out_ms - seg.in_ms).max(1) as f64 / speed / 1000.0,
                    )]
                };
                for (j, (part, seconds)) in parts.into_iter().enumerate() {
                    let out = if frozen {
                        dir.join(format!("seg_{i}_{j}.mp4"))
                    } else {
                        dir.join(format!("seg_{i}.mp4"))
                    };
                    jobs.push((
                        out.clone(),
                        stage_a_args(
                            &src.1,
                            &out,
                            seg,
                            (out_w, out_h),
                            (src.3, src.4),
                            fps,
                            part,
                            force_scale,
                            &encoder,
                            project.output.bitrate_kbps,
                        ),
                        seconds,
                    ));
                }
            }
        }
        for (out, args, seconds) in jobs {
            run_ffmpeg(&ffmpeg, &args, &handle, seconds, |p| {
                emit((i as f64 + p / 100.0) / piece_count * 55.0, "segments", false);
            })
            .await
            .map_err(|e| format!("piece {} failed: {e}", i + 1))?;
            if !out.is_file() {
                return Err(format!("piece {} produced no file", i + 1));
            }
            segment_files.push(out);
        }
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
    // No global master any more: the three channel gains are the whole mix.
    let (filter, audio_maps) = audio_filter(
        &project.segments,
        &input_index,
        &tracks_per_input,
        total_ms,
        1.0,
        &project.output.audio,
    );
    let has_audio = !audio_maps.is_empty();

    // Text overlays (E3a): one ASS subtitle file rendered by libass, which
    // (unlike drawtext) supports rotation, positioning and per-event styling.
    let texts: Vec<&crate::editor::project::Overlay> = project
        .overlays
        .iter()
        .filter(|o| {
            o.kind == "text"
                && o.text
                    .as_deref()
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false)
        })
        .collect();
    let mut vfilters: Vec<String> = Vec::new();
    if !texts.is_empty() {
        let font = resolve_font(app).ok_or_else(|| {
            "no font available for text overlays (expected fonts/DejaVuSans.ttf)".to_string()
        })?;
        let fonts_dir = font.parent().ok_or("font path has no directory")?;
        let ass_path = dir.join("overlays.ass");
        std::fs::write(&ass_path, ass_subtitles(&texts, out_w, out_h))
            .map_err(|e| format!("cannot write overlay subtitles: {e}"))?;
        vfilters.push(format!(
            "ass=filename='{}':fontsdir='{}'",
            escape_filter_value(&ass_path.to_string_lossy()),
            escape_filter_value(&fonts_dir.to_string_lossy()),
        ));
    }

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
    let mut graphs: Vec<String> = Vec::new();
    if !vfilters.is_empty() {
        graphs.push(format!("[0:v]{}[vout]", vfilters.join(",")));
    }
    if has_audio {
        graphs.push(filter.clone());
    }
    if !graphs.is_empty() {
        args.push("-filter_complex".into());
        args.push(graphs.join(";"));
    }
    if !vfilters.is_empty() {
        args.push("-map".into());
        args.push("[vout]".into());
    } else {
        args.push("-map".into());
        args.push("0:v:0".into());
    }
    for label in &audio_maps {
        args.push("-map".into());
        args.push(label.clone());
    }
    if vfilters.is_empty() {
        args.extend(["-c:v", "copy"].map(String::from));
    } else {
        args.push("-c:v".into());
        args.push(encoder.clone());
        args.extend(encoders::quality_args(
            &encoder,
            project.output.bitrate_kbps,
            out_h,
        ));
        args.extend(["-pix_fmt", "yuv420p"].map(String::from));
    }
    if has_audio {
        args.extend(["-c:a", "aac", "-b:a", "192k"].map(String::from));
    } else {
        args.push("-an".into());
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

            FreezePart::Full,
            false,
            "libx264",
            0,
        );
        assert!(a.windows(2).any(|w| w == ["-c:v", "copy"]));
        assert!(a.windows(2).any(|w| w == ["-ss", "1.000"]));
        assert!(a.windows(2).any(|w| w == ["-t", "1.500"]));
        assert!(!a.iter().any(|x| x == "-vf"));
    }

    #[test]
    fn timeline_pieces_insert_black_fillers_for_gaps() {
        let mut a = seg(0, 2000);
        a.timeline_start_ms = 0;
        let mut b = seg(0, 1000);
        b.timeline_start_ms = 3000;
        // Sorted by start even if the vector is not ordered.
        assert_eq!(
            timeline_pieces(&[b.clone(), a.clone()]),
            vec![
                TimelinePiece::Seg(1),
                TimelinePiece::Black { duration_ms: 1000 },
                TimelinePiece::Seg(0),
            ]
        );
        // Leading gap before the first clip.
        let mut c = seg(0, 1000);
        c.timeline_start_ms = 500;
        assert_eq!(
            timeline_pieces(&[c]),
            vec![
                TimelinePiece::Black { duration_ms: 500 },
                TimelinePiece::Seg(0)
            ]
        );
        // Contiguous clips starting at 0: no fillers.
        let mut d = seg(0, 1000);
        d.timeline_start_ms = 0;
        let mut e = seg(0, 1000);
        e.timeline_start_ms = 1000;
        assert_eq!(
            timeline_pieces(&[d, e]),
            vec![TimelinePiece::Seg(0), TimelinePiece::Seg(1)]
        );
    }

    #[test]
    fn black_filler_encodes_black_video_with_the_chosen_encoder() {
        let a = black_filler_args(
            Path::new("/tmp/gap.mp4"),
            (1920, 1080),
            30,
            1500,
            "h264_nvenc",
            8000,
        );
        assert!(a.contains(&"color=c=black:s=1920x1080:r=30".to_string()), "{a:?}");
        assert!(a.windows(2).any(|w| w == ["-t", "1.500"]));
        assert!(a.contains(&"-an".to_string()));
        assert!(a.windows(2).any(|w| w == ["-c:v", "h264_nvenc"]));
        assert_eq!(a.last().unwrap(), "/tmp/gap.mp4");
    }

    #[test]
    fn stage_a_force_scale_normalizes_same_size_sources() {
        // A multi-source project forces re-encode + cover scale even when the
        // segment's dims already match the output frame.
        let a = stage_a_args(
            Path::new("/clips/in.mp4"),
            Path::new("/tmp/seg.mp4"),
            &seg(0, 1000),
            (1920, 1080),
            (1920, 1080),
            30,
            FreezePart::Full,
            true,
            "libx264",
            0,
        );
        assert!(!a.windows(2).any(|w| w == ["-c:v", "copy"]));
        let vf = a
            .windows(2)
            .find(|w| w[0] == "-vf")
            .map(|w| w[1].clone())
            .unwrap();
        assert!(vf.contains("scale=1920:1080"), "{vf}");
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

            FreezePart::Full,
            false,
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
    fn stage_a_freeze_parts_split_and_hold_the_frame() {
        let mut s = seg(0, 2000);
        s.freeze_at_ms = 1000;
        s.freeze_ms = 500;
        let before = stage_a_args(
            Path::new("/clips/in.mp4"),
            Path::new("/tmp/before.mp4"),
            &s,
            (640, 360),
            (640, 360),
            30,
            FreezePart::Before {
                at_ms: 1000,
                len_ms: 500,
            },
            false,
            "libx264",
            0,
        );
        assert!(before.windows(2).any(|w| w == ["-ss", "0.000"]));
        // Play 1 s + hold 0.5 s = 1.5 s of output.
        assert!(before.windows(2).any(|w| w == ["-t", "1.500"]), "{before:?}");
        let vf = before
            .windows(2)
            .find(|w| w[0] == "-vf")
            .map(|w| w[1].clone())
            .unwrap();
        assert!(
            vf.contains("tpad=stop_mode=clone:stop_duration=0.500"),
            "{vf}"
        );

        let after = stage_a_args(
            Path::new("/clips/in.mp4"),
            Path::new("/tmp/after.mp4"),
            &s,
            (640, 360),
            (640, 360),
            30,
            FreezePart::After { at_ms: 1000 },
            false,
            "libx264",
            0,
        );
        assert!(after.windows(2).any(|w| w == ["-ss", "1.000"]));
        assert!(after.windows(2).any(|w| w == ["-t", "1.000"]));
        assert!(!after.iter().any(|x| x.contains("tpad")), "{after:?}");
    }

    #[test]
    fn audio_filter_freeze_splits_with_silent_hold() {
        let mut s = seg(0, 2000);
        s.freeze_at_ms = 1000;
        s.freeze_ms = 500;
        let (graph, _) = audio_filter(&[s], &[1], &[3], 2500, 1.0, "mix");
        assert!(
            graph.contains("[1:a:0]atrim=start=0.000:end=1.000"),
            "{graph}"
        );
        // The second half starts 1 s + 500 ms hold later.
        assert!(graph.contains("atrim=start=1.000:end=2.000"), "{graph}");
        assert!(graph.contains("adelay=1500|1500[a0p1_0]"), "{graph}");
    }

    #[test]
    fn audio_filter_mixes_three_independent_channels() {
        let mut s = seg(1000, 2500);
        s.timeline_start_ms = 500;
        s.gain_mix = 1.1;
        s.gain_game = 0.8;
        s.gain_mic = 1.5;
        let (graph, maps) = audio_filter(&[s], &[1], &[3], 3000, 1.0, "mix");
        assert_eq!(maps, vec!["[aout]".to_string()]);
        // Mix, Game and Mic are independent channels with their own gains.
        assert!(graph.contains("volume=1.100,adelay=500|500[a0_0]"), "{graph}");
        assert!(graph.contains("volume=0.800,adelay=500|500[a0_1]"), "{graph}");
        assert!(graph.contains("volume=1.500,adelay=500|500[a0_2]"), "{graph}");
        assert!(graph.contains("amix=inputs=3:duration=longest:normalize=0"), "{graph}");
        assert!(!graph.contains("rubberband"), "{graph}");
    }

    #[test]
    fn audio_filter_uses_each_clips_own_gains_times_master() {
        let mut a = seg(0, 1000);
        a.gain_mix = 1.0;
        a.gain_game = 0.25;
        a.gain_mic = 0.0;
        let mut b = seg(0, 1000);
        b.gain_mix = 1.0;
        b.gain_game = 1.0;
        b.gain_mic = 2.0;
        let (graph, maps) = audio_filter(&[a, b], &[1, 2], &[3, 3], 1000, 0.5, "mix");
        assert_eq!(maps, vec!["[aout]".to_string()]);
        // master x clip gain, per segment and per channel.
        assert!(graph.contains("volume=0.500[a0_0]"), "{graph}");
        assert!(graph.contains("volume=0.125[a0_1]"), "{graph}");
        assert!(graph.contains("volume=0.000[a0_2]"), "{graph}");
        assert!(graph.contains("volume=0.500[a1_0]"), "{graph}");
        assert!(graph.contains("volume=0.500[a1_1]"), "{graph}");
        assert!(graph.contains("volume=1.000[a1_2]"), "{graph}");
        assert!(graph.contains("amix=inputs=6"), "{graph}");
    }

    #[test]
    fn audio_filter_tracks_mode_exports_mix_game_and_mic() {
        let mut s = seg(0, 1000);
        s.gain_mix = 0.9;
        s.gain_game = 0.4;
        s.gain_mic = 1.6;
        let (graph, maps) = audio_filter(&[s], &[1], &[3], 1000, 1.0, "tracks");
        assert_eq!(
            maps,
            vec![
                "[amix]".to_string(),
                "[agame]".to_string(),
                "[amic]".to_string()
            ]
        );
        // Mix stem (stream 0) as its own track, plus Game and Mic.
        assert!(graph.contains("volume=0.900[a0_0]"), "{graph}");
        assert!(graph.contains("volume=0.400[a0_1]"), "{graph}");
        assert!(graph.contains("volume=1.600[a0_2]"), "{graph}");
        assert!(graph.contains("[a0_0]amix=inputs=1"), "{graph}");
        assert!(graph.contains("[a0_1]amix=inputs=1"), "{graph}");
        assert!(graph.contains("[a0_2]amix=inputs=1"), "{graph}");
        assert!(!graph.contains("[aout]"), "{graph}");
    }

    #[test]
    fn audio_filter_tracks_mode_single_track_lands_on_mix_bus() {
        let (graph, maps) = audio_filter(&[seg(0, 1000)], &[1], &[1], 1000, 1.0, "tracks");
        assert_eq!(maps, vec!["[amix]".to_string()]);
        assert!(graph.contains("[amix]"), "{graph}");
        assert!(!graph.contains("[agame]"), "{graph}");
        assert!(!graph.contains("[amic]"), "{graph}");
    }

    #[test]
    fn stage_a_applies_crop_zoom_pan_rotation_opacity_and_filters() {
        let mut s = seg(0, 2000);
        s.crop_left = 0.1;
        s.crop_right = 0.2;
        s.zoom = 2.0;
        s.offset_x = 0.5;
        s.offset_y = -1.0;
        s.rotation = 90.0;
        s.opacity = 0.5;
        s.brightness = 0.1;
        s.contrast = 1.2;
        s.saturation = 1.3;
        s.gamma = 1.1;
        s.temperature = 0.5;
        s.vignette = 0.4;
        let a = stage_a_args(
            Path::new("/clips/in.mp4"),
            Path::new("/tmp/seg.mp4"),
            &s,
            (1920, 1080),
            (1920, 1080),

            0,

            FreezePart::Full,
            false,
            "libx264",
            0,
        );
        let vf = a
            .windows(2)
            .find(|w| w[0] == "-vf")
            .map(|w| w[1].clone())
            .expect("vf");
        assert!(vf.contains("crop=iw*0.7000:ih*1.0000:iw*0.1000:ih*0.0000"), "{vf}");
        assert!(vf.contains("scale=iw*2.0000:ih*2.0000"), "{vf}");
        assert!(vf.contains("rotate=1.570796"), "{vf}");
        assert!(vf.contains("colorchannelmixer=aa=0.5000"), "{vf}");
        assert!(vf.contains("eq=brightness=0.1000:contrast=1.2000"), "{vf}");
        assert!(vf.contains("colorbalance=rs=0.1500"), "{vf}");
        assert!(vf.contains("vignette=angle=0.5655"), "{vf}");
        assert!(a.windows(2).any(|w| w == ["-c:v", "libx264"]));
    }

    #[test]
    fn stage_a_still_copies_when_nothing_is_set() {
        let a = stage_a_args(
            Path::new("/clips/in.mp4"),
            Path::new("/tmp/seg.mp4"),
            &seg(0, 1000),
            (1920, 1080),
            (1920, 1080),

            0,

            FreezePart::Full,
            false,
            "libx264",
            0,
        );
        assert!(a.windows(2).any(|w| w == ["-c:v", "copy"]));
        assert!(!a.iter().any(|x| x == "-vf"));
    }

    #[test]
    fn audio_filter_stretches_with_rubberband_for_speed() {
        let mut s = seg(0, 2000);
        s.speed = 0.5;
        let (graph, maps) = audio_filter(&[s], &[1], &[3], 4000, 1.0, "mix");
        assert!(!maps.is_empty());
        assert!(graph.contains("rubberband=tempo=0.5000"), "{graph}");
        let mut s = seg(0, 2000);
        s.speed = 3.0;
        let (graph, _) = audio_filter(&[s], &[1], &[3], 700, 1.0, "mix");
        assert!(graph.contains("rubberband=tempo=3.0000"), "{graph}");
    }

    #[test]
    fn audio_filter_single_track_uses_clip_mix_gain() {
        let mut s = seg(0, 1000);
        s.gain_mix = 0.5;
        let (graph, maps) = audio_filter(&[s], &[1], &[1], 1000, 1.0, "mix");
        assert_eq!(maps, vec!["[aout]".to_string()]);
        assert!(graph.contains("[1:a:0]"), "{graph}");
        assert!(graph.contains("volume=0.500"), "{graph}");
    }

    #[test]
    fn audio_filter_two_tracks_use_mix_and_game() {
        let mut s = seg(0, 1000);
        s.gain_mix = 0.3;
        s.gain_game = 0.5;
        let (graph, maps) = audio_filter(&[s], &[1], &[2], 1000, 1.0, "mix");
        assert_eq!(maps, vec!["[aout]".to_string()]);
        assert!(graph.contains("volume=0.300[a0_0]"), "{graph}");
        assert!(graph.contains("volume=0.500[a0_1]"), "{graph}");
        assert!(!graph.contains("[1:a:2]"), "{graph}");
    }

    #[test]
    fn audio_filter_without_tracks_reports_no_audio() {
        let (graph, maps) = audio_filter(&[seg(0, 1000)], &[1], &[0], 1000, 1.0, "mix");
        assert!(maps.is_empty());
        assert!(graph.is_empty());
    }

    #[test]
    fn ass_subtitles_include_position_size_rotation_and_opacity() {
        let mut o = crate::editor::project::default_text_overlay("Hola\nmundo", 1000, 3000);
        o.x = 0.25;
        o.y = 0.8;
        o.font_size = 60;
        o.stroke_width = 4.0;
        o.scale = 2.0;
        o.rotation = 45.0;
        o.opacity = 0.5;
        o.shadow = true;
        let s = ass_subtitles(&[&o], 1920, 1080);
        assert!(s.contains("PlayResX: 1920"), "{s}");
        assert!(s.contains("PlayResY: 1080"), "{s}");
        assert!(s.contains("\\pos(480.0,864.0)"), "{s}");
        assert!(s.contains("\\frz(45.0)"), "{s}");
        // 60px at 1080 reference * scale 2 = 120.
        assert!(s.contains("\\fs120"), "{s}");
        assert!(s.contains("\\bord8"), "{s}");
        assert!(s.contains("\\shad2"), "{s}");
        assert!(s.contains("\\alpha&H80&"), "{s}");
        assert!(s.contains("Hola\\Nmundo"), "{s}");
        assert!(s.contains("Dialogue: 0,0:00:01.00,0:00:04.00,Clip,,0,0,0,,"), "{s}");
    }

    #[test]
    fn ass_color_is_bgr_and_braces_are_escaped() {
        assert_eq!(ass_color("#22d3ee", 1.0), "&H00EED322");
        assert_eq!(ass_color("red", 1.0), "&H00FFFFFF");
        let mut o = crate::editor::project::default_text_overlay("{hola}", 0, 1000);
        o.rotation = -30.0;
        o.opacity = 1.0;
        let s = ass_subtitles(&[&o], 640, 360);
        assert!(s.contains("\\{hola\\}"), "{s}");
        assert!(s.contains("\\frz(-30.0)"), "{s}");
    }

    #[test]
    fn filter_value_escaping_is_safe() {
        assert_eq!(escape_filter_value("/tmp/it's/a.txt"), "/tmp/it\\'s/a.txt");
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

            FreezePart::Full,
            false,
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
        let (filter, maps) = audio_filter(&[seg(1000, 2500)], &[1], &[3], 1500, 1.0, "mix");
        assert_eq!(maps, vec!["[aout]".to_string()]);
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

        // Tracks mode: Mix, Game and Mic as three separate AAC tracks.
        let tracks_out = dir.join("tracks.mp4");
        let (tracks_filter, tracks_maps) =
            audio_filter(&[seg(1000, 2500)], &[1], &[3], 1500, 1.0, "tracks");
        assert_eq!(
            tracks_maps,
            vec![
                "[amix]".to_string(),
                "[agame]".to_string(),
                "[amic]".to_string()
            ]
        );
        let ok = tokio::process::Command::new(&ff)
            .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(&seg_file)
            .args(["-i"])
            .arg(&src)
            .args(["-filter_complex", &tracks_filter])
            .args(["-map", "0:v:0"])
            .args(["-map", "[amix]", "-map", "[agame]", "-map", "[amic]"])
            .args(["-c:v", "copy", "-c:a", "aac", "-b:a", "192k"])
            .arg(&tracks_out)
            .status()
            .await
            .unwrap();
        assert!(ok.success(), "tracks stage B failed");
        let stderr = tokio::process::Command::new(&ff)
            .args(["-hide_banner", "-i"])
            .arg(&tracks_out)
            .output()
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&stderr.stderr);
        assert_eq!(session::parse_audio_track_count(&text), 3, "{text}");

        // Speed 2x: video setpts + audio rubberband must produce a clip half
        // as long, still with one audio track.
        let mut fast = seg(1000, 2500);
        fast.speed = 2.0;
        let fast_a = dir.join("fast_a.mp4");
        let ok = tokio::process::Command::new(&ff)
            .args(stage_a_args(
                &src,
                &fast_a,
                &fast,
                (640, 360),
                (640, 360),

                0,

                FreezePart::Full,
                false,
                "libx264",
                0,
            ))
            .status()
            .await
            .unwrap();
        assert!(ok.success(), "speed stage A failed");
        let (fast_filter, _) = audio_filter(&[fast.clone()], &[1], &[3], 750, 1.0, "mix");
        let fast_out = dir.join("fast.mp4");
        let ok = tokio::process::Command::new(&ff)
            .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(&fast_a)
            .args(["-i"])
            .arg(&src)
            .args(["-filter_complex", &fast_filter, "-map", "0:v:0", "-map", "[aout]"])
            .args(["-c:v", "copy", "-c:a", "aac", "-b:a", "192k"])
            .arg(&fast_out)
            .status()
            .await
            .unwrap();
        assert!(ok.success(), "speed stage B failed");
        let ms = crate::editor::ffmpeg::probe_duration_ms(&ff, &fast_out)
            .await
            .expect("probe speed export");
        assert!((500..=1100).contains(&ms), "speed export duration {ms} ms");

        // Text overlay: the generated ASS subtitles must render with libass
        // and the bundled font (rotation included).
        let font = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fonts")
            .join("DejaVuSans.ttf");
        assert!(font.is_file(), "vendored font missing");
        let fonts_dir = font.parent().unwrap();
        let mut overlay =
            crate::editor::project::default_text_overlay("Hola MoonClip", 0, 1500);
        overlay.stroke_width = 4.0;
        overlay.rotation = 30.0;
        let ass_path = dir.join("overlays.ass");
        std::fs::write(&ass_path, ass_subtitles(&[&overlay], 640, 360)).unwrap();
        let filter = format!(
            "ass=filename='{}':fontsdir='{}'",
            escape_filter_value(&ass_path.to_string_lossy()),
            escape_filter_value(&fonts_dir.to_string_lossy()),
        );
        let with_text = dir.join("with_text.mp4");
        let ok = tokio::process::Command::new(&ff)
            .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(&out)
            .args(["-vf", &filter, "-c:v", "libx264", "-preset", "ultrafast", "-an"])
            .arg(&with_text)
            .status()
            .await
            .unwrap();
        assert!(ok.success(), "ass export failed");
        assert!(with_text.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Acceptance (export everything): two sources with DIFFERENT resolutions
    /// and a gap between them. Each segment must be normalized to the output
    /// frame, the gap filled with black, and the result must be one video +
    /// one mixed audio track with the exact project duration.
    #[tokio::test]
    async fn live_multisource_gap_export() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let ff = manifest
            .join("binaries")
            .join(crate::sidecar::host_triple())
            .join(crate::editor::ffmpeg::sidecar_name());
        if !ff.exists() {
            eprintln!("skip: bundled ffmpeg not staged");
            return;
        }
        let dir =
            std::env::temp_dir().join(format!("moonclip-multi-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        // Two fixtures: 640x360@30 and 480x270@30, 3 audio tracks each.
        let mut fixtures = Vec::new();
        for (w, h, name) in [(640u32, 360u32, "a.mp4"), (480, 270, "b.mp4")] {
            let path = dir.join(name);
            let status = tokio::process::Command::new(&ff)
                .args([
                    "-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                ])
                .arg(format!("testsrc2=size={w}x{h}:rate=30"))
                .args([
                    "-f", "lavfi", "-i", "sine=frequency=440",
                    "-f", "lavfi", "-i", "sine=frequency=880",
                    "-f", "lavfi", "-i", "sine=frequency=1320",
                    "-map", "0:v", "-map", "1:a", "-map", "2:a", "-map", "3:a",
                    "-t", "2", "-c:v", "libx264", "-preset", "ultrafast", "-g", "30",
                    "-c:a", "aac",
                ])
                .arg(&path)
                .status()
                .await
                .unwrap();
            assert!(status.success(), "fixture {name} failed");
            fixtures.push(path);
        }

        // Project: A [0..2000] at 0, B [0..2000] at 2500 -> 500 ms gap.
        let mut a_seg = seg(0, 2000);
        a_seg.timeline_start_ms = 0;
        let mut b_seg = seg(0, 2000);
        b_seg.timeline_start_ms = 2500;
        b_seg.zoom = 1.25;
        let project = vec![a_seg, b_seg];
        let pieces = timeline_pieces(&project);
        assert_eq!(pieces.len(), 3, "two clips + one black filler");

        // Stage A: pieces normalized to 640x360@30 (base frame).
        let mut piece_files: Vec<std::path::PathBuf> = Vec::new();
        for (i, piece) in pieces.iter().enumerate() {
            let out = dir.join(format!("p{i}.mp4"));
            let args = match piece {
                TimelinePiece::Black { duration_ms } => {
                    black_filler_args(&out, (640, 360), 30, *duration_ms, "libx264", 0)
                }
                TimelinePiece::Seg(idx) => stage_a_args(
                    &fixtures[*idx],
                    &out,
                    &project[*idx],
                    (640, 360),
                    if *idx == 0 { (640, 360) } else { (480, 270) },
                    30,
                    FreezePart::Full,
                    true,
                    "libx264",
                    0,
                ),
            };
            let ok = tokio::process::Command::new(&ff)
                .args(&args)
                .status()
                .await
                .unwrap();
            assert!(ok.success(), "piece {i} failed");
            piece_files.push(out);
        }

        // Stage B: concat + audio (the gap is silent via adelay).
        let list = dir.join("concat.txt");
        std::fs::write(&list, concat_file(&piece_files)).unwrap();
        let joined = dir.join("joined.mp4");
        let ok = tokio::process::Command::new(&ff)
            .args([
                "-y", "-hide_banner", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i",
            ])
            .arg(&list)
            .args(["-c", "copy"])
            .arg(&joined)
            .status()
            .await
            .unwrap();
        assert!(ok.success(), "concat failed");

        let (filter, maps) = audio_filter(&project, &[1, 2], &[3, 3], 4500, 1.0, "mix");
        assert_eq!(maps, vec!["[aout]".to_string()]);
        let out = dir.join("out.mp4");
        let ok = tokio::process::Command::new(&ff)
            .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(&joined)
            .args(["-i"])
            .arg(&fixtures[0])
            .args(["-i"])
            .arg(&fixtures[1])
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
        let ms = crate::editor::ffmpeg::probe_duration_ms(&ff, &out)
            .await
            .expect("probe duration");
        assert!((4300..=4700).contains(&ms), "duration {ms} ms (expected ~4500)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Acceptance (freeze): a frozen segment exports as "before + hold" and
    /// "after", concatenated, with the exact expected duration.
    #[tokio::test]
    async fn live_freeze_export_keeps_the_hold() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let ff = manifest
            .join("binaries")
            .join(crate::sidecar::host_triple())
            .join(crate::editor::ffmpeg::sidecar_name());
        if !ff.exists() {
            eprintln!("skip: bundled ffmpeg not staged");
            return;
        }
        let dir =
            std::env::temp_dir().join(format!("moonclip-freeze-{}", uuid::Uuid::new_v4().simple()));
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

        let mut s = seg(0, 2000);
        s.freeze_at_ms = 1000;
        s.freeze_ms = 500;
        let mut files = Vec::new();
        for (part, seconds) in [
            (
                FreezePart::Before {
                    at_ms: 1000,
                    len_ms: 500,
                },
                1.5,
            ),
            (FreezePart::After { at_ms: 1000 }, 1.0),
        ] {
            let out = dir.join(format!("part{}.mp4", files.len()));
            let args = stage_a_args(
                &src,
                &out,
                &s,
                (640, 360),
                (640, 360),
                30,
                part,
                true,
                "libx264",
                0,
            );
            let ok = tokio::process::Command::new(&ff)
                .args(&args)
                .status()
                .await
                .unwrap();
            assert!(ok.success(), "freeze {part:?} failed");
            let ms = crate::editor::ffmpeg::probe_duration_ms(&ff, &out)
                .await
                .expect("probe piece");
            assert!(
                (ms as f64 - seconds * 1000.0).abs() < 500.0,
                "piece {part:?} duration {ms} ms"
            );
            files.push(out);
        }
        let list = dir.join("concat.txt");
        std::fs::write(&list, concat_file(&files)).unwrap();
        let joined = dir.join("joined.mp4");
        let ok = tokio::process::Command::new(&ff)
            .args([
                "-y", "-hide_banner", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i",
            ])
            .arg(&list)
            .args(["-c", "copy"])
            .arg(&joined)
            .status()
            .await
            .unwrap();
        assert!(ok.success(), "freeze concat failed");
        let ms = crate::editor::ffmpeg::probe_duration_ms(&ff, &joined)
            .await
            .expect("probe joined");
        assert!((2300..=2800).contains(&ms), "freeze export duration {ms} ms");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unique_edit_name_avoids_collisions() {
        let taken: std::collections::HashSet<String> = ["clip_edit.mp4".to_string()].into();
        let name = unique_edit_name(Path::new("/tmp"), "clip", &taken);
        assert_eq!(name, "clip_edit_2.mp4");
    }
}
