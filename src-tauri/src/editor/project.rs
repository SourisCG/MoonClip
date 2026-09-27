//! Edit project model (JSON over IPC, camelCase keys per project convention).
//!
//! E2 supports a single video segment per clip plus the three capture audio
//! tracks; the schema already carries lists so E5 (multi-clip, transitions,
//! music) and E3/E4 (overlays, effects) extend it without migrations.

use serde::{Deserialize, Serialize};

fn default_speed() -> f64 {
    1.0
}
fn default_gain() -> f64 {
    1.0
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OutputSettings {
    /// Target height (0 = source height).
    pub height: u32,
    /// "source" | "16:9" | "9:16" | "1:1"
    pub aspect: String,
    /// Target fps (0 = source fps).
    pub fps: u32,
    /// Target bitrate in kbps (0 = encoder default/ladder).
    pub bitrate_kbps: u32,
    /// "auto" | "cpu" | explicit ffmpeg encoder id.
    pub encoder: String,
    pub container: String,
}

impl Default for OutputSettings {
    fn default() -> Self {
        Self {
            height: 0,
            aspect: "source".into(),
            fps: 0,
            bitrate_kbps: 0,
            encoder: "auto".into(),
            container: "mp4".into(),
        }
    }
}

/// One piece of a source clip on the video track.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub id: String,
    pub source_clip_id: String,
    /// Trim inside the source (ms).
    pub in_ms: i64,
    pub out_ms: i64,
    /// Position on the timeline (ms).
    pub timeline_start_ms: i64,
    #[serde(default = "default_speed")]
    pub speed: f64,
    /// Freeze frame at this source time for `freeze_ms` (E4, 0 = off).
    #[serde(default)]
    pub freeze_at_ms: i64,
    #[serde(default)]
    pub freeze_ms: i64,
    #[serde(default = "default_gain")]
    pub gain_mix: f64,
    #[serde(default = "default_gain")]
    pub gain_game: f64,
    #[serde(default = "default_gain")]
    pub gain_mic: f64,
}

impl Segment {
    /// Timeline length after speed/freeze.
    pub fn timeline_duration_ms(&self) -> i64 {
        let base = ((self.out_ms - self.in_ms).max(0) as f64 / self.speed.max(0.05)) as i64;
        base + self.freeze_ms.max(0)
    }
}

/// Extra audio (music/uploads) placed on the timeline (E5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrack {
    pub id: String,
    /// File name inside the session dir or clips dir.
    pub file_name: String,
    pub start_ms: i64,
    pub duration_ms: i64,
    #[serde(default = "default_gain")]
    pub gain: f64,
    #[serde(default = "default_true")]
    pub loop_playback: bool,
}

fn default_opacity() -> f64 {
    1.0
}
fn default_font_size() -> u32 {
    64
}
fn default_color() -> String {
    "#ffffff".into()
}
fn default_stroke_color() -> String {
    "#000000".into()
}
fn default_text_align() -> String {
    "center".into()
}

/// Visual overlay (text/sticker/gif/image). E3a ships text; stickers/GIFs
/// reuse the same schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Overlay {
    pub id: String,
    /// "text" | "sticker" | "gif" | "image"
    pub kind: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub asset: Option<String>,
    pub start_ms: i64,
    pub duration_ms: i64,
    /// Normalized center position (0..1 of the output frame).
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default = "default_gain")]
    pub scale: f64,
    #[serde(default)]
    pub rotation: f64,
    #[serde(default = "default_opacity")]
    pub opacity: f64,
    /// Text style (font size at a 1080p reference height).
    #[serde(default = "default_font_size")]
    pub font_size: u32,
    #[serde(default = "default_color")]
    pub color: String,
    #[serde(default = "default_stroke_color")]
    pub stroke_color: String,
    #[serde(default)]
    pub stroke_width: f64,
    #[serde(default)]
    pub shadow: bool,
    #[serde(default = "default_text_align")]
    pub align: String,
}

