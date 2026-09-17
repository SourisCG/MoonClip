//! Live per-track capture gain (Linux/PipeWire).
//! Mechanism: per-stream (source-output) volume via pactl. This changes ONLY
//! what GSR records — never what the user hears (device volumes untouched).
//!
//! Stream identity (proven in GSR source, main.cpp: description = "gsr-" + -a arg):
//! a stream belongs to us iff application.name == "gsr-<our -a argument>".
//! Heuristic fallback (monitor/output/input words) covers older/other builds.

use std::collections::HashMap;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::sleep;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Track {
    /// Merged game+mic (track 1). Always full-fidelity safety copy.
    Mix,
    Game,
    Mic,
}

#[derive(Debug, serde::Deserialize)]
struct Output {
    index: u32,
    properties: HashMap<String, String>,
}

fn prop(o: &Output, key: &str) -> String {
    o.properties.get(key).cloned().unwrap_or_default().to_lowercase()
}

async fn all_outputs() -> Result<Vec<Output>, String> {
    let out = Command::new("pactl")
        .args(["-f", "json", "list", "source-outputs"])
        .output()
        .await
        .map_err(|e| format!("pactl failed: {e}"))?;
    if !out.status.success() {
        return Err("pactl list failed".into());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("pactl parse: {e}"))
}

fn looks_like_ours(app_name: &str) -> bool {
    app_name.starts_with("gsr-") || app_name.contains("gpu-screen-recorder")
}

fn classify(name: &str) -> Option<Track> {
    if name.contains("monitor") || name.contains("output") || name.contains("sink") {
        // "default_input" contains none of the mic words; check mic first below.
        if name.contains("input") || name.contains("source") || name.contains("mic") {
            // Ambiguous names (contain both families): prefer mic only when no
            // game word other than a bare "output" device suffix is present.
            if !name.contains("monitor") && !name.contains("sink") {
                return Some(Track::Mic);
            }
        }
        return Some(Track::Game);
    }
    if name.contains("input") || name.contains("source") || name.contains("mic") {
        return Some(Track::Mic);
    }
    None
}

/// GSR recording streams tagged with their track.
/// `known_args`: the exact `-a` values our engine spawned with (e.g.
/// ["default_output", "default_input", "device:x"]). Exact match wins.
pub async fn gsr_streams(known_args: &[String]) -> Result<Vec<(u32, Track)>, String> {
    let outputs = all_outputs().await?;
    let ours: Vec<&Output> = outputs
        .iter()
        .filter(|o| looks_like_ours(&prop(o, "application.name")))
        .collect();
    if ours.is_empty() {
        return Ok(vec![]);
    }
    let mut tagged: Vec<(u32, Track)> = Vec::new();
    let mut untagged: Vec<u32> = Vec::new();
    for o in &ours {
        let app = prop(o, "application.name");
        // Merged track: GSR names it gsr-combined-<random> (proven in source).
        if app.starts_with("gsr-combined") {
            tagged.push((o.index, Track::Mix));
            continue;
        }
        let suffix = app.strip_prefix("gsr-").unwrap_or(&app);
        // 1. Exact match against our -a args (mix = first arg, game = second, mic = third).
        if let Some(pos) = known_args.iter().position(|a| {
            let a = a.to_lowercase();
            suffix == a || suffix == format!("device:{a}") || suffix == format!("app:{a}")
        }) {
            let track = match pos {
                0 => Track::Mix,
                1 => Track::Game,
                _ => Track::Mic,
            };
            tagged.push((o.index, track));
            continue;
        }
        // 2. Heuristic fallback on media/node names.
        let hay = format!("{} {}", prop(o, "media.name"), prop(o, "node.name"));
        match classify(&hay) {
            Some(t) => tagged.push((o.index, t)),
            None => untagged.push(o.index),
        }
    }
    // 3. Order fallback: GSR spawns -a in order (game first).
    untagged.sort_unstable();
    for idx in untagged {
        let has_game = tagged.iter().any(|(_, t)| *t == Track::Game);
        tagged.push((idx, if has_game { Track::Mic } else { Track::Game }));
    }
    Ok(tagged)
}

async fn set_volume(index: u32, percent: u32) -> Result<(), String> {
    let pct = percent.clamp(0, 200);
    let status = Command::new("pactl")
        .args(["set-source-output-volume", &index.to_string(), &format!("{pct}%")])
        .status()
        .await
        .map_err(|e| format!("pactl volume failed: {e}"))?;
    if !status.success() {
        return Err(format!("pactl volume failed for stream {index}"));
    }
    Ok(())
}

async fn set_mute(index: u32, muted: bool) -> Result<(), String> {
    let status = Command::new("pactl")
        .args([
            "set-source-output-mute",
            &index.to_string(),
            if muted { "1" } else { "0" },
        ])
        .status()
        .await
        .map_err(|e| format!("pactl mute failed: {e}"))?;
    if !status.success() {
        return Err(format!("pactl mute failed for stream {index}"));
    }
    Ok(())
}

/// Wait (streams appear async after spawn) then apply gains to each track.
/// Returns how many tracks were linked. Errors name the failing step.
pub async fn apply_gains(
    known_args: &[String],
    game_pct: u32,
    mic_pct: u32,
    mute_game: bool,
    mute_mic: bool,
) -> Result<usize, String> {
    // Streams can take a while to appear after spawn (device negotiation),
    // so retry in background instead of failing fast.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let streams = gsr_streams(known_args).await?;
        if !streams.is_empty() {
            for (idx, track) in &streams {
                match track {
                    // Mix tap stays a full-fidelity safety copy: gains and
                    // mutes shape the solo tracks only (a muted tap would
                    // kill both sources in the mix, which is never wanted).
                    Track::Mix => {
                        set_volume(*idx, 100).await?;
                        set_mute(*idx, false).await?;
                    }
                    Track::Game => {
                        set_volume(*idx, game_pct).await?;
                        set_mute(*idx, mute_game).await?;
                    }
                    Track::Mic => {
                        set_volume(*idx, mic_pct).await?;
                        set_mute(*idx, mute_mic).await?;
                    }
                }
            }
            return Ok(streams.len());
        }
        if std::time::Instant::now() >= deadline {
            return Err("no GSR audio streams appeared within 30s".into());
        }
        sleep(Duration::from_millis(250)).await;
    }
}

/// Single-shot count for visible UI status (no waiting, errors count as zero).
pub async fn linked_count(known_args: &[String]) -> usize {
    gsr_streams(known_args).await.map(|s| s.len()).unwrap_or(0)
}

/// Peak levels of the last refresh window, for the UI signal meters. Linux
/// captures through GSR (its own process), so per-track peaks are not
/// available here — Windows implements this from its cpal rings. `None`
/// hides the meters.
pub fn recent_peaks() -> Option<(f32, f32)> {
    None
}
