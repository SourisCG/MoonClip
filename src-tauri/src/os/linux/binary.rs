//! Linux backend binary resolution: the embedded OBS Studio distribution and
//! the obs-cmd CLI. Bundled sidecar first (installer layout), dev staging
//! second. System OBS is a last-resort fallback: it is still launched with
//! `--config-dir` + `--multi`, so the user's own config is never touched
//! (enforced by the config guard in the engine).

use std::path::PathBuf;
use tauri::AppHandle;

/// Relative path of the OBS binary inside the bundled sidecar directory
/// (portable layout: `bin/obs`, `lib/`, `share/obs`).
pub const OBS_REL: &str = "obs/bin/obs";
/// Relative path of the obs-cmd binary inside the bundled sidecar directory.
pub const OBSCMD_REL: &str = "obs-cmd";

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
    // System OBS (distro package / flatpak wrapper). Still isolated at launch.
    if let Ok(out) = std::process::Command::new("sh")
        .args(["-c", "command -v obs"])
        .output()
    {
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() {
                return Ok((PathBuf::from(p), "system"));
            }
        }
    }
    Err(
        "embedded OBS not found (expected binaries/<triple>/obs/bin/obs) and no \
         system `obs` in PATH. Run build-aux/fetch-obs.sh or install OBS."
            .into(),
    )
}

pub fn resolve_obscmd(app: &AppHandle) -> Result<(PathBuf, &'static str), String> {
    if let Ok(path) = std::env::var("MOONCLIP_OBSCMD_BIN") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return Ok((p, "env"));
        }
        return Err(format!("MOONCLIP_OBSCMD_BIN points nowhere: {path}"));
    }
    if let Some(found) = crate::sidecar::search_bundled(OBSCMD_REL, app) {
        return Ok(found);
    }
    Err(
        "embedded obs-cmd not found (expected binaries/<triple>/obs-cmd). \
         Run build-aux/fetch-obs.sh or reinstall MoonClip."
            .into(),
    )
}
