//! Pure parsers for the detection sources (Steam, Heroic, Prism, Wine, Flatpak)
//! and the process blacklist. No filesystem access here: scanners feed text.

use std::path::PathBuf;

/// Processes that may hold a GPU FD but are never "the game".
pub const BLACKLIST: &[&str] = &[
    // MoonClip's own engine (uses the GPU for NVENC/render).
    "moonclip-engine",
    "moonclip-mux",
    "moonclip-nvenc-test",
    "moonclip",
    "obs",
    "obs-ffmpeg-mux",
    // Compositors / display servers / desktop plumbing.
    "kwin_wayland",
    "kwin_x11",
    "gnome-shell",
    "mutter",
    "Xwayland",
    "Xorg",
    "plasmashell",
    "plasma-keyboard",
    "kded",
    "kded6",
    "kglobalacceld",
    "ksmserver",
    "kaccess",
    "systemsettings",
    "xdg-desktop-portal",
    "xdg-desktop-portal-kde",
    "xdg-desktop-portal-gtk",
    "xdg-desktop-portal-gnome",
    // Game launchers/overlays (the game itself runs as a different process).
    "steam",
    "gamescope",
    "mangohud",
    "gameoverlayui",
    "prismlauncher",
    "heroic",
    "lutris",
    "bottles",
    // Browsers / Electron / WebKit (hardware accelerated).
    "firefox",
    "firefox-bin",
    "chrome",
    "chromium",
    "chromium-browser",
    "google-chrome",
    "brave",
    "brave-browser",
    "vivaldi",
    "opera",
    "electron",
    "discord",
    "vesktop",
    "code",
    "code-oss",
    "codium",
    "steamwebhelper",
    "WebKitWebProcess",
    "WebKitNetworkProcess",
    "qtwebengineprocess",
    // Media / capture tools.
    "ffmpeg",
    "ffplay",
    "gpu-screen-recorder",
    "kdenlive",
    "obs-ffmpeg-mux",
    // Wine/Proton plumbing (never the game itself).
    "wineserver",
    "winedevice.exe",
    "explorer.exe",
    "services.exe",
    "conhost.exe",
    "plugplay.exe",
    "wineboot.exe",
    "steam.exe",
    "steamservice.exe",
    "steamerrorreporter.exe",
    "rundll32.exe",
    "gfexperience.exe",
];

/// True when `name_or_path` (exe basename or comm) is blacklisted.
pub fn is_blacklisted(name_or_path: &str) -> bool {
    let base = name_or_path.rsplit(['/', '\\']).next().unwrap_or(name_or_path);
    let base = base.trim().to_lowercase();
    // MoonClip's own binaries (app, engine, mux, test harness) are never the
    // game, even when the GPU is busy with NVENC.
    if base.starts_with("moonclip") {
        return true;
    }
    BLACKLIST.iter().any(|b| base == b.to_lowercase())
}

/// Wine/Proton processes: the launcher binary, a pressure-vessel container or
/// the `.exe` itself. Deliberately strict — a cmdline merely mentioning
/// "wine" must not turn every process into a game.
pub fn looks_like_wine(exe: &str, cmdline: &[String]) -> bool {
    let base = exe
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(exe)
        .to_lowercase();
    base.starts_with("wine")
        || base.contains("pressure-vessel")
        || base.ends_with(".exe")
        || cmdline
            .first()
            .map(|a| a.to_lowercase().ends_with(".exe"))
            .unwrap_or(false)
}

/// Wine helper processes are never the game, even when they run a `.exe`.
pub fn is_wine_helper(exe: &str) -> bool {
    let base = exe
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(exe)
        .to_lowercase();
    matches!(
        base.as_str(),
        "winedevice.exe"
            | "explorer.exe"
            | "services.exe"
            | "conhost.exe"
            | "plugplay.exe"
            | "wineboot.exe"
            | "steam.exe"
            | "steamservice.exe"
            | "steamerrorreporter.exe"
            | "rundll32.exe"
    )
}

/// First `.exe` (case-insensitive) in a Wine/Proton command line.
pub fn wine_exe_from_cmdline(args: &[String]) -> Option<String> {
    args.iter()
        .find(|a| a.to_lowercase().ends_with(".exe") && !a.starts_with('-'))
        .cloned()
}

/// `SteamAppId` from a `/proc/<pid>/environ` blob (NUL-separated).
pub fn parse_steam_appid_env(environ: &[u8]) -> Option<u32> {
    for var in environ.split(|&b| b == 0) {
        if let Some(rest) = var.strip_prefix(b"SteamAppId=") {
            if let Ok(s) = std::str::from_utf8(rest) {
                if let Ok(id) = s.trim().parse::<u32>() {
                    return Some(id);
                }
            }
        }
    }
    None
}

