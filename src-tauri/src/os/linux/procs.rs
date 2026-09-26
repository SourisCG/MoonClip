//! Minimal process list for registered-app polling (read-only `/proc`).

use crate::os::shared::procs::ProcInfo;

pub fn running() -> Vec<ProcInfo> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let dir = e.path();
        let comm = std::fs::read_to_string(dir.join("comm"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if comm.is_empty() {
            continue;
        }
        let cmdline = read_cmdline(&dir.join("cmdline"));
        if cmdline.is_empty() {
            continue;
        }
        let exe = std::fs::read_link(dir.join("exe"))
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| cmdline[0].clone());
        out.push(ProcInfo {
            pid,
            comm,
            exe,
            cmdline,
        });
    }
    out
}

fn read_cmdline(p: &std::path::Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(p) else {
        return Vec::new();
    };
    bytes
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).to_string())
        .collect()
}