/// New text overlay centered on screen for `duration_ms` at `start_ms`.
/// The frontend creates overlays; this helper documents the defaults and is
/// the fixture used by export tests.
#[allow(dead_code)]
pub fn default_text_overlay(text: &str, start_ms: i64, duration_ms: i64) -> Overlay {
    Overlay {
        id: uuid::Uuid::new_v4().to_string(),
        kind: "text".into(),
        text: Some(text.into()),
        asset: None,
        start_ms: start_ms.max(0),
        duration_ms: duration_ms.max(200),
        x: 0.5,
        y: 0.78,
        scale: 1.0,
        rotation: 0.0,
        opacity: 1.0,
        font_size: 64,
        color: default_color(),
        stroke_color: default_stroke_color(),
        stroke_width: 3.0,
        shadow: true,
        align: "center".into(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EditProject {
    pub version: u32,
    pub id: String,
    pub name: String,
    pub source_clip_id: String,
    /// Track mix: `gain_master` multiplies the stem mix, and Game/Mic are the
    /// individual stems. Track 1 of the recording (Mix) is the SUM of both, so
    /// it is never played alongside them (that caused doubled audio).
    #[serde(default = "default_gain")]
    pub gain_master: f64,
    #[serde(default = "default_gain")]
    pub gain_game: f64,
    #[serde(default = "default_gain")]
    pub gain_mic: f64,
    #[serde(default)]
    pub output: OutputSettings,
    pub segments: Vec<Segment>,
    #[serde(default)]
    pub audio_tracks: Vec<AudioTrack>,
    #[serde(default)]
    pub overlays: Vec<Overlay>,
    #[serde(default)]
    pub updated_at: String,
}

impl EditProject {
    /// Total timeline length in ms.
    pub fn duration_ms(&self) -> i64 {
        self.segments
            .iter()
            .map(|s| s.timeline_start_ms + s.timeline_duration_ms())
            .max()
            .unwrap_or(0)
    }
}

/// Fresh single-segment project for a clip.
pub fn default_project(clip_id: &str, name: &str, duration_ms: i64) -> EditProject {
    EditProject {
        version: 2,
        id: uuid::Uuid::new_v4().to_string(),
        name: name.to_string(),
        source_clip_id: clip_id.to_string(),
        gain_master: 1.0,
        gain_game: 1.0,
        gain_mic: 1.0,
        output: OutputSettings::default(),
        segments: vec![Segment {
            id: uuid::Uuid::new_v4().to_string(),
            source_clip_id: clip_id.to_string(),
            in_ms: 0,
            out_ms: duration_ms.max(100),
            timeline_start_ms: 0,
            speed: 1.0,
            freeze_at_ms: 0,
            freeze_ms: 0,
            // Track 1 (Mix) already contains Game+Mic: start with the mix only
            // so the preview never plays the same audio twice. Users raise the
            // stems and lower Mix to remix.
            gain_mix: 1.0,
            gain_game: 0.0,
            gain_mic: 0.0,
        }],
        audio_tracks: Vec::new(),
        overlays: Vec::new(),
        updated_at: String::new(),
    }
}

/// Projects saved before the track gains moved to the project level carry
/// them per segment (and the old default was "mix only", which in the new
/// model would silence the stems since the Mix track is no longer played).
/// Adopt the first segment's values, then make sure a mix-only project ends
/// up as Game+Mic so it still sounds like before. Returns true when changed.
pub fn migrate_gains(project: &mut EditProject) -> bool {
    let mut changed = false;
    let mut master = project.gain_master;
    let mut game = project.gain_game;
    let mut mic = project.gain_mic;
    // v1 projects carried the gains per segment; v2 moved them to the project.
    if project.version < 2 {
        if let Some(seg) = project.segments.first() {
            let legacy = (seg.gain_mix, seg.gain_game, seg.gain_mic);
            if (legacy.0 - 1.0).abs() > f64::EPSILON
                || (legacy.1 - 1.0).abs() > f64::EPSILON
                || (legacy.2 - 1.0).abs() > f64::EPSILON
            {
                master = legacy.0;
                game = legacy.1;
                mic = legacy.2;
            }
        }
        project.version = 2;
        changed = true;
    }
    // Old "mix only" default (mix=1, stems 0): the same audio is now
    // Game+Mic, so enable the stems.
    if master > 0.0 && game <= 0.0 && mic <= 0.0 {
        game = 1.0;
        mic = 1.0;
        changed = true;
    }
    project.gain_master = master.clamp(0.0, 4.0);
    project.gain_game = game.clamp(0.0, 4.0);
    project.gain_mic = mic.clamp(0.0, 4.0);
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_roundtrip_is_camel_case() {
        let p = default_project("clip-1", "Test", 2500);
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"sourceClipId\""));
        assert!(json.contains("\"timelineStartMs\""));
        assert!(!json.contains("\"source_clip_id\""));
        let back: EditProject = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn duration_accounts_for_speed_and_freeze() {
        let mut p = default_project("c", "n", 1000);
        p.segments[0].speed = 2.0;
        assert_eq!(p.duration_ms(), 500);
        p.segments[0].freeze_at_ms = 400;
        p.segments[0].freeze_ms = 300;
        assert_eq!(p.duration_ms(), 800);
        // A gap (segment moved right) extends the timeline too.
        p.segments[0].timeline_start_ms = 1000;
        assert_eq!(p.duration_ms(), 1800);
    }

    #[test]
    fn migration_adopts_segment_gains_and_enables_stems_for_mix_only() {
        let mut p = default_project("c", "n", 1000);
        p.version = 1;
        // Old default: mix only (track 1 = game+mic baked).
        p.segments[0].gain_mix = 1.0;
        p.segments[0].gain_game = 0.0;
        p.segments[0].gain_mic = 0.0;
        assert!(migrate_gains(&mut p));
        assert_eq!(p.gain_master, 1.0);
        assert_eq!(p.gain_game, 1.0);
        assert_eq!(p.gain_mic, 1.0);
        assert!(!migrate_gains(&mut p));
        // A deliberate stem remix survives (adopted as-is).
        p.gain_game = 0.3;
        p.gain_mic = 1.4;
        assert!(!migrate_gains(&mut p));
        assert_eq!(p.gain_game, 0.3);
        assert_eq!(p.gain_mic, 1.4);
    }

    #[test]
    fn migration_adopts_legacy_segment_values_once() {
        let mut p = default_project("c", "n", 1000);
        p.version = 1;
        p.gain_master = 1.0;
        p.gain_game = 1.0;
        p.gain_mic = 1.0;
        p.segments[0].gain_mix = 0.5;
        p.segments[0].gain_game = 1.2;
        p.segments[0].gain_mic = 0.0;
        assert!(migrate_gains(&mut p));
        assert_eq!(p.gain_master, 0.5);
        assert_eq!(p.gain_game, 1.2);
        assert_eq!(p.gain_mic, 0.0);
    }

    #[test]
    fn default_project_uses_the_stems_with_full_gains() {
        let p = default_project("c", "n", 1000);
        assert_eq!(p.gain_master, 1.0);
        assert_eq!(p.gain_game, 1.0);
        assert_eq!(p.gain_mic, 1.0);
    }

    #[test]
    fn old_json_without_optional_fields_still_loads() {
        let json = r#"{
            "version": 1, "id": "x", "name": "n", "sourceClipId": "c",
            "segments": [{"id":"s","sourceClipId":"c","inMs":0,"outMs":500,"timelineStartMs":0}]
        }"#;
        let p: EditProject = serde_json::from_str(json).unwrap();
        assert_eq!(p.output, OutputSettings::default());
        assert_eq!(p.segments[0].speed, 1.0);
        assert!(p.audio_tracks.is_empty());
    }
}
