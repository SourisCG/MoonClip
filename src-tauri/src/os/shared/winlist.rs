//! Desktop window list + registered-game matching (OS-independent logic).
//!
//! The autopilot ("checker") starts the buffer when a window with the same
//! name as a registered game appears and stops it when the window is gone.
//! Platforms only provide the raw window list; the matching rules live here.

use crate::storage::models::RegisteredInput;

/// A visible top-level window as the desktop reports it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DesktopWindow {
    pub title: String,
    /// App id/class when the platform exposes it (KDE icon name, else empty).
    #[serde(default)]
    pub app_id: String,
}

/// Window identity recorded by a platform picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowIdentity {
    /// Window title (also used as the registry display name).
    pub title: String,
    /// App id/class when known ("" otherwise).
    pub app_id: String,
    /// Executable name when the platform reports one (Windows target).
    pub exe: String,
}

/// Parse the Windows window-capture target. OBS stores the picked window as
/// `"<title>:<class>:<exe>"`; titles may contain colons, so split from the
/// right. A malformed value is treated as a plain title. Only the Windows
/// backend calls this at runtime; tests cover it on every OS.
#[allow(dead_code)]
pub fn parse_windows_target(target: &str) -> WindowIdentity {
    let parts: Vec<&str> = target.rsplitn(3, ':').collect();
    match parts.as_slice() {
        [exe, class, title] => WindowIdentity {
            title: title.trim().to_string(),
            app_id: class.trim().to_string(),
            exe: exe.trim().to_string(),
        },
        _ => WindowIdentity {
            title: target.trim().to_string(),
            app_id: String::new(),
            exe: String::new(),
        },
    }
}

/// Windows/app ids that are never games: file managers whose title is just
/// the folder name (Dolphin browsing `~/Videos/MoonClip/Overwatch` would
/// otherwise auto-start Overwatch's buffer).
const NON_GAME_APP_IDS: &[&str] = &[
    "dolphin",
    "org.kde.dolphin",
    "system-file-manager",
    "nautilus",
    "org.gnome.nautilus",
    "thunar",
    "nemo",
    "pcmanfm",
    "krusader",
    "org.kde.krusader",
    "doublecmd",
    "files",
    // Windows window classes of Explorer windows.
    "cabinetwclass",
    "explorewclass",
];

/// Is this window a known file manager (never a game)?
pub fn is_non_game_window(app_id: &str) -> bool {
    let id = app_id.trim().to_ascii_lowercase();
    !id.is_empty() && NON_GAME_APP_IDS.contains(&id.as_str())
}

/// Do the registered and the desktop app ids describe the same app?
///
/// The two sources disagree in format for the same window (the picker reads
/// KDE's restore data: `steam_app_2357570`; KRunner reports the icon name:
/// `steam_icon_2357570` or `steam`), so this is deliberately tolerant: exact
/// match, either side containing the other, or the same Steam app id. When
/// either side is unknown the title decides (historic behavior).
fn app_id_matches(registered: &str, candidate: &str) -> bool {
    fn norm(s: &str) -> String {
        s.trim()
            .to_ascii_lowercase()
            .replace("steam_icon_", "steam_")
            .replace("steam_app_", "steam_")
    }
    let a = norm(registered);
    let b = norm(candidate);
    if a.is_empty() || b.is_empty() {
        return true;
    }
    a == b || a.contains(&b) || b.contains(&a)
}

/// Case/space-insensitive title normalization. Titles often change state
/// ("Game - ", "Game | 1.2", "Game:"), so trailing separators are dropped;
/// matching then uses prefix-at-word-boundary.
fn norm(s: &str) -> String {
    let mut t = s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    loop {
        let trimmed = t.trim_end();
        match trimmed.chars().last() {
            Some(c) if matches!(c, '-' | '|' | ':' | '–' | '—') => {
                t = trimmed[..trimmed.len() - c.len_utf8()].trim_end().to_string();
            }
            _ => break,
        }
    }
    t
}

