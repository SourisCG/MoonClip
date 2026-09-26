//! Resolve the window picked in the KDE portal back to (app id, title).
//!
//! OBS stores only the portal `RestoreToken` in the source settings (Wayland
//! has no window-name API). xdg-desktop-portal persists, per restore token,
//! the backend's restore data — and KDE's backend records the selected
//! window's app id + title in it. The permission store D-Bus service exposes
//! that data (`Lookup`), which we parse here.

use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowIdentity {
    pub app_id: String,
    pub title: String,
}

/// Window identity recorded for a portal restore token, if available.
pub fn window_identity(restore_token: &str) -> Option<WindowIdentity> {
    let payload = lookup_payload(restore_token)?;
    parse_windows(&payload)
}

const PERMISSION_STORE_PATH: &str = "/org/freedesktop/impl/portal/PermissionStore";

/// Raw restore-data payload bytes stored for `screencast`/`token`.
fn lookup_payload(token: &str) -> Option<Vec<u8>> {
    let out = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.impl.portal.PermissionStore",
            "--object-path",
            PERMISSION_STORE_PATH,
            "--method",
            "org.freedesktop.impl.portal.PermissionStore.Lookup",
            "screencast",
            token,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_byte_array(&String::from_utf8_lossy(&out.stdout))
}

/// `gdbus` renders the `ay` as `<[byte 0x01, 0x02, ...]>`; pull the bytes out.
fn parse_byte_array(text: &str) -> Option<Vec<u8>> {
    let start = text.find("<[")? + 2;
    let end = text[start..].find("]>")? + start;
    let bytes: Vec<u8> = text[start..end]
        .split(',')
        .filter_map(|tok| {
            let tok = tok.trim().trim_start_matches("byte ").trim();
            tok.strip_prefix("0x")
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        })
        .collect();
    (!bytes.is_empty()).then_some(bytes)
}

/// KDE stores `restore_data.payload.windows` as a QDataStream-serialized
/// `QList<WindowRestoreInfo>` where each entry is two QStrings (UTF-16BE,
/// length in bytes). Locate the type-name marker and read the first entry.
fn parse_windows(bytes: &[u8]) -> Option<WindowIdentity> {
    const MARKER: &[u8] = b"QList<WindowRestoreInfo>";
    let mut search = 0usize;
    while let Some(rel) = bytes
        .get(search..)?
        .windows(MARKER.len())
        .position(|w| w == MARKER)
    {
        let mut p = search + rel + MARKER.len();
        if let Some((app_id, title)) = read_first_of_list(bytes, p) {
            if !title.trim().is_empty() {
                return Some(WindowIdentity { app_id, title });
            }
        }
        // Advance a bit so the next search does not re-find the same marker.
        p += 1;
        search = p;
    }
    None
}

fn read_first_of_list(bytes: &[u8], p: usize) -> Option<(String, String)> {
    let count_be = |q: usize| -> Option<usize> {
        Some(u32::from_be_bytes(bytes.get(q..q + 4)?.try_into().ok()?) as usize)
    };
    // Qt's QDataStream writes the user-type name with a NUL terminator here
    // (length includes it), so the list count can start one byte later.
    let (mut q, _count) = match (count_be(p), count_be(p + 1)) {
        (Some(0), Some(c)) if (1..=64).contains(&c) => (p + 1, c),
        (Some(c), _) if (1..=64).contains(&c) => (p, c),
        _ => return None,
    };
    q += 4;
    let app_id = read_qstring_be(bytes, &mut q)?;
    let title = read_qstring_be(bytes, &mut q)?;
    Some((app_id, title))
}

