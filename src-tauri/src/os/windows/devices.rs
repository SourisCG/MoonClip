//! Capture device enumeration (Windows): WASAPI endpoints.
//! Output (render) devices are game/desktop sources; input (capture) devices
//! are microphones. The embedded OBS opens the endpoints itself; this module
//! fills the Settings UI and resolves stored values to OBS `device_id`s.
//!
//! OBS wants the WASAPI endpoint id (stable across renames) or `default`.
//! Legacy settings may hold a friendly name ("Speakers (USB Audio Device)"),
//! which OBS cannot enumerate (`0x80070057`); `resolve_obs_device_id` maps
//! those to the endpoint id or safe `default`.

use super::super::AudioDevice;
use tauri::AppHandle;
use wasapi::{DeviceEnumerator, Direction};

fn enumerator() -> Result<DeviceEnumerator, String> {
    let _ = wasapi::initialize_mta();
    DeviceEnumerator::new().map_err(|e| format!("audio enumerator: {e}"))
}

fn kind_of(direction: &Direction) -> &'static str {
    match direction {
        Direction::Render => "desktop",
        Direction::Capture => "mic",
    }
}

fn enumerate_blocking() -> Result<Vec<AudioDevice>, String> {
    let en = enumerator()?;
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
    // Magic entries first: "follow the OS default" (resolved again at every
    // buffer start, so a Windows output switch is tracked automatically).
    let default_out = en
        .get_default_device(&Direction::Render)
        .ok()
        .and_then(|d| d.get_friendlyname().ok())
        .unwrap_or_default();
    push("default_output".into(), default_out, "desktop");
    let default_in = en
        .get_default_device(&Direction::Capture)
        .ok()
        .and_then(|d| d.get_friendlyname().ok())
        .unwrap_or_default();
    push("default_input".into(), default_in, "mic");
    for direction in [Direction::Render, Direction::Capture] {
        let Ok(coll) = en.get_device_collection(&direction) else {
            continue;
        };
        let n = coll.get_nbr_devices().unwrap_or(0);
        for i in 0..n {
            let Ok(d) = coll.get_device_at_index(i) else {
                continue;
            };
            let (Ok(id), Ok(name)) = (d.get_id(), d.get_friendlyname()) else {
                continue;
            };
            push(id, name, kind_of(&direction));
        }
    }
    if devices.is_empty() {
        return Err("no audio devices found".into());
    }
    Ok(devices)
}

pub async fn list_audio_devices(_app: &AppHandle) -> Result<Vec<AudioDevice>, String> {
    tokio::task::spawn_blocking(enumerate_blocking)
        .await
        .map_err(|e| format!("audio enumeration task failed: {e}"))?
}

/// Pure resolution: stored setting + enumerated devices -> OBS `device_id`.
pub fn match_device_id(stored: &str, devices: &[AudioDevice], render: bool) -> String {
    let t = stored.trim();
    let magic = t.is_empty()
        || t == "default"
        || (render && is_default_output_id(t))
        || (!render && is_default_input_id(t));
    if magic {
        return "default".to_string();
    }
    // Already an endpoint id (`{0.0.0.00000000}.{guid}`).
    if t.starts_with("{0.0.0.") {
        return t.to_string();
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

/// Stored setting -> OBS `device_id`, resolving legacy friendly names.
pub async fn resolve_obs_device_id(stored: &str, render: bool) -> String {
    let stored = stored.to_string();
    tokio::task::spawn_blocking(move || match enumerate_blocking() {
        Ok(devices) => match_device_id(&stored, &devices, render),
        // Enumeration failure must not block a start: OBS's default device.
        Err(_) => "default".to_string(),
    })
    .await
    .unwrap_or_else(|_| "default".to_string())
}

/// Magic ids (Linux defaults, also seeded into `settings` by migration
/// `003_devices.sql`). On Windows they mean "the OS default device".
pub fn is_default_output_id(id: &str) -> bool {
    matches!(id.trim(), "" | "default_output" | "default")
}

pub fn is_default_input_id(id: &str) -> bool {
    matches!(id.trim(), "" | "default_input" | "default")
}

#[cfg(test)]
mod tests {
    use super::AudioDevice;
    use super::{is_default_input_id, is_default_output_id, match_device_id};
    fn dev(id: &str, desc: &str, kind: &str) -> AudioDevice {
        AudioDevice {
            id: id.into(),
            description: desc.into(),
            kind: kind.into(),
        }
    }

    #[test]
    fn magic_ids_mean_default() {
        assert!(is_default_output_id(""));
        assert!(is_default_output_id("  "));
        assert!(is_default_output_id("default_output"));
        assert!(is_default_output_id("default"));
        assert!(!is_default_output_id("default_input"));
        assert!(!is_default_output_id("Speakers (USB)"));
        assert!(is_default_input_id(""));
        assert!(is_default_input_id("default_input"));
        assert!(is_default_input_id("default"));
        assert!(!is_default_input_id("default_output"));
        assert!(!is_default_input_id("Microphone (USB)"));
    }

    #[test]
    fn magic_and_endpoint_ids_pass_through() {
        assert_eq!(match_device_id("", &[], true), "default");
        assert_eq!(match_device_id("default_output", &[], true), "default");
        assert_eq!(match_device_id("default_input", &[], false), "default");
        assert_eq!(
            match_device_id("{0.0.0.00000000}.{abc}", &[], true),
            "{0.0.0.00000000}.{abc}"
        );
    }

    #[test]
    fn legacy_friendly_names_resolve_to_endpoint_ids() {
        let devices = vec![
            dev("default_output", "Speakers", "desktop"),
            dev("default_input", "Microphone", "mic"),
            dev(
                "{0.0.0.00000000}.{speakers}",
                "Speakers (USB Audio Device)",
                "desktop",
            ),
            dev("{0.0.0.00000000}.{mic}", "Microphone (USB Audio)", "mic"),
        ];
        // The observed failure: a friendly name passed straight to OBS.
        assert_eq!(
            match_device_id("Speakers (USB Audio Device)", &devices, true),
            "{0.0.0.00000000}.{speakers}"
        );
        assert_eq!(
            match_device_id("Microphone (USB Audio)", &devices, false),
            "{0.0.0.00000000}.{mic}"
        );
        // Endpoint ids pass through; magic maps to default.
        assert_eq!(
            match_device_id("{0.0.0.00000000}.{speakers}", &devices, true),
            "{0.0.0.00000000}.{speakers}"
        );
        assert_eq!(match_device_id("default_output", &devices, true), "default");
        // Unknown legacy value (e.g. GSR-era name) falls back to default.
        assert_eq!(
            match_device_id("alsa_output.pci-0000", &devices, true),
            "default"
        );
        // Kind is respected: a mic name never resolves as a desktop device.
        assert_eq!(
            match_device_id("Microphone (USB Audio)", &devices, true),
            "default"
        );
    }
}
