//! KMS capability check + one-click fix (Linux only).
//! Upstream GSR checks `cap_sys_admin` on `gsr-kms-server` (not on the main
//! binary) and, when missing, launches the helper through `pkexec` — which
//! prompts for admin authentication on every capture start.

use std::path::{Path, PathBuf};

/// The binary that actually needs the cap: the KMS helper next to GSR.
/// Falls back to the given path if no sibling exists (legacy layouts).
fn kms_server_path(gsr_bin: &Path) -> PathBuf {
    let sibling = gsr_bin.with_file_name("gsr-kms-server");
    if sibling.exists() {
        sibling
    } else {
        gsr_bin.to_path_buf()
    }
}

/// Does the KMS helper carry cap_sys_admin (KMS capture without polkit prompts)?
pub fn caps_ok(path: &Path) -> bool {
    let target = kms_server_path(path);
    let Ok(out) = std::process::Command::new("getcap").arg(&target).output() else {
        return false;
    };
    String::from_utf8_lossy(&out.stdout).contains("cap_sys_admin")
}

/// One-click fix: pkexec setcap on OUR bundled KMS helper (polkit dialog, once).
pub async fn fix_caps(path: &Path) -> Result<(), String> {
    let target = kms_server_path(path);
    let status = tokio::process::Command::new("pkexec")
        .args(["setcap", "cap_sys_admin+ep", &target.to_string_lossy()])
        .status()
        .await
        .map_err(|e| format!("pkexec failed: {e}"))?;
    if !status.success() {
        return Err("setcap rejected or failed".into());
    }
    if !caps_ok(&target) {
        return Err("capability still missing after setcap".into());
    }
    Ok(())
}
