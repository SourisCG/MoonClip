//! Linux backend binary resolution: the embedded capture engine distribution.
//! Bundled sidecar first (installer layout), dev staging second. There is NO
//! PATH/system fallback on purpose: launching the user's own OBS would expose
//! the real product identity and its single-instance socket.

use std::path::PathBuf;
use tauri::AppHandle;

/// Relative path of the engine binary inside the bundled sidecar directory
/// (portable layout: `bin/moonclip-engine`, `lib*/`, `share/obs`).
pub const OBS_REL: &str = "engine/bin/moonclip-engine";

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
        "capture engine not found (expected binaries/<triple>/engine/bin/moonclip-engine). \
         Run build-aux/linux/build-obs.sh or reinstall MoonClip."
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::OBS_REL;

    /// The engine identity must never leak the upstream product name.
    #[test]
    fn engine_relative_path_is_neutral() {
        assert!(
            !OBS_REL.to_lowercase().contains("obs"),
            "engine path leaks the upstream name: {OBS_REL}"
        );
    }
}