/// Flatpak app id from a cgroup line (`.../app-<id>-....scope` or
/// `.../app/<id>/...`).
pub fn parse_flatpak_id(cgroup: &str) -> Option<String> {
    for part in cgroup.split(['/', '\n']) {
        let rest = part
            .strip_prefix("app-flatpak-")
            .or_else(|| part.strip_prefix("app-"));
        if let Some(rest) = rest {
            if let Some(id) = rest.split('-').next() {
                if id.contains('.') {
                    return Some(id.to_string());
                }
            }
        }
    }
    None
}

/// Steam `appmanifest_<id>.acf` → (appid, name).
pub fn parse_acf(text: &str) -> Option<(u32, String)> {
    let mut appid = None;
    let mut name = None;
    for line in text.lines() {
        let line = line.trim();
        let mut kv = line.splitn(2, char::is_whitespace).map(str::trim);
        let (Some(k), Some(v)) = (kv.next(), kv.next()) else {
            continue;
        };
        let v = v.trim_matches('"');
        match k.trim_matches('"') {
            "appid" => appid = v.parse::<u32>().ok(),
            "name" => name = (!v.is_empty()).then(|| v.to_string()),
            _ => {}
        }
    }
    match (appid, name) {
        (Some(id), Some(name)) => Some((id, name)),
        _ => None,
    }
}

/// `libraryfolders.vdf` → library paths (order preserved, deduped).
pub fn parse_libraryfolders(text: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut current: Option<PathBuf> = None;
    for line in text.lines() {
        let line = line.trim();
        let mut kv = line.splitn(2, char::is_whitespace).map(str::trim);
        let (Some(k), Some(v)) = (kv.next(), kv.next()) else {
            continue;
        };
        if k.trim_matches('"') == "path" {
            let p = v.trim_matches('"').replace("\\\\", "\\");
            let p = PathBuf::from(p);
            if !out.contains(&p) {
                out.push(p);
            }
            current = None;
        } else if current.is_none() && k.trim_matches('"').parse::<u32>().is_ok() && v == "{" {
            current = Some(PathBuf::new());
        }
    }
    out
}

/// Prism/MultiMC `instance.cfg` → display name (`name=`).
pub fn parse_prism_instance_cfg(text: &str) -> Option<String> {
    for line in text.lines() {
        if let Some(v) = line.trim().strip_prefix("name=") {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Prism launches with `--gameDir <instances>/<Name>`; return `<Name>`.
pub fn parse_prism_game_dir(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--gameDir" {
            let dir = it.next()?;
            return dir.rsplit('/').next().map(str::to_string);
        }
        if let Some(dir) = a.strip_prefix("--gameDir=") {
            return dir.rsplit('/').next().map(str::to_string);
        }
    }
    None
}

/// Official Minecraft launcher / most modded launchers run the game through
/// the Java main class.
pub fn is_minecraft_java(args: &[String]) -> bool {
    args.iter()
        .any(|a| a.contains("net.minecraft.client.main.Main"))
}

/// Heroic `installed.json` (flat map app_name -> object) → title + exe.
pub fn parse_heroic_installed(json: &str, app_name: &str) -> Option<(String, Option<String>)> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let entry = value.get(app_name)?;
    let title = entry
        .get("title")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)?;
    let exe = entry
        .get("executable")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Some((title, exe))
}