/// Does this desktop window match the registered game? Matching is by the
/// window title learned at registration; games often append state to their
/// title ("Game - 1.2.3") so a word-boundary prefix also counts.
///
/// Two guards keep other apps from faking a game by sharing its title:
/// file managers (Dolphin/Explorer browsing the game's folder) are excluded
/// outright, and when both sides report an app id they must be compatible.
pub fn window_matches(row: &RegisteredInput, w: &DesktopWindow) -> bool {
    if is_non_game_window(&w.app_id) {
        return false;
    }
    if let Some(registered_id) = row.window_app_id.as_deref() {
        if !app_id_matches(registered_id, &w.app_id) {
            return false;
        }
    }
    let Some(registered) = row
        .window_title
        .as_deref()
        .map(norm)
        .filter(|t| !t.is_empty())
    else {
        return false;
    };
    let title = norm(&w.title);
    if title.is_empty() {
        return false;
    }
    if title == registered {
        return true;
    }
    match title.strip_prefix(&registered) {
        Some("") => true,
        Some(rest) => rest.starts_with([' ', '-', ':', '–', '—', '(', '[']),
        None => false,
    }
}

/// First registered game with a matching window (registration order).
pub fn first_match_registered<'a>(
    rows: &'a [RegisteredInput],
    windows: &[DesktopWindow],
) -> Option<&'a RegisteredInput> {
    rows.iter()
        .find(|r| windows.iter().any(|w| window_matches(r, w)))
}

