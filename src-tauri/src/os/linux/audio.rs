//! PulseAudio/PipeWire helpers for the editor's playback stream.
//!
//! KDE/WirePlumber remembers per-application mute state (`module-stream-restore`):
//! once the editor stream is muted from the system mixer, every new
//! `application.name = "moonclip"` output stream starts muted. The editor
//! clears that for its OWN stream only (never other apps, never the capture
//! engine, whose PulSE client is `MoonClip` as a record stream).

use std::process::Command;

/// Are any of the editor's streams still muted? (verification pass)
pub fn editor_streams_muted(pactl_list: &str) -> usize {
    let mut count = 0;
    let mut is_ours = false;
    let mut muted = false;
    for line in pactl_list.lines() {
        if line.trim().starts_with("Sink Input #") {
            if is_ours && muted {
                count += 1;
            }
            is_ours = false;
            muted = false;
        } else if line.contains("application.name = \"moonclip\"") {
            is_ours = true;
        } else if line.contains("Mute: yes") {
            // Mute can be printed before or after application.name.
            muted = true;
        }
    }
    if is_ours && muted {
        count += 1;
    }
    count
}

/// Sink-input ids whose `application.name` is exactly `moonclip`.
pub fn editor_stream_ids(pactl_list: &str) -> Vec<u32> {
    let mut ids = Vec::new();
    let mut current: Option<u32> = None;
    let mut is_ours = false;
    for line in pactl_list.lines() {
        if let Some(rest) = line.trim().strip_prefix("Sink Input #") {
            if let (Some(id), true) = (current, is_ours) {
                ids.push(id);
            }
            current = rest.split_whitespace().next().and_then(|s| s.parse().ok());
            is_ours = false;
        } else if line.contains("application.name = \"moonclip\"") {
            is_ours = true;
        }
    }
    if let (Some(id), true) = (current, is_ours) {
        ids.push(id);
    }
    ids
}

/// Unmute + raise the editor's output streams. Returns how many were fixed.
pub fn unmute_editor_streams() -> Result<usize, String> {
    let out = Command::new("pactl")
        .args(["list", "sink-inputs"])
        .output()
        .map_err(|e| format!("pactl not available: {e}"))?;
    if !out.status.success() {
        return Err("pactl list sink-inputs failed".into());
    }
    let ids = editor_stream_ids(&String::from_utf8_lossy(&out.stdout));
    let mut fixed = 0;
    for id in &ids {
        let id = id.to_string();
        let muted = Command::new("pactl")
            .args(["set-sink-input-mute", &id, "0"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let volume = Command::new("pactl")
            .args(["set-sink-input-volume", &id, "100%"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if muted || volume {
            fixed += 1;
        }
    }
    Ok(fixed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
Sink Input #22068
\tDriver: PipeWire
\tMute: yes
\tVolume: front-left: 65536 / 100%
\tapplication.name = \"moonclip\"
Sink Input #22970
\tMute: no
\tapplication.name = \"Brave\"
Sink Input #24102
\tMute: yes
\tapplication.name = \"moonclip\"
";

    #[test]
    fn finds_only_the_editor_streams() {
        assert_eq!(editor_stream_ids(SAMPLE), vec![22068, 24102]);
        assert!(editor_stream_ids("no streams").is_empty());
    }

    #[test]
    fn detects_streams_still_muted() {
        assert_eq!(editor_streams_muted(SAMPLE), 2);
        let clean = "Sink Input #1
	Mute: no
	application.name = \"moonclip\"
";
        assert_eq!(editor_streams_muted(clean), 0);
    }

    #[test]
    fn ignores_the_capture_engine_client() {
        let text = "\tapplication.name = \"MoonClip\"\n";
        assert!(editor_stream_ids(text).is_empty());
    }
}
