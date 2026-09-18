//! Capture device enumeration (Linux): PipeWire/PulseAudio via `pactl -f json`
//! (works on both PipeWire's pulse layer and plain PulseAudio). Outputs are
//! game/desktop sources (OBS `pulse_output_capture` reads the sink);
//! inputs are microphones. The embedded OBS opens the devices itself; this
//! module fills the Settings UI and resolves stored values to OBS device ids.

use super::super::AudioDevice;
use tauri::AppHandle;

#[derive(Debug, serde::Deserialize)]
struct Sink {
    name: String,
    #[serde(default)]
    description: String,
}

#[derive(Debug, serde::Deserialize)]
struct Source {
    name: String,
    #[serde(default)]
    description: String,
    /// Present (number) when the source is a sink monitor, i.e. not a mic.
    #[serde(default)]
    monitor_of_sink: Option<serde_json::Value>,
}

/// Pure parser: (sinks, sources, default sink name, default source name).
pub fn parse_devices(
    sinks: &str,
    sources: &str,
    default_sink: Option<&str>,
    default_source: Option<&str>,
) -> Vec<AudioDevice> {
    let mut devices = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut push = |id: String, description: String, kind: &str| {
        if seen.insert((id.clone(), kind.to_string())) {
            devices.push(AudioDevice {
                id,
                description,
                kind: kind.to_string(),
            });
        }
    };
    // Magic "follow the OS default" entries come first, like Windows.
    push(
        "default_output".into(),
        default_sink.unwrap_or_default().to_string(),
        "desktop",
    );
    push(
        "default_input".into(),
        default_source.unwrap_or_default().to_string(),
        "mic",
    );
    if let Ok(list) = serde_json::from_str::<Vec<Sink>>(sinks) {
        for s in list {
            push(s.name, s.description, "desktop");
        }
    }
    if let Ok(list) = serde_json::from_str::<Vec<Source>>(sources) {
        for s in list {
            if s.monitor_of_sink.is_some() {
                continue; // monitor of a sink = desktop tap, not a mic
            }
            push(s.name, s.description, "mic");
        }
    }
    devices
}

async fn pactl_json(args: &[&str]) -> Result<String, String> {
    let out = tokio::process::Command::new("pactl")
        .args(["-f", "json"])
        .args(args)
        .output()
        .await
        .map_err(|e| format!("pactl not available: {e}"))?;
    if !out.status.success() {
        return Err(format!("pactl {} failed", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

async fn default_name(kind: &str) -> Option<String> {
    let out = tokio::process::Command::new("pactl")
        .args(["get-default-", kind])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}

async fn enumerate() -> Result<Vec<AudioDevice>, String> {
    let sinks = pactl_json(&["list", "sinks"]).await?;
    let sources = pactl_json(&["list", "sources"]).await?;
    let default_sink = default_name("sink").await;
    let default_source = default_name("source").await;
    let devices = parse_devices(
        &sinks,
        &sources,
        default_sink.as_deref(),
        default_source.as_deref(),
    );
    if devices.len() <= 2 {
        return Err("no PulseAudio/PipeWire devices found".into());
    }
    Ok(devices)
}

pub async fn list_audio_devices(_app: &AppHandle) -> Result<Vec<AudioDevice>, String> {
    enumerate().await
}

/// Pure resolution: stored setting + enumerated devices -> OBS device id.
/// OBS's pulse sources take the sink/source name or `default`.
pub fn match_device_id(stored: &str, devices: &[AudioDevice], render: bool) -> String {
    let t = stored.trim();
    let magic = t.is_empty()
        || t == "default"
        || (render && t == "default_output")
        || (!render && t == "default_input");
    if magic {
        return "default".to_string();
    }
    let kind = if render { "desktop" } else { "mic" };
    if let Some(d) = devices.iter().find(|d| d.id == t && d.kind == kind) {
        return d.id.clone();
    }
    if let Some(d) = devices
        .iter()
        .find(|d| d.kind == kind && d.description.eq_ignore_ascii_case(t))
    {
        return d.id.clone();
    }
    "default".to_string()
}

/// Stored setting -> OBS device id, resolving legacy friendly names.
pub async fn resolve_obs_device_id(stored: &str, render: bool) -> String {
    match enumerate().await {
        Ok(devices) => match_device_id(stored, &devices, render),
        Err(_) => "default".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::AudioDevice;
    use super::{match_device_id, parse_devices};

    const SINKS: &str = r#"[
        {"index": 0, "name": "alsa_output.pci-0000_00_1f.3.analog-stereo", "description": "Built-in Audio Analog Stereo"}
    ]"#;
    const SOURCES: &str = r#"[
        {"index": 0, "name": "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor", "description": "Monitor of Built-in Audio", "monitor_of_sink": 0},
        {"index": 1, "name": "alsa_input.usb-Mic-00.analog-stereo", "description": "USB Mic", "monitor_of_sink": null}
    ]"#;

    fn dev(id: &str, desc: &str, kind: &str) -> AudioDevice {
        AudioDevice {
            id: id.into(),
            description: desc.into(),
            kind: kind.into(),
        }
    }

    #[test]
    fn parser_lists_defaults_sinks_and_real_mics() {
        let devices = parse_devices(SINKS, SOURCES, Some("alsa_output"), Some("alsa_input"));
        let ids: Vec<&str> = devices.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids[0], "default_output");
        assert_eq!(ids[1], "default_input");
        assert!(ids.contains(&"alsa_output.pci-0000_00_1f.3.analog-stereo"));
        assert!(ids.contains(&"alsa_input.usb-Mic-00.analog-stereo"));
        assert!(!ids.contains(&"alsa_output.pci-0000_00_1f.3.analog-stereo.monitor"));
        assert_eq!(devices[0].kind, "desktop");
        assert_eq!(devices[1].kind, "mic");
    }

    #[test]
    fn parser_survives_empty_and_garbage() {
        assert_eq!(parse_devices("", "", None, None).len(), 2);
        assert_eq!(parse_devices("not json", "[]", None, None).len(), 2);
    }

    #[test]
    fn magic_ids_pass_through() {
        assert_eq!(match_device_id("default_output", &[], true), "default");
        assert_eq!(match_device_id("", &[], true), "default");
        assert_eq!(
            match_device_id("alsa_input.usb-Mic", &[], false),
            "alsa_input.usb-Mic"
        );
    }

    #[test]
    fn legacy_names_and_descriptions_resolve() {
        let devices = vec![
            dev("default_output", "alsa_output", "desktop"),
            dev("default_input", "alsa_input", "mic"),
            dev(
                "alsa_output.pci-0000_00_1f.3.analog-stereo",
                "Built-in Audio Analog Stereo",
                "desktop",
            ),
            dev("alsa_input.usb-Mic-00.analog-stereo", "USB Mic", "mic"),
        ];
        assert_eq!(
            match_device_id("alsa_output.pci-0000_00_1f.3.analog-stereo", &devices, true),
            "alsa_output.pci-0000_00_1f.3.analog-stereo"
        );
        assert_eq!(
            match_device_id("Built-in Audio Analog Stereo", &devices, true),
            "alsa_output.pci-0000_00_1f.3.analog-stereo"
        );
        assert_eq!(
            match_device_id("USB Mic", &devices, false),
            "alsa_input.usb-Mic-00.analog-stereo"
        );
        assert_eq!(match_device_id("default_output", &devices, true), "default");
        assert_eq!(match_device_id("Speakers (USB)", &devices, true), "default");
    }
}
