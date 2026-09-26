//! Windows game-candidate scanner: ToolHelp32 process list + foreground
//! window (no hooks, no injection), resolved against Steam (registry +
//! `appmanifest_*.acf`), Epic manifests and a Battle.net exe map.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::os::shared::detect::{
    is_blacklisted, parse_acf, parse_libraryfolders, resolve as shared_resolve, title_from_exe,
    CandidateProcess, Lookups, ResolvedCandidate, WindowInfo,
};

/// Running candidates (best effort; empty on any API failure).
pub fn scan() -> Vec<CandidateProcess> {
    let foreground = foreground_window();
    let processes = toolhelp_processes();
    let mut out = Vec::new();
    for (pid, ppid, exe_name) in processes {
        if is_blacklisted(&exe_name) {
            continue;
        }
        let exe = full_image_path(pid).unwrap_or_else(|| exe_name.clone());
        if is_blacklisted(&exe) {
            continue;
        }
        let window = if foreground.as_ref().map(|w| w.pid) == Some(pid) {
            foreground.clone()
        } else {
            None
        };
        out.push(CandidateProcess {
            pid,
            ppid,
            exe,
            comm: exe_name,
            cmdline: Vec::new(),
            uses_gpu: window.is_some(),
            is_wine: false,
            flatpak_id: None,
            steam_app_id: None,
            window,
        });
    }
    out
}

/// Resolve candidates: Steam ACF (library install dir), Epic manifests,
/// Battle.net exe names, then the shared fallback (window/exe title).
pub fn resolve_all(candidates: Vec<CandidateProcess>) -> Vec<ResolvedCandidate> {
    let steam = SteamIndex::discover();
    let epic = epic_titles();
    candidates
        .into_iter()
        .map(|c| {
            if let Some((app_id, title)) = steam.match_exe(&c.exe) {
                let mut r = shared_resolve(&c, &empty_lookups());
                r.title = title;
                r.game_key = format!("steam:{app_id}");
                r.source = "steam".to_string();
                r.steam_app_id = Some(app_id);
                r.window_match = wgc_target(&c);
                return r;
            }
            if let Some(title) = epic_match(&epic, &c.exe) {
                let mut r = shared_resolve(&c, &empty_lookups());
                r.title = title;
                r.game_key = format!("exe:{}", c.exe_basename());
                r.source = "epic".to_string();
                r.window_match = wgc_target(&c);
                return r;
            }
            if let Some(title) = battle_net_title(&c.exe_basename()) {
                let mut r = shared_resolve(&c, &empty_lookups());
                r.title = title.to_string();
                r.game_key = format!("exe:{}", c.exe_basename());
                r.source = "battlenet".to_string();
                return r;
            }
            let mut r = shared_resolve(&c, &empty_lookups());
            r.window_match = wgc_target(&c);
            r
        })
        .collect()
}

/// WGC target for a detected window (`title:class`), the format OBS's
/// `window_capture` source stores.
fn wgc_target(c: &CandidateProcess) -> Option<String> {
    c.window
        .as_ref()
        .map(|w| format!("{}:{}", w.name, w.class))
}