/// Humanized name from an exe path: `eldenring.exe` → `Eldenring`.
pub fn title_from_exe(exe: &str) -> String {
    let base = exe.rsplit(['/', '\\']).next().unwrap_or(exe);
    let stem = base.rsplit_once('.').map(|(s, _)| s).unwrap_or(base);
    let stem = stem.replace(['_', '-'], " ");
    let mut out = String::with_capacity(stem.len());
    for word in stem.split_whitespace() {
        let mut chars = word.chars();
        if let Some(c) = chars.next() {
            out.extend(c.to_uppercase());
            out.push_str(chars.as_str());
            out.push(' ');
        }
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blacklist_covers_our_engine_and_browsers() {
        assert!(is_blacklisted("moonclip-engine"));
        assert!(is_blacklisted("/usr/lib/MoonClip/moonclip-mux"));
        assert!(is_blacklisted("firefox"));
        assert!(is_blacklisted("kwin_wayland"));
        assert!(is_blacklisted("discord"));
        assert!(is_blacklisted("xdg-desktop-portal-kde"));
        assert!(is_blacklisted("plasma-keyboard"));
        assert!(is_blacklisted("steam"));
        assert!(!is_blacklisted("eldenring.exe"));
        assert!(!is_blacklisted("Terraria"));
    }

    #[test]
    fn wine_detection_is_strict() {
        assert!(looks_like_wine("/usr/bin/wine-preloader", &[]));
        assert!(looks_like_wine("/usr/bin/wine64", &[]));
        assert!(looks_like_wine(
            "/usr/lib/pressure-vessel/from-host/bin/pressure-vessel-wrap",
            &[]
        ));
        assert!(looks_like_wine("Z:\\game.exe", &[]));
        assert!(looks_like_wine("/bin/sh", &["game.exe".to_string()]));
        assert!(!looks_like_wine("/bin/bash", &[
            "bash".to_string(),
            "-c".to_string(),
            "echo wine is nice".to_string()
        ]));
        assert!(!looks_like_wine("/games/Terraria", &["./Terraria".to_string()]));
    }

    #[test]
    fn wine_helpers_are_not_games() {
        assert!(is_wine_helper("/games/winedevice.exe"));
        assert!(is_wine_helper("services.exe"));
        assert!(!is_wine_helper("/games/eldenring.exe"));
        let args = vec![
            "Z:\\game.exe".to_string(),
            "/path/gamemoderun".to_string(),
            "eldenring.exe".to_string(),
            "-novid".to_string(),
        ];
        assert_eq!(
            wine_exe_from_cmdline(&args).as_deref(),
            Some("Z:\\game.exe")
        );
    }

    #[test]
    fn steam_env_and_flatpak_cgroup() {
        let environ = b"PATH=/usr/bin\0SteamAppId=570\0SteamGameId=570\0";
        assert_eq!(parse_steam_appid_env(environ), Some(570));
        assert_eq!(parse_steam_appid_env(b"PATH=/usr/bin\0"), None);
        let cgroup = "0::/user.slice/user-1000.slice/app-flatpak-com.valvesoftware.Steam-123.scope";
        assert_eq!(
            parse_flatpak_id(cgroup).as_deref(),
            Some("com.valvesoftware.Steam")
        );
        assert_eq!(parse_flatpak_id("0::/user.slice/user-1000.slice"), None);
    }

    #[test]
    fn acf_parses_id_and_name() {
        let acf = r#"
"AppState"
{
	"appid"		"105600"
	"name"		"Terraria"
	"installdir"		"Terraria"
}
"#;
        assert_eq!(parse_acf(acf), Some((105600, "Terraria".to_string())));
        assert_eq!(parse_acf("nonsense"), None);
    }

    #[test]
    fn libraryfolders_parses_paths() {
        let text = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"/home/u/.steam/steam"
	}
	"1"
	{
		"path"		"/mnt/games/Steam"
	}
}
"#;
        assert_eq!(
            parse_libraryfolders(text),
            vec![
                PathBuf::from("/home/u/.steam/steam"),
                PathBuf::from("/mnt/games/Steam")
            ]
        );
    }

    #[test]
    fn prism_and_minecraft() {
        assert_eq!(
            parse_prism_instance_cfg("InstanceType=OneSix\nname=Fabulously Optimized\n"),
            Some("Fabulously Optimized".to_string())
        );
        let args = vec![
            "java".to_string(),
            "-Xmx4G".to_string(),
            "--gameDir".to_string(),
            "/home/u/.var/app/org.prismlauncher.PrismLauncher/data/PrismLauncher/instances/MiMundo"
                .to_string(),
            "net.minecraft.client.main.Main".to_string(),
        ];
        assert_eq!(parse_prism_game_dir(&args).as_deref(), Some("MiMundo"));
        assert!(is_minecraft_java(&args));
    }

    #[test]
    fn heroic_installed_json() {
        let json = r#"{
            "Terraria": { "title": "Terraria", "executable": "Terraria.exe" },
            "Empty": { "title": "" }
        }"#;
        assert_eq!(
            parse_heroic_installed(json, "Terraria"),
            Some(("Terraria".to_string(), Some("Terraria.exe".to_string())))
        );
        assert_eq!(parse_heroic_installed(json, "Nope"), None);
    }

    #[test]
    fn titles_are_humanized() {
        assert_eq!(title_from_exe("Z:\\games\\eldenring.exe"), "Eldenring");
        assert_eq!(title_from_exe("/opt/game/My_Cool-Game"), "My Cool Game");
    }
}
