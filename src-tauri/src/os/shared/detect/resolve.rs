//! Pure game resolution: turns a scanned [`CandidateProcess`] into a titled,
//! keyed game using the manifest lookups the platform loaded. Priority
//! (custom apps are applied one layer above, in the command layer):
//! Steam -> Heroic -> Prism/Minecraft -> Wine/Proton -> fallback.

use std::collections::HashMap;

use super::parsers::{is_minecraft_java, parse_prism_game_dir, title_from_exe, wine_exe_from_cmdline};
use super::types::{CandidateProcess, SourceKind};

pub const SRC_CUSTOM: &str = "custom";
pub const SRC_STEAM: &str = "steam";
pub const SRC_HEROIC: &str = "heroic";
pub const SRC_PRISM: &str = "prism";
pub const SRC_WINE: &str = "wine";
pub const SRC_FALLBACK: &str = "fallback";

/// One resolved, displayable candidate (IPC shape for the picker/status).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResolvedCandidate {
    pub pid: u32,
    pub exe: String,
    pub comm: String,
    pub title: String,
    /// Stable identity used to persist per-game state (`steam:570`, ...).
    pub game_key: String,
    /// `custom` | `steam` | `heroic` | `prism` | `wine` | `fallback`
    pub source: String,
    pub uses_gpu: bool,
    pub is_wine: bool,
    pub steam_app_id: Option<u32>,
    /// Ready-to-use `xcomposite_input` match string when an X11 window exists.
    pub window_match: Option<String>,
    /// Intended capture route: X11 window (no portal) or portal window.
    pub source_kind: SourceKind,
    /// The user registered this app (title/prefs come from `custom_apps`).
    pub registered: bool,
    /// Row id in `custom_apps` when registered.
    pub custom_id: Option<String>,
    /// Medal-style auto buffer for this app (default on).
    pub auto_buffer: bool,
    /// User-chosen clip duration for this app, if any.
    pub clip_duration_seconds: Option<i64>,
    /// Cached icon PNG path (from the `custom_apps` row), when known.
    pub icon_path: Option<String>,
}

/// Manifest data the platform scanner loaded once per pass.
pub struct Lookups<'a> {
    /// Steam appid -> official name (from `appmanifest_*.acf`).
    pub steam_names: &'a HashMap<u32, String>,
    /// Wine/Proton exe basename (lowercase) -> title (Heroic configs).
    pub heroic_titles: &'a HashMap<String, String>,
    /// Prism instance folder -> display name (`instance.cfg`).
    pub prism_names: &'a HashMap<String, String>,
}

pub fn steam_key(app_id: u32) -> String {
    format!("steam:{app_id}")
}
pub fn wine_key(exe: &str) -> String {
    format!("wine:{}", exe.to_lowercase())
}
pub fn prism_key(instance: &str) -> String {
    format!("prism:{}", instance.to_lowercase())
}
pub fn exe_key(exe: &str) -> String {
    format!("exe:{}", exe.to_lowercase())
}

/// Resolve the best title/identity for one candidate.
pub fn resolve(c: &CandidateProcess, lookups: &Lookups) -> ResolvedCandidate {
    let base = c.exe_basename();

    if let Some(id) = c.steam_app_id {
        if let Some(name) = lookups.steam_names.get(&id) {
            return build(c, name.clone(), steam_key(id), SRC_STEAM);
        }
    }

    if let Some(title) = lookups.heroic_titles.get(&base) {
        return build(c, title.clone(), exe_key(&base), SRC_HEROIC);
    }

    if is_minecraft_java(&c.cmdline) {
        if let Some(inst) = parse_prism_game_dir(&c.cmdline) {
            let title = lookups
                .prism_names
                .get(&inst)
                .cloned()
                .unwrap_or_else(|| format!("Minecraft ({inst})"));
            return build(c, title, prism_key(&inst), SRC_PRISM);
        }
        return build(c, "Minecraft".to_string(), exe_key("minecraft"), SRC_PRISM);
    }

    if c.is_wine {
        if let Some(exe) = wine_exe_from_cmdline(&c.cmdline) {
            let title = title_from_exe(&exe);
            if !title.is_empty() {
                return build(c, title, wine_key(&exe), SRC_WINE);
            }
        }
    }

    build(c, c.fallback_title(), exe_key(&base), SRC_FALLBACK)
}