/// Steam/Epic artwork when available (exe icons via `SHGetFileInfo` are an
/// owner follow-up; the UI falls back to the letter avatar).
pub fn find_icon(r: &ResolvedCandidate) -> Option<PathBuf> {
    let app_id = r.steam_app_id?;
    let steam = SteamIndex::discover();
    for cache in steam.librarycache {
        for file in [
            "header.jpg",
            "library_600x900.jpg",
            "library_hero.jpg",
            "logo.png",
        ] {
            let p = cache.join(app_id.to_string()).join(file);
            if p.is_file() {
                return Some(p);
            }
        }
        let p = cache.join(format!("{app_id}_header.jpg"));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn empty_lookups() -> Lookups<'static> {
    use std::sync::OnceLock;
    static EMPTY_STEAM: OnceLock<HashMap<u32, String>> = OnceLock::new();
    static EMPTY_STR: OnceLock<HashMap<String, String>> = OnceLock::new();
    Lookups {
        steam_names: EMPTY_STEAM.get_or_init(HashMap::new),
        heroic_titles: EMPTY_STR.get_or_init(HashMap::new),
        prism_names: EMPTY_STR.get_or_init(HashMap::new),
    }
}

// ---------------------------------------------------------------------------
// Steam (registry + libraryfolders.vdf + appmanifest_*.acf)
// ---------------------------------------------------------------------------

struct SteamIndex {
    /// installdir (lowercase) -> (appid, name)
    games: HashMap<String, (u32, String)>,
    librarycache: Vec<PathBuf>,
}

impl SteamIndex {
    fn discover() -> Self {
        let mut games = HashMap::new();
        let mut librarycache = Vec::new();
        let Some(root) = steam_root() else {
            return Self {
                games,
                librarycache,
            };
        };
        librarycache.push(root.join("appcache").join("librarycache"));
        let mut libraries = vec![root.join("steamapps")];
        for vdf in [
            root.join("steamapps").join("libraryfolders.vdf"),
            root.join("config").join("libraryfolders.vdf"),
        ] {
            if let Ok(text) = std::fs::read_to_string(&vdf) {
                for lib in parse_libraryfolders(&text) {
                    let dir = lib.join("steamapps");
                    if !libraries.contains(&dir) {
                        libraries.push(dir);
                    }
                }
            }
        }
        for lib in libraries {
            let Ok(entries) = std::fs::read_dir(&lib) else {
                continue;
            };
            for e in entries.flatten() {
                let name = e.file_name();
                let Some(name) = name.to_str() else { continue };
                if !name.starts_with("appmanifest_") || !name.ends_with(".acf") {
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(e.path()) {
                    if let Some((app_id, title)) = parse_acf(&text) {
                        if let Some(dir) = acf_install_dir(&text) {
                            games.insert(dir.to_lowercase(), (app_id, title));
                        }
                    }
                }
            }
        }
        Self {
            games,
            librarycache,
        }
    }

    fn match_exe(&self, exe: &str) -> Option<(u32, String)> {
        let lower = exe.to_lowercase();
        self.games
            .iter()
            .find(|(dir, _)| {
                let marker = format!("\\steamapps\\common\\{dir}\\");
                lower.contains(&marker)
            })
            .map(|(_, v)| v.clone())
    }
}

/// `installdir` quoted value inside an ACF.
fn acf_install_dir(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        let mut kv = line.splitn(2, char::is_whitespace).map(str::trim);
        let (Some(k), Some(v)) = (kv.next(), kv.next()) else {
            continue;
        };
        if k.trim_matches('"').eq_ignore_ascii_case("installdir") {
            let v = v.trim_matches('"');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn steam_root() -> Option<PathBuf> {
    use windows::core::w;
    use windows::Win32::Foundation::WIN32_ERROR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_READ, KEY_WOW64_32KEY, REG_SZ,
    };

    unsafe {
        let mut hkey = HKEY(std::ptr::null_mut());
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Valve\\Steam"),
            None,
            KEY_READ | KEY_WOW64_32KEY,
            &mut hkey,
        ) != WIN32_ERROR(0)
        {
            return None;
        }
        let mut buf = [0u16; 512];
        let mut len = (buf.len() * 2) as u32;
        let mut kind = REG_SZ;
        let ok = RegQueryValueExW(
            hkey,
            w!("SteamPath"),
            None,
            Some(&mut kind),
            Some(buf.as_mut_ptr() as *mut u8),
            Some(&mut len),
        ) == WIN32_ERROR(0);
        let _ = RegCloseKey(hkey);
        if !ok {
            return None;
        }
        let n = (len as usize / 2).min(buf.len());
        let path = String::from_utf16_lossy(&buf[..n])
            .trim_end_matches('\0')
            .to_string();
        (!path.is_empty()).then(|| PathBuf::from(path))
    }
}

// ---------------------------------------------------------------------------
// Epic manifests
// ---------------------------------------------------------------------------

fn epic_titles() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let dir = std::env::var("PROGRAMDATA")
        .map(|p| Path::new(&p).join("Epic/EpicGamesLauncher/Data/Manifests"))
        .unwrap_or_default();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for e in entries.flatten() {
        if e.path().extension().and_then(|s| s.to_str()) != Some("item") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(e.path()) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let title = v.get("DisplayName").and_then(|x| x.as_str());
        let location = v.get("InstallLocation").and_then(|x| x.as_str());
        if let (Some(title), Some(location)) = (title, location) {
            if !title.trim().is_empty() {
                out.push((location.to_lowercase(), title.to_string()));
            }
        }
    }
    out
}

fn epic_match(epic: &[(String, String)], exe: &str) -> Option<String> {
    let lower = exe.to_lowercase();
    epic.iter()
        .find(|(loc, _)| !loc.is_empty() && lower.starts_with(loc))
        .map(|(_, title)| title.clone())
}

// ---------------------------------------------------------------------------
// Battle.net / generic
// ---------------------------------------------------------------------------

fn battle_net_title(exe_base: &str) -> Option<&'static str> {
    match exe_base.to_lowercase().as_str() {
        "overwatch.exe" => Some("Overwatch"),
        "wow.exe" | "wowclassic.exe" => Some("World of Warcraft"),
        "diablo iv.exe" => Some("Diablo IV"),
        "diablo iii64.exe" => Some("Diablo III"),
        "hearthstone.exe" => Some("Hearthstone"),
        "starcraft ii.exe" => Some("StarCraft II"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Win32 process/window helpers
// ---------------------------------------------------------------------------

fn toolhelp_processes() -> Vec<(u32, u32, String)> {
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut out = Vec::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        if snapshot == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let name = String::from_utf16_lossy(
                    entry
                        .szExeFile
                        .split(|&c| c == 0)
                        .next()
                        .unwrap_or(&[]),
                );
                if !name.is_empty() {
                    out.push((entry.th32ProcessID, entry.th32ParentProcessID, name));
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    out
}

#[derive(Clone)]
struct Foreground {
    pid: u32,
    window: WindowInfo,
}

fn foreground_window() -> Option<Foreground> {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    };

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let mut title = [0u16; 512];
        let n = GetWindowTextW(hwnd, &mut title);
        let name = String::from_utf16_lossy(&title[..n as usize]);
        let mut class = [0u16; 256];
        let c = GetClassNameW(hwnd, &mut class);
        let class = String::from_utf16_lossy(&class[..c as usize]);
        if name.trim().is_empty() {
            return None;
        }
        Some(Foreground {
            pid,
            window: WindowInfo {
                id: hwnd.0 as usize as u32,
                name,
                class,
                wm_pid: pid,
            },
        })
    }
}

fn full_image_path(pid: u32) -> Option<String> {
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        if !ok {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acf_install_dir_is_parsed() {
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\"570\"\n\t\"installdir\"\t\"dota 2 beta\"\n}\n";
        assert_eq!(acf_install_dir(acf).as_deref(), Some("dota 2 beta"));
    }

    #[test]
    fn battle_net_names_are_known() {
        assert_eq!(battle_net_title("Overwatch.exe"), Some("Overwatch"));
        assert_eq!(battle_net_title("Wow.exe"), Some("World of Warcraft"));
        assert_eq!(battle_net_title("unknown.exe"), None);
        assert_eq!(title_from_exe("Z:\\games\\cool_game.exe"), "Cool Game");
    }
}
