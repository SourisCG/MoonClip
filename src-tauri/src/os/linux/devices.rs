//! Capture device enumeration (Linux): query OUR bundled GSR binary.
//! Output lines are `name|description`; kind follows GSR conventions.
//! Argless like the Windows backend so shared code never branches: the
//! binary resolves internally (MOONCLIP_GSR_BIN → bundled sidecar → PATH),
//! exactly as engine start does.

use super::super::AudioDevice;
use super::gsr::LinuxGsrEngine;

pub async fn list_audio_devices() -> Result<Vec<AudioDevice>, String> {
    let bin = LinuxGsrEngine::resolve_binary()?;
    let out = tokio::process::Command::new(&bin)
        .arg("--list-audio-devices")
        .output()
        .await
        .map_err(|e| format!("cannot list audio devices: {e}"))?;
    if !out.status.success() {
        return Err("GSR device list failed".into());
    }
    let mut devices = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (id, desc) = match line.split_once('|') {
            Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
            None => (line.to_string(), line.to_string()),
        };
        if id.is_empty() {
            continue;
        }
        let lower = id.to_lowercase();
        let kind = if lower.contains("monitor") || lower == "default_output" {
            "desktop"
        } else {
            "mic"
        };
        devices.push(AudioDevice {
            id,
            description: desc,
            kind: kind.to_string(),
        });
    }
    Ok(devices)
}
