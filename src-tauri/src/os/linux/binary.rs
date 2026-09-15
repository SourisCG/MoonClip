//! Backend binary resolution (Linux): bundled sidecar first, system fallback.
//! The `.exe`/native side of this lives in `os/windows/binary.rs` (which
//! correctly reports "native, nothing to resolve").

use std::path::PathBuf;
use tauri::AppHandle;

/// Resolve the GSR binary: MOONCLIP_GSR_BIN -> bundled sidecar -> system PATH.
pub fn backend_binary(app: &AppHandle) -> Result<(PathBuf, &'static str), String> {
    if let Ok(path) = std::env::var("MOONCLIP_GSR_BIN") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return Ok((p, "env"));
        }
        return Err(format!("MOONCLIP_GSR_BIN points nowhere: {path}"));
    }
    if let Some(found) = crate::sidecar::search_bundled("gpu-screen-recorder", app) {
        return Ok(found);
    }
    // No `sh` assumption beyond POSIX (Linux-only module).
    let out = std::process::Command::new("sh")
        .args(["-c", "command -v gpu-screen-recorder"])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !p.is_empty() {
            return Ok((PathBuf::from(p), "system"));
        }
    }
    Err("gpu-screen-recorder not bundled and not installed. Rebuild with build-aux/build-gsr.sh.".into())
}
