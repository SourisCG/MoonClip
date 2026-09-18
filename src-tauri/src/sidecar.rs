//! Sidecar path resolution (OS-free): bundled binaries first, system fallback.
//! Layout: <res|exe>/binaries/<triple>/… (obs/, obs-cmd, ffmpeg-<triple>).
//! Dev layout: src-tauri/binaries/<triple>/ (walked up from target/debug/<bin>).
//! OS-specific bits (device lists, binary resolution) live under os/, never here.

use std::path::PathBuf;
use tauri::{AppHandle, Manager};

pub fn host_triple() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        (_, "x86_64") => "x86_64-unknown-linux-gnu",
        (_, "aarch64") => "aarch64-unknown-linux-gnu",
        _ => "unknown",
    }
}

fn find_in(dir: &std::path::Path, triple: &str, name: &str) -> Option<PathBuf> {
    let p = dir.join("binaries").join(triple).join(name);
    p.exists().then_some(p)
}

/// Search resource dir, exe dir (+ ancestors for dev layout: target/debug/<bin>).
/// OS-free path walk. OS-specific binary resolution lives in `os/*/binary.rs`.
pub(crate) fn search_bundled(name: &str, app: &AppHandle) -> Option<(PathBuf, &'static str)> {
    let triple = host_triple();
    if let Ok(res) = app.path().resource_dir() {
        if let Some(p) = find_in(&res, triple, name) {
            return Some((p, "bundled"));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(|p| p.to_path_buf());
        for _ in 0..5 {
            let Some(d) = dir else { break };
            if let Some(p) = find_in(&d, triple, name) {
                return Some((p, "bundled"));
            }
            // Flat dev layout: src-tauri/binaries/<triple> next to Cargo project.
            if d.ends_with("target/debug") || d.ends_with("target/release") {
                if let Some(root) = d.parent().and_then(|p| p.parent()) {
                    let p = root.join("binaries").join(triple).join(name);
                    if p.exists() {
                        return Some((p, "bundled"));
                    }
                }
            }
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }
    None
}