/// `QDataStream` QString: quint32 byte length (0xFFFFFFFF = null) + UTF-16BE.
fn read_qstring_be(bytes: &[u8], p: &mut usize) -> Option<String> {
    let len = u32::from_be_bytes(bytes.get(*p..*p + 4)?.try_into().ok()?) as usize;
    *p += 4;
    if len == u32::MAX as usize {
        return Some(String::new());
    }
    if !len.is_multiple_of(2) || len > 4096 {
        return None;
    }
    let raw = bytes.get(*p..*p + len)?;
    *p += len;
    let units: Vec<u16> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_be_bytes(*c))
        .collect();
    String::from_utf16(&units).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qstring_be(s: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let units: Vec<u16> = s.encode_utf16().collect();
        out.extend_from_slice(&((units.len() * 2) as u32).to_be_bytes());
        for u in units {
            out.extend_from_slice(&u.to_be_bytes());
        }
        out
    }

    #[test]
    fn parses_window_restore_info_payload() {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"QList<WindowRestoreInfo>");
        payload.push(0); // Qt writes the type name NUL-terminated here
        payload.extend_from_slice(&1u32.to_be_bytes());
        payload.extend_from_slice(&qstring_be("steam_app_2552430"));
        payload.extend_from_slice(&qstring_be("KINGDOM HEARTS FINAL MIX"));
        assert_eq!(
            parse_windows(&payload),
            Some(WindowIdentity {
                app_id: "steam_app_2552430".into(),
                title: "KINGDOM HEARTS FINAL MIX".into(),
            })
        );
    }

    #[test]
    fn parses_payload_without_nul_terminator() {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"QList<WindowRestoreInfo>");
        payload.extend_from_slice(&1u32.to_be_bytes());
        payload.extend_from_slice(&qstring_be("brave-browser"));
        payload.extend_from_slice(&qstring_be("WhatsApp - Brave"));
        assert_eq!(
            parse_windows(&payload).unwrap().title,
            "WhatsApp - Brave"
        );
    }

    /// Real payload captured from the permission store on Fedora/KDE 6:
    /// a Discord window picked by OBS (app id + title inside).
    #[test]
    fn parses_real_kde_payload_fixture() {
        let hex = "\
000000030000000e006f0075007400700075007400730000000900000000000000000c0072006500670069006f006e\
00000013000000000000000000ffffffffffffffff0000000e00770069006e0064006f00770073000100000000000019\
514c6973743c57696e646f77526573746f7265496e666f3e00000000010000000e0064006900730063006f00720064\
0000005e00430041004e0041004c00200045004e0020004d0041004e00540045004e0049004d00490045004e0054004f\
0020007c002000540045004e004f00430048005400490054004c0041004e0020002d00200044006900730063006f0072\
0064";
        let hex: String = hex.split_whitespace().collect();
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        assert_eq!(
            parse_windows(&bytes),
            Some(WindowIdentity {
                app_id: "discord".into(),
                title: "CANAL EN MANTENIMIENTO | TENOCHTITLAN - Discord".into(),
            })
        );
    }

    #[test]
    fn skips_empty_window_lists_and_finds_later_entry() {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"QList<WindowRestoreInfo>");
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(b"junk");
        payload.extend_from_slice(b"QList<WindowRestoreInfo>");
        payload.extend_from_slice(&1u32.to_be_bytes());
        payload.extend_from_slice(&qstring_be("brave-browser"));
        payload.extend_from_slice(&qstring_be("WhatsApp - Brave"));
        assert_eq!(
            parse_windows(&payload).unwrap().title,
            "WhatsApp - Brave"
        );
    }

    #[test]
    fn monitor_payload_has_no_windows() {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"QList<WindowRestoreInfo>");
        payload.extend_from_slice(&0u32.to_be_bytes());
        assert!(parse_windows(&payload).is_none());
    }

    #[test]
    fn extracts_bytes_from_gdbus_text() {
        let text = "({'app': ['yes']}, <('KDE', uint32 1, <[byte 0x51, 0x4c, 0x00, 0xff]>)>)";
        assert_eq!(parse_byte_array(text), Some(vec![0x51, 0x4c, 0x00, 0xff]));
        assert_eq!(parse_byte_array("no array"), None);
    }
}
