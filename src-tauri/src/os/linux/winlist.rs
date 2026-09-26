//! KDE window list through KWin's KRunner "Windows" runner.
//!
//! On Wayland a normal client cannot enumerate windows, but KWin exposes its
//! own runner over D-Bus: `org.kde.KWin /WindowsRunner` with
//! `org.kde.krunner1.Match ""` returns every window with its title (and for
//! some apps a useful icon/app id). Titles are all the autopilot checker
//! needs; there is no window-name API on Wayland otherwise.

use crate::os::shared::winlist::DesktopWindow;
use std::collections::HashSet;
use std::process::Command;

/// KDE Plasma session? (the only desktop with the KWin runner).
pub fn is_kde() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|v| v.to_ascii_uppercase().contains("KDE"))
        .unwrap_or(false)
        || std::env::var("KDE_FULL_SESSION")
            .map(|v| v == "true")
            .unwrap_or(false)
}

pub fn list_windows() -> Vec<DesktopWindow> {
    if !is_kde() {
        return Vec::new();
    }
    let out = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.kde.KWin",
            "--object-path",
            "/WindowsRunner",
            "--method",
            "org.kde.krunner1.Match",
            "",
        ])
        .output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    parse_krunner_matches(&String::from_utf8_lossy(&out.stdout))
}

/// Parse the GVariant text reply of `Match`:
/// `([('0_{uuid}', 'Title', 'icon', 100, 0.8, {...}), ...],)`.
/// KWin may report the same window more than once; dedupe by match id.
fn parse_krunner_matches(text: &str) -> Vec<DesktopWindow> {
    let mut out: Vec<DesktopWindow> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut i = 0usize;
    while let Some(pos) = text[i..].find("('0_{") {
        let start = i + pos + 1;
        let Some((id, j)) = read_gvariant_string(text, start) else {
            break;
        };
        let Some((title, k)) = read_gvariant_string(text, skip_sep(text, j)) else {
            break;
        };
        let (app_id, end) = read_gvariant_string(text, skip_sep(text, k)).unwrap_or_default();
        if seen.insert(id.clone()) {
            out.push(DesktopWindow { title, app_id });
        }
        i = end.max(k);
    }
    out
}

/// Skip `,` and spaces between tuple fields.
fn skip_sep(s: &str, mut i: usize) -> usize {
    let b = s.as_bytes();
    while i < b.len() && (b[i] == b',' || b[i] == b' ' || b[i] == b'\n') {
        i += 1;
    }
    i
}

/// Read one single-quoted GVariant string starting at `i` (which must point
/// at the opening quote). Returns the unescaped value and the index after the
/// closing quote. Defaults to an empty string when malformed.
fn read_gvariant_string(s: &str, i: usize) -> Option<(String, usize)> {
    let b = s.as_bytes();
    if i >= b.len() || b[i] != b'\'' {
        return None;
    }
    let mut out = String::new();
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => {
                j += 1;
                if j >= b.len() {
                    return None;
                }
                match b[j] {
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'r' => out.push('\r'),
                    other => out.push(other as char),
                }
                j += 1;
            }
            b'\'' => return Some((out, j + 1)),
            _ => {
                // Copy the full UTF-8 sequence.
                let start = j;
                j += 1;
                while j < b.len() && (b[j] & 0xC0) == 0x80 {
                    j += 1;
                }
                out.push_str(std::str::from_utf8(&b[start..j]).ok()?);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"([('0_{9973af4b-5172-4e19-b4c7-2f92429cc012}', 'SourisCG (Sebastian) - Brave', '', 100, 0.8, {'icon-data': <(64, 64, 256, true, 8, 4, [byte 0x00, 0x00])>}), ('0_{28f9cd98-8576-4892-9d65-0b0d0a6d0f77}', 'Steam', 'steam', 100, 0.7, {}), ('0_{28f9cd98-8576-4892-9d65-0b0d0a6d0f77}', 'Steam', 'steam', 100, 0.7, {})],)"#;

    #[test]
    fn parses_krunner_reply_and_dedupes() {
        let wins = parse_krunner_matches(SAMPLE);
        assert_eq!(wins.len(), 2);
        assert_eq!(wins[0].title, "SourisCG (Sebastian) - Brave");
        assert_eq!(wins[1].title, "Steam");
        assert_eq!(wins[1].app_id, "steam");
    }

    #[test]
    fn parses_escaped_titles() {
        let text = r"[('0_{abc}', 'It\'s \\ a game', '', 100, 1.0, {})]";
        let wins = parse_krunner_matches(text);
        assert_eq!(wins[0].title, "It's \\ a game");
    }

    #[test]
    fn garbage_yields_empty() {
        assert!(parse_krunner_matches("").is_empty());
        assert!(parse_krunner_matches("Error: something failed").is_empty());
    }

    /// Manual check against a real `gdbus ... Match ""` capture:
    /// `MOONCLIP_WINLIST_FIXTURE=/path/to/reply cargo test live_krunner`.
    #[test]
    fn parses_live_krunner_fixture_when_available() {
        let Ok(path) = std::env::var("MOONCLIP_WINLIST_FIXTURE") else {
            return;
        };
        let text = std::fs::read_to_string(&path).unwrap();
        let wins = parse_krunner_matches(&text);
        assert!(!wins.is_empty(), "no windows parsed from {path}");
        assert!(wins.iter().any(|w| !w.title.is_empty()));
    }
}
