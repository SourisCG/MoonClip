//! Running-process polling shared by every platform: a tiny read-only
//! snapshot used to match registered games. No hooks, no windowing.

#[derive(Debug, Clone)]
pub struct ProcInfo {
    /// Diagnostic only (the matcher never needs it).
    #[allow(dead_code)]
    pub pid: u32,
    pub comm: String,
    pub exe: String,
    pub cmdline: Vec<String>,
    /// Holds a GPU device FD (`/dev/dri/renderD*`, `/dev/nvidia*`).
    pub uses_gpu: bool,
}

/// Noise that must never reach the Games picker (browsers, Electron, build
/// tools, Proton plumbing). The list stays short on purpose.
const PICKER_BLACKLIST: &[&str] = &[
    "brave", "brave-browser", "chrome", "chromium", "chromium-browser", "firefox",
    "google-chrome", "discord", "vesktop", "code", "codium", "node", "electron",
    "steamwebhelper", "gameoverlayui", "rust-analyzer", "rust-analyzer-proc-macro-srv",
    "reaper", "srt-bwrap", "pv-adverb", "pv-runtime", "pressure-vessel",
    "python3", "python3.11", "python3.12", "python3.13", "mangohud", "gamescope",
];

/// Should this process be offered in the Games picker? Only real apps: GPU
/// holders or Windows `.exe` games, minus the noise blacklist.
pub fn is_selectable(p: &ProcInfo) -> bool {
    let game_exe = game_exe_name(p);
    let low = game_exe.to_lowercase();
    if low.is_empty() || low.starts_with("moonclip") {
        return false;
    }
    let known_game = p.cmdline.iter().any(|a| a.to_lowercase().ends_with(".exe"));
    if !(p.uses_gpu || known_game) {
        return false;
    }
    let base = basename(&p.exe).to_lowercase();
    !PICKER_BLACKLIST
        .iter()
        .any(|b| base == *b || low == *b)
}

fn basename(s: &str) -> String {
    s.rsplit(['/', '\\']).next().unwrap_or(s).to_string()
}

/// Display/registration name for a process: Wine/Proton wrappers show the
/// game's `.exe` (from the cmdline) instead of `wine-preloader`.
pub fn game_exe_name(p: &ProcInfo) -> String {
    let base = basename(&p.exe);
    let low = base.to_lowercase();
    if low.starts_with("wine") || low.contains("pressure-vessel") {
        if let Some(exe) = p
            .cmdline
            .iter()
            .find(|a| a.to_lowercase().ends_with(".exe"))
        {
            return basename(exe);
        }
    }
    base
}

/// Rule-based match shared by the picker and the autopilot.
pub fn rule_matches(strategy: &str, target_exe: &str, p: &ProcInfo) -> bool {
    let target = target_exe.trim().to_lowercase();
    if target.is_empty() {
        return false;
    }
    match strategy {
        "exact_exe" => {
            let want = basename(&target);
            basename(&p.exe).to_lowercase() == want || p.comm.to_lowercase() == want
        }
        "cmdline_contains" => p.cmdline.iter().any(|a| a.to_lowercase().contains(&target)),
        "wine_target" => {
            let want = basename(&target);
            p.cmdline
                .iter()
                .any(|a| basename(&a.to_lowercase()) == want)
        }
        _ => false,
    }
}

/// First registered INPUT (ground truth for the autopilot) whose rule matches
/// a running process.
pub fn first_match_registered<'a>(
    inputs: &'a [crate::storage::models::RegisteredInput],
    procs: &[ProcInfo],
) -> Option<&'a crate::storage::models::RegisteredInput> {
    inputs.iter().find(|i| {
        i.input_kind == "window"
            && !i.target_exe.trim().is_empty()
            && procs
                .iter()
                .any(|p| rule_matches(&i.match_strategy, &i.target_exe, p))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc_info(comm: &str, exe: &str, cmdline: &[&str]) -> ProcInfo {
        ProcInfo {
            pid: 7,
            comm: comm.into(),
            exe: exe.into(),
            cmdline: cmdline.iter().map(|s| s.to_string()).collect(),
            uses_gpu: true,
        }
    }

    #[test]
    fn matches_registered_rules() {
        let p = proc_info("fnaf.exe", "Z:\\games\\fnaf.exe", &["Z:\\games\\fnaf.exe"]);
        assert!(rule_matches("exact_exe", "fnaf.exe", &p));
        assert!(rule_matches("wine_target", "fnaf.exe", &p));
        assert!(rule_matches("cmdline_contains", "games\\fnaf", &p));
        assert!(!rule_matches("exact_exe", "other.exe", &p));
        assert!(!rule_matches("window_title", "fnaf", &p));
        assert!(!rule_matches("exact_exe", "", &p));
    }

    #[test]
    fn wine_wrappers_report_the_game_exe() {
        let p = proc_info(
            "wine-preloader",
            "/usr/bin/wine-preloader",
            &["Z:\\games\\KINGDOM HEARTS FINAL MIX.exe", "-steam"],
        );
        assert_eq!(game_exe_name(&p), "KINGDOM HEARTS FINAL MIX.exe");
        let native = proc_info("fnaf.exe", "/games/fnaf.exe", &["/games/fnaf.exe"]);
        assert_eq!(game_exe_name(&native), "fnaf.exe");
    }

    #[test]
    fn picker_filter_hides_noise_and_keeps_games() {
        let game = proc_info("fnaf.exe", "Z:\\games\\fnaf.exe", &["Z:\\games\\fnaf.exe"]);
        assert!(is_selectable(&game));
        let browser = proc_info("brave", "/usr/bin/brave", &["/usr/bin/brave"]);
        assert!(!is_selectable(&browser));
        let mut no_gpu = proc_info("mytool", "/opt/mytool", &["/opt/mytool"]);
        no_gpu.uses_gpu = false;
        assert!(!is_selectable(&no_gpu));
        let mut editor = proc_info("someeditor", "/home/u/someeditor", &["/home/u/someeditor"]);
        editor.uses_gpu = true;
        assert!(is_selectable(&editor));
    }

    #[test]
    fn first_match_registered_follows_list_order() {
        let procs = vec![
            proc_info("moonclip-testgame", "/tmp/moonclip-testgame", &[]),
            proc_info("other", "/usr/bin/other", &[]),
        ];
        let row = |name: &str, exe: &str| crate::storage::models::RegisteredInput {
            id: name.into(),
            input_name: name.into(),
            input_kind: "window".into(),
            display_name: name.into(),
            target_exe: exe.into(),
            match_strategy: "exact_exe".into(),
            input_settings: None,
            source_uuid: "u".into(),
            icon_path: None,
        };
        let inputs = vec![row("Other", "other"), row("Testgame", "moonclip-testgame")];
        assert_eq!(
            first_match_registered(&inputs, &procs).unwrap().display_name,
            "Other"
        );
    }
}
