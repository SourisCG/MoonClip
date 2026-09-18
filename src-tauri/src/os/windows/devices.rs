//! Capture device enumeration (Windows): WASAPI endpoints, no sidecar.
//! Output (render) devices are game/desktop sources (loopback reads them);
//! input (capture) devices are microphones. Same item shape as
//! `os/linux/devices`; the AppHandle is unused here (WASAPI needs no sidecar)
//! but keeps one shared signature across backends.
//!
//! `id` is the WASAPI endpoint id (stable across renames); legacy
//! friendly-name settings still resolve by case-insensitive name.

use super::super::AudioDevice;
use tauri::AppHandle;
use wasapi::{Device, DeviceEnumerator, Direction};

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

pub async fn list_audio_devices(_app: &AppHandle) -> Result<Vec<AudioDevice>, String> {
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
    // Linux lists the same ids from GSR; Windows must offer them too or the
    // user can never go back to automatic after picking a concrete device.
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

/// Resolve a settings id to a WASAPI device: magic ids (and empty) = OS
/// default, endpoint ids by exact match, else legacy friendly name
/// (case-insensitive).
pub fn resolve_endpoint(id: &str, render: bool) -> Result<Device, String> {
    let en = enumerator()?;
    let direction = if render {
        Direction::Render
    } else {
        Direction::Capture
    };
    let wanted = id.trim();
    let magic = if render {
        is_default_output_id(wanted)
    } else {
        is_default_input_id(wanted)
    };
    if magic {
        return en.get_default_device(&direction).map_err(|e| {
            format!(
                "{} default device: {e}",
                if render { "output" } else { "input" }
            )
        });
    }
    if let Ok(d) = en.get_device(wanted) {
        return Ok(d);
    }
    if let Ok(coll) = en.get_device_collection(&direction) {
        let n = coll.get_nbr_devices().unwrap_or(0);
        for i in 0..n {
            if let Ok(d) = coll.get_device_at_index(i) {
                if d.get_friendlyname()
                    .map(|n| n.eq_ignore_ascii_case(wanted))
                    .unwrap_or(false)
                {
                    return Ok(d);
                }
            }
        }
    }
    Err(format!(
        "{} device not found: {wanted}",
        if render { "output" } else { "input" }
    ))
}

/// GSR magic ids (Linux defaults, also seeded into `settings` by migration
/// `003_devices.sql`). On Windows they mean "the OS default device".
pub fn is_default_output_id(id: &str) -> bool {
    matches!(id.trim(), "" | "default_output")
}

pub fn is_default_input_id(id: &str) -> bool {
    matches!(id.trim(), "" | "default_input")
}

#[cfg(test)]
mod tests {
    use super::{is_default_input_id, is_default_output_id};

    #[test]
    fn magic_ids_mean_default() {
        assert!(is_default_output_id(""));
        assert!(is_default_output_id("  "));
        assert!(is_default_output_id("default_output"));
        assert!(!is_default_output_id("default_input"));
        assert!(!is_default_output_id("Speakers (USB)"));
        assert!(is_default_input_id(""));
        assert!(is_default_input_id("default_input"));
        assert!(!is_default_input_id("default_output"));
        assert!(!is_default_input_id("Microphone (USB)"));
    }
}
