//! Capture device enumeration (Linux): query OUR resolved GSR binary.
//! Output lines are `name|description`; kind follows GSR conventions.
//! The AppHandle resolves through the same walker as the engine
//! (env → bundled dev/resources → PATH), so dev staging works too.

use super::super::AudioDevice;
use super::binary;
use tauri::AppHandle;

pub async fn list_audio_devices(app: &AppHandle) -> Result<Vec<AudioDevice>, String> {
    let (bin, _source) = binary::backend_binary(app)?;
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