fn build(c: &CandidateProcess, title: String, game_key: String, source: &str) -> ResolvedCandidate {
    ResolvedCandidate {
        pid: c.pid,
        exe: c.exe.clone(),
        comm: c.comm.clone(),
        title,
        game_key,
        source: source.to_string(),
        uses_gpu: c.uses_gpu,
        is_wine: c.is_wine,
        steam_app_id: c.steam_app_id,
        window_match: c.window.as_ref().map(|w| w.xcomposite_match()),
        source_kind: if c.window.is_some() {
            SourceKind::Window
        } else {
            SourceKind::Portal
        },
        registered: false,
        custom_id: None,
        auto_buffer: true,
        clip_duration_seconds: None,
        icon_path: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os::shared::detect::WindowInfo;

    fn candidate() -> CandidateProcess {
        CandidateProcess {
            pid: 42,
            ppid: 1,
            exe: "/games/Terraria".to_string(),
            comm: "Terraria".to_string(),
            cmdline: vec!["./Terraria".to_string()],
            uses_gpu: true,
            is_wine: false,
            flatpak_id: None,
            steam_app_id: None,
            window: None,
        }
    }

    fn window() -> WindowInfo {
        WindowInfo {
            id: 0x3a00007,
            name: "Terraria".to_string(),
            class: "terraria".to_string(),
            wm_pid: 42,
        }
    }

    #[test]
    fn steam_wins_over_generic() {
        let mut names = HashMap::new();
        names.insert(105600, "Terraria".to_string());
        let heroic = HashMap::new();
        let prism = HashMap::new();
        let lookups = Lookups {
            steam_names: &names,
            heroic_titles: &heroic,
            prism_names: &prism,
        };
        let mut c = candidate();
        c.steam_app_id = Some(105600);
        c.window = Some(window());
        let r = resolve(&c, &lookups);
        assert_eq!(r.title, "Terraria");
        assert_eq!(r.game_key, "steam:105600");
        assert_eq!(r.source, SRC_STEAM);
        assert_eq!(r.window_match.as_deref(), Some("60817415\r\nTerraria\r\nterraria"));
    }

    #[test]
    fn wine_and_heroic_paths() {
        let steam = HashMap::new();
        let mut heroic = HashMap::new();
        heroic.insert("eldenring.exe".to_string(), "ELDEN RING".to_string());
        let prism = HashMap::new();
        let lookups = Lookups {
            steam_names: &steam,
            heroic_titles: &heroic,
            prism_names: &prism,
        };
        let mut c = candidate();
        c.exe = "Z:\\games\\eldenring.exe".to_string();
        c.cmdline = vec!["Z:\\games\\eldenring.exe".to_string()];
        c.is_wine = true;
        assert_eq!(resolve(&c, &lookups).title, "ELDEN RING");

        // Unknown wine exe -> humanized title, wine key.
        c.exe = "Z:\\games\\cool_game.exe".to_string();
        c.cmdline = vec!["Z:\\games\\cool_game.exe".to_string()];
        let r = resolve(&c, &lookups);
        assert_eq!(r.title, "Cool Game");
        assert_eq!(r.source, SRC_WINE);
    }

    #[test]
    fn minecraft_prism_instance() {
        let steam = HashMap::new();
        let heroic = HashMap::new();
        let mut prism = HashMap::new();
        prism.insert("MiMundo".to_string(), "Mi Mundo".to_string());
        let lookups = Lookups {
            steam_names: &steam,
            heroic_titles: &heroic,
            prism_names: &prism,
        };
        let mut c = candidate();
        c.exe = "/usr/bin/java".to_string();
        c.cmdline = vec![
            "java".to_string(),
            "--gameDir".to_string(),
            "/instances/MiMundo".to_string(),
            "net.minecraft.client.main.Main".to_string(),
        ];
        let r = resolve(&c, &lookups);
        assert_eq!(r.title, "Mi Mundo");
        assert_eq!(r.game_key, "prism:mimundo");
        assert_eq!(r.source, SRC_PRISM);
    }

    #[test]
    fn fallback_uses_window_name_then_exe() {
        let empty_nums: HashMap<u32, String> = HashMap::new();
        let empty_strings: HashMap<String, String> = HashMap::new();
        let lookups = Lookups {
            steam_names: &empty_nums,
            heroic_titles: &empty_strings,
            prism_names: &empty_strings,
        };
        let mut c = candidate();
        c.window = Some(window());
        assert_eq!(resolve(&c, &lookups).title, "Terraria");
        c.window = None;
        let r = resolve(&c, &lookups);
        assert_eq!(r.title, "Terraria");
        assert_eq!(r.source, SRC_FALLBACK);
        assert_eq!(r.game_key, "exe:terraria");
    }
}
