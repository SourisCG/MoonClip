//! Per-game library folders (Phase: library organization).
//!
//! Every registered game gets a folder under the clips directory. The folder
//! name is a sanitized version of the game name; if a folder with that name
//! already exists (case-insensitive) it is REUSED, so re-registering a game
//! merges its clips back into the same group. Folders are never deleted, not
//! even when the game registration is removed.

use std::path::Path;

/// Folder used when no game was detected.
pub const UNKNOWN_FOLDER: &str = "Unknown";

const MAX_LEN: usize = 80;
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Turn a game name into a safe folder name (Windows-invalid characters and
/// control chars replaced, whitespace collapsed, reserved device names
/// prefixed, length capped). Never empty: falls back to `Unknown`.
pub fn sanitize_game_folder(name: &str) -> String {
    let mut cleaned: String = name
        .chars()
        .map(|ch| {
            if ch.is_whitespace() {
                ' '
            } else if matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
                || ch.is_control()
            {
                '-'
            } else {
                ch
            }
        })
        .collect();
    cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    cleaned = cleaned
        .trim_matches(|c: char| c == '.' || c == ' ')
        .to_string();
    if cleaned.chars().count() > MAX_LEN {
        cleaned = cleaned.chars().take(MAX_LEN).collect::<String>();
        cleaned = cleaned
            .trim_matches(|c: char| c == '.' || c == ' ')
            .to_string();
    }
    if cleaned.is_empty() {
        return UNKNOWN_FOLDER.to_string();
    }
    let upper = cleaned.to_ascii_uppercase();
    let base = upper.split('.').next().unwrap_or("");
    if RESERVED.contains(&base) {
        cleaned = format!("_{cleaned}");
    }
    cleaned
}

/// Existing folder matching `desired` (case-insensitive), without creating
/// anything. Used to link registrations that predate `clips_folder`.
pub fn find_existing_folder(base: &Path, desired: &str) -> Option<String> {
    let sanitized = sanitize_game_folder(desired);
    let entries = std::fs::read_dir(base).ok()?;
    for entry in entries.filter_map(|e| e.ok()) {
        if !entry.path().is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if name.to_lowercase() == sanitized.to_lowercase() {
            return Some(name);
        }
    }
    None
}

/// Existing folder for `desired` (case-insensitive) or a fresh one. Returns
/// the folder NAME as it exists on disk. Never deletes anything.
pub fn ensure_game_folder(base: &Path, desired: &str) -> Result<String, String> {
    if let Some(existing) = find_existing_folder(base, desired) {
        return Ok(existing);
    }
    let sanitized = sanitize_game_folder(desired);
    let dir = base.join(&sanitized);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("cannot create game folder {}: {e}", dir.display()))?;
    Ok(sanitized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_forbidden_characters() {
        assert_eq!(sanitize_game_folder("DOOM: Eternal"), "DOOM- Eternal");
        assert_eq!(sanitize_game_folder("a/b\\c|d?e*f\"g<h>i"), "a-b-c-d-e-f-g-h-i");
        assert_eq!(sanitize_game_folder("Line\nbreak\t tab"), "Line break tab");
        assert_eq!(sanitize_game_folder("  ..trailing..  "), "trailing");
    }

    #[test]
    fn sanitize_handles_empty_reserved_and_long_names() {
        assert_eq!(sanitize_game_folder(""), UNKNOWN_FOLDER);
        assert_eq!(sanitize_game_folder("   "), UNKNOWN_FOLDER);
        assert_eq!(sanitize_game_folder("CON"), "_CON");
        assert_eq!(sanitize_game_folder("com1.txt"), "_com1.txt");
        let long = "x".repeat(200);
        assert_eq!(sanitize_game_folder(&long).chars().count(), MAX_LEN);
    }

    #[test]
    fn sanitize_keeps_unicode() {
        assert_eq!(sanitize_game_folder("Pokémon Esmeralda"), "Pokémon Esmeralda");
        assert_eq!(sanitize_game_folder("ゲーム"), "ゲーム");
    }

    #[test]
    fn ensure_reuses_an_existing_folder_case_insensitively() {
        let base =
            std::env::temp_dir().join(format!("moonclip-folders-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(base.join("Kingdom Hearts")).unwrap();
        assert_eq!(
            ensure_game_folder(&base, "kingdom hearts").unwrap(),
            "Kingdom Hearts"
        );
        assert_eq!(ensure_game_folder(&base, "DOOM").unwrap(), "DOOM");
        assert!(base.join("DOOM").is_dir());
        // Called again it reuses the one it just made.
        assert_eq!(ensure_game_folder(&base, "doom").unwrap(), "DOOM");
        let _ = std::fs::remove_dir_all(&base);
    }
}
