//! Capture device enumeration (Windows): `cpal` hosts, no sidecar.
//! Output (render) devices are game/desktop sources (WASAPI loopback reads
//! them); input (capture) devices are microphones. Same item shape as
//! `os/linux/devices`; the AppHandle is unused here (cpal needs no sidecar)
//! but keeps one shared signature across backends.

use super::super::AudioDevice;
use cpal::traits::{DeviceTrait, HostTrait};
use tauri::AppHandle;

/// Friendly device name, or `None` when the OS will not name it.
fn dev_name(d: &cpal::Device) -> Option<String> {
    d.name()
        .ok()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
}

pub async fn list_audio_devices(_app: &AppHandle) -> Result<Vec<AudioDevice>, String> {
    // Device enumeration is quick; keep it sync-shaped inside async for
    // signature parity with the Linux backend.
    let host = cpal::default_host();
    let mut devices = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut push = |id: String, kind: &str| {
        if seen.insert((id.clone(), kind.to_string())) {
            devices.push(AudioDevice {
                id: id.clone(),
                description: id,
                kind: kind.to_string(),
            });
        }
    };
    match host.output_devices() {
        Ok(list) => {
            for d in list {
                if let Some(n) = dev_name(&d) {
                    push(n, "desktop");
                }
            }
        }
        Err(e) => return Err(format!("cannot list output devices: {e}")),
    }
    match host.input_devices() {
        Ok(list) => {
            for d in list {
                if let Some(n) = dev_name(&d) {
                    push(n, "mic");
                }
            }
        }
        Err(e) => return Err(format!("cannot list input devices: {e}")),
    }
    if devices.is_empty() {
        return Err("no audio devices found".into());
    }
    Ok(devices)
}

/// Resolve a `cpal` device by the id `list_audio_devices` returned
/// (the OS friendly name). `render=true` searches outputs (loopback game
/// capture), `render=false` searches inputs (mic).
pub fn find_device(id: &str, render: bool) -> Option<cpal::Device> {
    let host = cpal::default_host();
    let list = if render {
        host.output_devices().ok()?
    } else {
        host.input_devices().ok()?
    };
    list.into_iter().find(|d| dev_name(d).as_deref() == Some(id))
}

/// GSR magic ids (Linux defaults, also seeded into `settings` by migration
/// `003_devices.sql`). On Windows they mean "the OS default device" — cpal
/// friendly names never equal them, so they must be intercepted before the
/// by-name search or stock installs can never link audio.
pub fn is_default_output_id(id: &str) -> bool {
    matches!(id.trim(), "" | "default_output")
}

pub fn is_default_input_id(id: &str) -> bool {
    matches!(id.trim(), "" | "default_input")
}

/// Output (render) device for a game id: default endpoint for empty/GSR
/// magic ids, by-name lookup otherwise.
pub fn find_output_device(id: &str) -> Option<cpal::Device> {
    if is_default_output_id(id) {
        return cpal::default_host().default_output_device();
    }
    find_device(id.trim(), true)
}

/// Input (capture) device for a mic id: default endpoint for empty/GSR
/// magic ids, by-name lookup otherwise.
pub fn find_input_device(id: &str) -> Option<cpal::Device> {
    if is_default_input_id(id) {
        return cpal::default_host().default_input_device();
    }
    find_device(id.trim(), false)
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