/// Manual start pick: the matched game, else the last recorded one, else the
/// only registered game.
pub fn pick_manual<'a>(
    rows: &'a [RegisteredInput],
    windows: &[DesktopWindow],
    last_input: Option<&str>,
) -> Option<&'a RegisteredInput> {
    first_match_registered(rows, windows)
        .or_else(|| {
            last_input.and_then(|n| rows.iter().find(|r| r.input_name == n))
        })
        .or_else(|| (rows.len() == 1).then(|| &rows[0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(title: &str) -> RegisteredInput {
        RegisteredInput {
            id: title.into(),
            input_name: title.into(),
            input_kind: "window".into(),
            display_name: title.into(),
            window_title: Some(title.into()),
            window_app_id: None,
            target_exe: String::new(),
            input_settings: None,
            source_uuid: "u".into(),
            icon_path: None,
            clips_folder: String::new(),
        }
    }

    fn win(title: &str) -> DesktopWindow {
        DesktopWindow {
            title: title.into(),
            app_id: String::new(),
        }
    }

    #[test]
    fn file_manager_windows_never_match_even_with_the_same_title() {
        // Dolphin browsing ~/Videos/MoonClip/Overwatch is titled "Overwatch".
        let r = row("Overwatch");
        for app_id in [
            "dolphin",
            "org.kde.dolphin",
            "system-file-manager",
            "CabinetWClass",
        ] {
            assert!(
                !window_matches(
                    &r,
                    &DesktopWindow {
                        title: "Overwatch".into(),
                        app_id: app_id.into(),
                    }
                ),
                "file manager {app_id} must not match"
            );
        }
        // The real game window (unknown app id) still matches.
        assert!(window_matches(
            &r,
            &DesktopWindow {
                title: "Overwatch".into(),
                app_id: "steam_app_2357570".into(),
            }
        ));
    }

    #[test]
    fn mismatched_app_ids_reject_a_title_collision() {
        let mut r = row("Overwatch");
        r.window_app_id = Some("steam_app_2357570".into());
        // A video player titled like the game is rejected.
        assert!(!window_matches(
            &r,
            &DesktopWindow {
                title: "Overwatch".into(),
                app_id: "mpv".into(),
            }
        ));
        // KRunner reports icon names; the tolerant comparison accepts formats.
        assert!(window_matches(
            &r,
            &DesktopWindow {
                title: "Overwatch".into(),
                app_id: "steam_icon_2357570".into(),
            }
        ));
        assert!(window_matches(
            &r,
            &DesktopWindow {
                title: "Overwatch".into(),
                app_id: "steam".into(),
            }
        ));
        // Unknown ids keep the historic title-only behavior.
        assert!(window_matches(
            &r,
            &DesktopWindow {
                title: "Overwatch".into(),
                app_id: String::new(),
            }
        ));
        let mut unknown = row("Overwatch");
        unknown.window_app_id = None;
        assert!(window_matches(
            &unknown,
            &DesktopWindow {
                title: "Overwatch".into(),
                app_id: "whatever".into(),
            }
        ));
    }

    #[test]
    fn app_id_matching_is_tolerant_but_not_blind() {
        assert!(app_id_matches("", "dolphin"));
        assert!(app_id_matches("discord", "discord"));
        assert!(app_id_matches("com.mojang.minecraft", "com.mojang.minecraft"));
        assert!(app_id_matches("brave-browser", "brave"));
        assert!(app_id_matches("steam_app_2357570", "steam_icon_2357570"));
        assert!(!app_id_matches("steam_app_2357570", "dolphin"));
        assert!(!app_id_matches("discord", "mpv"));
    }

    #[test]
    fn matches_exact_ignoring_case_and_spacing() {
        assert!(window_matches(
            &row("KINGDOM HEARTS FINAL MIX"),
            &win("Kingdom  Hearts   Final Mix")
        ));
        assert!(!window_matches(
            &row("KINGDOM HEARTS FINAL MIX"),
            &win("KINGDOM HEARTS")
        ));
    }

    #[test]
    fn matches_state_suffix_at_word_boundary() {
        assert!(window_matches(
            &row("Terraria"),
            &win("Terraria - 1.4.4.9")
        ));
        assert!(window_matches(&row("Steam"), &win("Steam")));
        assert!(window_matches(&row("Steam"), &win("Steam (Beta)")));
        assert!(!window_matches(&row("Steam"), &win("SteamDeck")));
    }

    #[test]
    fn ignores_trailing_separators_and_state() {
        // The picker recorded "KINGDOM HEARTS - HD 1.5+2.5 ReMIX -" (loading
        // state) while the loaded window reports the plain title.
        assert!(window_matches(
            &row("KINGDOM HEARTS - HD 1.5+2.5 ReMIX -"),
            &win("KINGDOM HEARTS - HD 1.5+2.5 ReMIX")
        ));
        assert!(window_matches(
            &row("Five Nights at Freddy's:"),
            &win("Five Nights at Freddy's")
        ));
        assert!(window_matches(
            &row("Game |"),
            &win("Game | Level 2")
        ));
    }

    #[test]
    fn rows_without_title_never_match() {
        let mut r = row("");
        r.window_title = None;
        assert!(!window_matches(&r, &win("Anything")));
    }

    #[test]
    fn first_match_follows_registration_order() {
        let rows = vec![row("Brave"), row("Terraria")];
        let windows = vec![win("Terraria - 1.4"), win("Brave")];
        assert_eq!(
            first_match_registered(&rows, &windows).unwrap().display_name,
            "Brave"
        );
    }

    #[test]
    fn parses_windows_window_target() {
        let id = parse_windows_target("KINGDOM HEARTS FINAL MIX:UnrealWindow:KINGDOM HEARTS FINAL MIX.exe");
        assert_eq!(id.title, "KINGDOM HEARTS FINAL MIX");
        assert_eq!(id.app_id, "UnrealWindow");
        assert_eq!(id.exe, "KINGDOM HEARTS FINAL MIX.exe");
        // Titles may contain colons: split from the right keeps them intact.
        let id = parse_windows_target("A: B - Game:Class:game.exe");
        assert_eq!(id.title, "A: B - Game");
        assert_eq!(id.exe, "game.exe");
        // Malformed values degrade to a plain title.
        let id = parse_windows_target("Just a title");
        assert_eq!(id.title, "Just a title");
        assert!(id.exe.is_empty());
    }

    #[test]
    fn manual_pick_falls_back_to_last_then_only() {
        let rows = vec![row("Brave"), row("Terraria")];
        let none: Vec<DesktopWindow> = vec![];
        assert_eq!(
            pick_manual(&rows, &none, Some("Terraria"))
                .unwrap()
                .display_name,
            "Terraria"
        );
        // With several games and no matching window, Start must not guess:
        // recording an arbitrary (closed) window would save a black clip.
        assert!(pick_manual(&rows, &none, Some("Gone")).is_none());
        assert!(pick_manual(&rows, &none, None).is_none());
        let single = vec![row("Brave")];
        assert_eq!(
            pick_manual(&single, &none, None).unwrap().display_name,
            "Brave"
        );
    }
}
