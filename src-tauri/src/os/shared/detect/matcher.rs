//! Custom-app matching: a row the user registered (or that stores per-game
//! prefs) always wins over manifest detection. Strategies are evaluated in a
//! fixed precedence so ambiguous rows resolve deterministically.

use crate::storage::models::CustomApp;

use super::parsers::parse_prism_game_dir;
use super::resolve::{ResolvedCandidate, SRC_CUSTOM};
use super::types::CandidateProcess;

/// Higher priority first.
pub const STRATEGY_PRECEDENCE: &[&str] = &[
    "exact_exe",
    "steam_appid",
    "prism_instance",
    "wine_target",
    "cmdline_contains",
    "window_title",
];

fn basename(s: &str) -> String {
    s.rsplit(['/', '\\']).next().unwrap_or(s).to_string()
}

/// True when the registered rule matches this process. Auto-created rows use
/// the `auto` strategy and never match here (they are keyed, not rules).
pub fn strategy_matches(app: &CustomApp, c: &CandidateProcess) -> bool {
    let target = app.target_exe.trim().to_lowercase();
    if target.is_empty() {
        return false;
    }
    match app.match_strategy.as_str() {
        "exact_exe" => {
            c.exe_basename() == basename(&target) || c.exe.to_lowercase() == target
        }
        "cmdline_contains" => c.cmdline.iter().any(|a| a.to_lowercase().contains(&target)),
        "window_title" => c
            .window
            .as_ref()
            .map(|w| w.name.to_lowercase().contains(&target))
            .unwrap_or(false),
        "wine_target" => {
            c.is_wine
                && c.cmdline
                    .iter()
                    .any(|a| basename(&a.to_lowercase()) == basename(&target))
        }
        "steam_appid" => c
            .steam_app_id
            .map(|id| id.to_string() == target)
            .unwrap_or(false),
        "prism_instance" => parse_prism_game_dir(&c.cmdline)
            .map(|i| i.to_lowercase() == target)
            .unwrap_or(false),
        _ => false,
    }
}

/// First registered app matching the candidate, honouring the precedence.
pub fn best_custom<'a>(apps: &'a [CustomApp], c: &CandidateProcess) -> Option<&'a CustomApp> {
    STRATEGY_PRECEDENCE.iter().find_map(|strategy| {
        apps.iter()
            .find(|a| a.match_strategy == *strategy && strategy_matches(a, c))
    })
}

/// Apply the user's registration (if any) over a manifest-resolved candidate.
pub fn with_custom(
    mut resolved: ResolvedCandidate,
    c: &CandidateProcess,
    apps: &[CustomApp],
) -> ResolvedCandidate {
    if let Some(app) = best_custom(apps, c) {
        resolved.title = app.display_name.clone();
        resolved.source = SRC_CUSTOM.to_string();
        resolved.registered = true;
        resolved.custom_id = Some(app.id.clone());
        resolved.auto_buffer = app.auto_buffer;
        resolved.clip_duration_seconds = app.clip_duration_seconds;
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os::shared::detect::{resolve, Lookups, WindowInfo};
    use std::collections::HashMap;

    fn app(strategy: &str, target: &str) -> CustomApp {
        CustomApp {
            id: "row-1".to_string(),
            display_name: "Mi Juego".to_string(),
            target_exe: target.to_string(),
            match_strategy: strategy.to_string(),
            clip_duration_seconds: Some(45),
            icon_path: None,
            is_wine_proton: false,
            game_key: None,
            capture_mode: "window".to_string(),
            source_kind: Some("x11".to_string()),
            window_match: None,
            portal_token: None,
            auto_buffer: false,
            last_seen_ms: None,
        }
    }

    fn candidate() -> CandidateProcess {
        CandidateProcess {
            pid: 7,
            ppid: 1,
            exe: "/games/Terraria".to_string(),
            comm: "Terraria".to_string(),
            cmdline: vec!["./Terraria".to_string()],
            uses_gpu: true,
            is_wine: false,
            flatpak_id: None,
            steam_app_id: None,
            window: Some(WindowInfo {
                id: 9,
                name: "Terraria 1.4".to_string(),
                class: "terraria".to_string(),
                wm_pid: 7,
            }),
        }
    }

    fn empty_lookups<'a>(
        steam: &'a HashMap<u32, String>,
        strings: &'a HashMap<String, String>,
    ) -> Lookups<'a> {
        Lookups {
            steam_names: steam,
            heroic_titles: strings,
            prism_names: strings,
        }
    }

    #[test]
    fn every_strategy_matches_its_process_shape() {
        let c = candidate();
        assert!(strategy_matches(&app("exact_exe", "terraria"), &c));
        assert!(strategy_matches(&app("exact_exe", "/games/Terraria"), &c));
        assert!(strategy_matches(&app("cmdline_contains", "./terraria"), &c));
        assert!(strategy_matches(&app("window_title", "terraria 1.4"), &c));
        assert!(!strategy_matches(&app("window_title", "otro"), &c));

        let mut steam = candidate();
        steam.steam_app_id = Some(105600);
        assert!(strategy_matches(&app("steam_appid", "105600"), &steam));
        assert!(!strategy_matches(&app("steam_appid", "570"), &steam));

        let mut prism = candidate();
        prism.cmdline = vec![
            "java".to_string(),
            "--gameDir".to_string(),
            "/instances/MiMundo".to_string(),
            "net.minecraft.client.main.Main".to_string(),
        ];
        assert!(strategy_matches(&app("prism_instance", "mimundo"), &prism));

        let mut wine = candidate();
        wine.is_wine = true;
        wine.cmdline = vec!["Z:\\games\\eldenring.exe".to_string()];
        assert!(strategy_matches(&app("wine_target", "eldenring.exe"), &wine));
        assert!(!strategy_matches(&app("wine_target", "terraria.exe"), &wine));

        // Auto-created rows never match as rules.
        assert!(!strategy_matches(&app("auto", "terraria"), &c));
    }

    #[test]
    fn precedence_prefers_exact_exe_over_title() {
        let c = candidate();
        let loose = app("window_title", "terraria");
        let strict = app("exact_exe", "terraria");
        let apps = vec![loose, strict];
        let best = best_custom(&apps, &c).unwrap();
        assert_eq!(best.match_strategy, "exact_exe");
    }

    #[test]
    fn with_custom_overrides_title_and_prefs() {
        let c = candidate();
        let steam: HashMap<u32, String> = HashMap::new();
        let strings: HashMap<String, String> = HashMap::new();
        let lookups = empty_lookups(&steam, &strings);
        let base = resolve(&c, &lookups);
        assert!(!base.registered);

        let apps = vec![app("exact_exe", "terraria")];
        let over = with_custom(base, &c, &apps);
        assert_eq!(over.title, "Mi Juego");
        assert_eq!(over.source, SRC_CUSTOM);
        assert!(over.registered);
        assert_eq!(over.custom_id.as_deref(), Some("row-1"));
        assert!(!over.auto_buffer);
        assert_eq!(over.clip_duration_seconds, Some(45));
    }
}
