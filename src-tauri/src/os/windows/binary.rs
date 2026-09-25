//! Backend binary resolution (Windows): the embedded OBS Studio distribution.
//! Looked up through the shared bundled-sidecar walker (installed resources
//! first, dev staging second); PATH is only a loud dev fallback and is never
//! used to launch a user's OBS.

use std::path::PathBuf;
use tauri::AppHandle;

/// Relative path of the OBS binary inside the bundled sidecar directory.
pub const OBS_REL: &str = "obs/bin/64bit/obs64.exe";

/// Resolve the embedded OBS (`obs64.exe`).
pub fn resolve_obs(app: &AppHandle) -> Result<(PathBuf, &'static str), String> {
    if let Ok(path) = std::env::var("MOONCLIP_OBS_BIN") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return Ok((p, "env"));
        }
        return Err(format!("MOONCLIP_OBS_BIN points nowhere: {path}"));
    }
    if let Some(found) = crate::sidecar::search_bundled(OBS_REL, app) {
        return Ok(found);
    }
    Err(
        "embedded OBS not found (expected binaries/<triple>/obs/bin/64bit/obs64.exe). \
         Run build-aux/windows/fetch-obs.ps1 or reinstall MoonClip."
            .into(),
    )
}
