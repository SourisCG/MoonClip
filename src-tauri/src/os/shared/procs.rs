//! Running-process polling shared by every platform: a tiny read-only
//! snapshot used to match registered games. No hooks, no windowing.

use crate::storage::models::CustomApp;

#[derive(Debug, Clone)]
pub struct ProcInfo {
    /// Diagnostic only (the matcher never needs it).
    #[allow(dead_code)]
    pub pid: u32,
    pub comm: String,
    pub exe: String,
    pub cmdline: Vec<String>,
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

/// Does a registered app rule match this process? `window_title` needs
/// compositor window info and is not supported by the simple poller.
pub fn app_matches(app: &CustomApp, p: &ProcInfo) -> bool {
    let target = app.target_exe.trim().to_lowercase();
    if target.is_empty() {
        return false;
    }
    match app.match_strategy.as_str() {
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

/// First registered app with a running process (list order wins).
pub fn first_match<'a>(apps: &'a [CustomApp], procs: &[ProcInfo]) -> Option<&'a CustomApp> {
    apps.iter()
        .find(|a| procs.iter().any(|p| app_matches(a, p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(strategy: &str, target: &str) -> CustomApp {
        CustomApp {
            id: "1".into(),
            display_name: "Game".into(),
            target_exe: target.into(),
            match_strategy: strategy.into(),
            clip_duration_seconds: None,
            icon_path: None,
            is_wine_proton: false,
            portal_token: None,
        }
    }

    fn proc_info(comm: &str, exe: &str, cmdline: &[&str]) -> ProcInfo {
        ProcInfo {
            pid: 7,
            comm: comm.into(),
            exe: exe.into(),
            cmdline: cmdline.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn matches_registered_rules() {
        let p = proc_info("fnaf.exe", "Z:\\games\\fnaf.exe", &["Z:\\games\\fnaf.exe"]);
        assert!(app_matches(&app("exact_exe", "fnaf.exe"), &p));
        assert!(app_matches(&app("wine_target", "fnaf.exe"), &p));
        assert!(app_matches(&app("cmdline_contains", "games\\fnaf"), &p));
        assert!(!app_matches(&app("exact_exe", "other.exe"), &p));
        assert!(!app_matches(&app("window_title", "fnaf"), &p));
        assert!(!app_matches(&app("exact_exe", ""), &p));
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
    fn first_match_follows_list_order() {
        let procs = vec![
            proc_info("moonclip-testgame", "/tmp/moonclip-testgame", &[]),
            proc_info("other", "/usr/bin/other", &[]),
        ];
        let apps = vec![
            app("exact_exe", "other"),
            app("exact_exe", "moonclip-testgame"),
        ];
        assert_eq!(first_match(&apps, &procs).unwrap().target_exe, "other");
    }
}
