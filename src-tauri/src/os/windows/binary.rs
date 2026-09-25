//! Backend binary resolution (Windows): the embedded capture engine
//! distribution. Looked up through the shared bundled-sidecar walker
//! (installed resources first, dev staging second); PATH is never used to
//! launch anything else (the user's own install is never touched).

use std::path::PathBuf;
use tauri::AppHandle;

/// Relative path of the engine binary inside the bundled sidecar directory
/// (staged by `build-aux/windows/fetch-obs.ps1` from the pinned upstream zip,
/// renamed so nothing matches the upstream product name).
pub const OBS_REL: &str = "engine/bin/64bit/moonclip-engine.exe";

/// Resolve the embedded engine binary.
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
        "capture engine not found (expected binaries/<triple>/engine/bin/64bit/moonclip-engine.exe). \
         Run build-aux/windows/fetch-obs.ps1 or reinstall MoonClip."
            .into(),
    )
}
