//! Linux game-candidate scanner: read-only `/proc` walk plus an optional X11
//! window map (X11/XWayland sessions) used for pid-to-window matching and
//! window capture. Never writes, never hooks: anti-cheat safe.

use std::collections::HashMap;
use std::path::Path;

use crate::os::shared::detect::{
    is_blacklisted, is_wine_helper, parse_flatpak_id, parse_steam_appid_env, wine_exe_from_cmdline,
    CandidateProcess, WindowInfo,
};

/// Candidates running right now (best effort; never panics).
pub fn scan() -> Vec<CandidateProcess> {
    let windows = x11_windows();
    scan_proc(Path::new("/proc"), &windows)
}

/// Injectable scanner: `/proc` in production, a fixture tree in tests.
pub fn scan_proc(proc_root: &Path, windows: &HashMap<u32, WindowInfo>) -> Vec<CandidateProcess> {
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let dir = e.path();
        let comm = read_trim(&dir.join("comm")).unwrap_or_default();
        let cmdline = read_cmdline(&dir.join("cmdline"));
        if comm.is_empty() || cmdline.is_empty() {
            continue; // kernel thread / zombie
        }
        if is_blacklisted(&comm) {
            continue;
        }
        let exe = read_link_string(&dir.join("exe")).unwrap_or_else(|| cmdline[0].clone());
        if is_blacklisted(&exe) {
            continue;
        }
        let is_wine = crate::os::shared::detect::looks_like_wine(&exe, &cmdline);
        if is_wine {
            if let Some(target) = wine_exe_from_cmdline(&cmdline) {
                if is_wine_helper(&target) {
                    continue;
                }
            }
        }
        let ppid = read_ppid(&dir.join("stat")).unwrap_or(0);
        let steam_app_id = std::fs::read(dir.join("environ"))
            .ok()
            .and_then(|b| parse_steam_appid_env(&b));
        let flatpak_id = read_trim(&dir.join("cgroup")).and_then(|c| parse_flatpak_id(&c));
        out.push(CandidateProcess {
            pid,
            ppid,
            exe,
            comm,
            cmdline,
            uses_gpu: uses_gpu_device(&dir.join("fd")),
            is_wine,
            flatpak_id,
            steam_app_id,
            window: windows.get(&pid).cloned(),
        });
    }
    out
}

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn read_link_string(p: &Path) -> Option<String> {
    std::fs::read_link(p).ok().map(|pb| pb.to_string_lossy().to_string())
}

fn read_cmdline(p: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(p) else {
        return Vec::new();
    };
    bytes
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).to_string())
        .collect()
}

fn read_ppid(stat: &Path) -> Option<u32> {
    let s = std::fs::read_to_string(stat).ok()?;
    let rest = s.rsplit_once(')')?.1;
    rest.split_whitespace().nth(1)?.parse().ok()
}

fn uses_gpu_device(fd_dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(fd_dir) else {
        return false;
    };
    for e in entries.flatten() {
        if let Ok(target) = std::fs::read_link(e.path()) {
            let s = target.to_string_lossy();
            if s.starts_with("/dev/dri/renderD") || s.starts_with("/dev/nvidia") {
                return true;
            }
        }
    }
    false
}

/// Top-level X11/XWayland windows by owning PID. Empty when there is no X
/// server (pure Wayland session or headless).
pub fn x11_windows() -> HashMap<u32, WindowInfo> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

    let Ok((conn, screen_num)) = x11rb::connect(None) else {
        return HashMap::new();
    };
    let root = conn.setup().roots[screen_num].root;
    let atom = |name: &[u8]| -> Option<u32> {
        conn.intern_atom(false, name)
            .ok()?
            .reply()
            .ok()
            .map(|r| r.atom)
    };
    let (Some(client_list), Some(net_wm_pid), Some(net_wm_name), Some(utf8), Some(wm_class)) = (
        atom(b"_NET_CLIENT_LIST"),
        atom(b"_NET_WM_PID"),
        atom(b"_NET_WM_NAME"),
        atom(b"UTF8_STRING"),
        atom(b"WM_CLASS"),
    ) else {
        return HashMap::new();
    };
    let Some(list) = conn
        .get_property(false, root, client_list, AtomEnum::WINDOW, 0, u32::MAX)
        .ok()
        .and_then(|c| c.reply().ok())
    else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    for win in list.value32().into_iter().flatten() {
        let pid = conn
            .get_property(false, win, net_wm_pid, AtomEnum::CARDINAL, 0, 1)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().and_then(|mut v| v.next()))
            .unwrap_or(0);
        if pid == 0 {
            continue;
        }
        let name = conn
            .get_property(false, win, net_wm_name, utf8, 0, 1024)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| String::from_utf8_lossy(&r.value).to_string())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                conn.get_property(false, win, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)
                    .ok()
                    .and_then(|c| c.reply().ok())
                    .map(|r| String::from_utf8_lossy(&r.value).to_string())
            })
            .unwrap_or_default();
        let class = conn
            .get_property(false, win, wm_class, AtomEnum::STRING, 0, 1024)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| {
                // WM_CLASS is `res_name\0res_class\0`.
                let mut parts = r.value.split(|&b| b == 0).filter(|p| !p.is_empty());
                parts.next();
                String::from_utf8_lossy(parts.next().unwrap_or(b"")).to_string()
            })
            .unwrap_or_default();
        out.entry(pid).or_insert(WindowInfo {
            id: win,
            name,
            class,
            wm_pid: pid,
        });
    }
    out
}

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::os::shared::detect::{
    parse_acf, parse_heroic_installed, parse_libraryfolders, parse_prism_instance_cfg,
    ResolvedCandidate, Lookups,
};

/// Steam `steamapps` dirs: base installs plus every `libraryfolders.vdf`
/// entry (older `steamapps/libraryfolders.vdf` and the newer `config/` path).
pub fn steam_library_dirs() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let roots = [
        home.join(".steam/steam"),
        home.join(".local/share/Steam"),
        home.join(".steam/root"),
    ];
    let mut out: Vec<PathBuf> = Vec::new();
    for root in roots {
        let steamapps = root.join("steamapps");
        let vdfs = [
            steamapps.join("libraryfolders.vdf"),
            root.join("config/libraryfolders.vdf"),
        ];
        for vdf in vdfs {
            if let Ok(text) = std::fs::read_to_string(&vdf) {
                for lib in parse_libraryfolders(&text) {
                    let dir = lib.join("steamapps");
                    if dir.is_dir() && !out.contains(&dir) {
                        out.push(dir);
                    }
                }
            }
        }
        if steamapps.is_dir() && !out.contains(&steamapps) {
            out.push(steamapps);
        }
    }
    out
}

/// appid -> official name across every library dir.
pub fn load_steam_names(libraries: &[PathBuf]) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    for lib in libraries {
        let Ok(entries) = std::fs::read_dir(lib) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(id) = name
                .strip_prefix("appmanifest_")
                .and_then(|s| s.strip_suffix(".acf"))
            else {
                continue;
            };
            if id.parse::<u32>().is_err() {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(e.path()) {
                if let Some((appid, title)) = parse_acf(&text) {
                    out.insert(appid, title);
                }
            }
        }
    }
    out
}

/// Heroic configs (native + flatpak): wine exe basename -> title.
pub fn load_heroic_titles_in(dirs: &[PathBuf]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in entries.flatten() {
            if e.path().extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let Some(map) = value.as_object() else {
                continue;
            };
            for app in map.keys() {
                if let Some((title, Some(exe))) = parse_heroic_installed(&text, app) {
                    let base = exe
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or(&exe)
                        .to_lowercase();
                    out.insert(base, title);
                }
            }
        }
    }
    out
}

/// Prism/MultiMC instances (native + flatpak): instance folder -> name.
pub fn load_prism_names_in(dirs: &[PathBuf]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in entries.flatten() {
            if !e.path().is_dir() {
                continue;
            }
            let Some(inst) = e.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let name = std::fs::read_to_string(e.path().join("instance.cfg"))
                .ok()
                .and_then(|t| parse_prism_instance_cfg(&t))
                .unwrap_or_else(|| inst.clone());
            out.insert(inst, name);
        }
    }
    out
}

struct OwnedLookups {
    loaded: Instant,
    steam: HashMap<u32, String>,
    heroic: HashMap<String, String>,
    prism: HashMap<String, String>,
}

const LOOKUP_TTL: Duration = Duration::from_secs(60);
static LOOKUPS: OnceLock<Mutex<OwnedLookups>> = OnceLock::new();

fn load_owned() -> OwnedLookups {
    let Some(home) = dirs::home_dir() else {
        return OwnedLookups {
            loaded: Instant::now(),
            steam: HashMap::new(),
            heroic: HashMap::new(),
            prism: HashMap::new(),
        };
    };
    let heroic_dirs = [
        home.join(".config/heroic/GamesConfig"),
        home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic/GamesConfig"),
    ];
    let prism_dirs = [
        home.join(".local/share/PrismLauncher/instances"),
        home.join(".var/app/org.prismlauncher.PrismLauncher/data/PrismLauncher/instances"),
    ];
    OwnedLookups {
        loaded: Instant::now(),
        steam: load_steam_names(&steam_library_dirs()),
        heroic: load_heroic_titles_in(&heroic_dirs),
        prism: load_prism_names_in(&prism_dirs),
    }
}

/// Resolve every candidate with cached manifest lookups (60 s TTL).
pub fn resolve_all(candidates: Vec<CandidateProcess>) -> Vec<ResolvedCandidate> {
    let cell = LOOKUPS.get_or_init(|| {
        Mutex::new(OwnedLookups {
            loaded: Instant::now() - LOOKUP_TTL,
            steam: HashMap::new(),
            heroic: HashMap::new(),
            prism: HashMap::new(),
        })
    });
    let mut guard = cell.lock().unwrap_or_else(|e| e.into_inner());
    if guard.loaded.elapsed() >= LOOKUP_TTL {
        *guard = load_owned();
    }
    let lookups = Lookups {
        steam_names: &guard.steam,
        heroic_titles: &guard.heroic,
        prism_names: &guard.prism,
    };
    candidates
        .iter()
        .map(|c| crate::os::shared::detect::resolve(c, &lookups))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn fixture_scan_classifies_candidates() {
        let root = std::env::temp_dir().join(format!("moonclip-detect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        // 1111: plain GPU app with Steam env + X11 window.
        write(&root.join("1111/comm"), b"Terraria");
        write(&root.join("1111/cmdline"), b"./Terraria\0");
        write(&root.join("1111/stat"), b"1111 (Terraria) S 1 1111 1111 0 -1");
        write(&root.join("1111/environ"), b"PATH=/usr/bin\0SteamAppId=105600\0");
        write(&root.join("1111/cgroup"), b"0::/user.slice/user-1000.slice/session-2.scope");
        symlink("/games/Terraria", root.join("1111/exe")).unwrap();
        std::fs::create_dir_all(root.join("1111/fd")).unwrap();
        symlink("/dev/dri/renderD128", root.join("1111/fd/7")).unwrap();

        // 2222: compositor (blacklisted).
        write(&root.join("2222/comm"), b"kwin_wayland");
        write(&root.join("2222/cmdline"), b"/usr/bin/kwin_wayland\0--no-lockscreen\0");

        // 3333: kernel thread (empty cmdline).
        write(&root.join("3333/comm"), b"kworker/0:1");
        write(&root.join("3333/cmdline"), b"");

        // 4444: Wine process running a real .exe.
        write(&root.join("4444/comm"), b"wine-preloader");
        write(
            &root.join("4444/cmdline"),
            b"Z:\\games\\eldenring.exe\0steam.exe\0-novid\0",
        );
        write(&root.join("4444/stat"), b"4444 (wine-preloader) S 1 4444 4444 0 -1");
        symlink("/usr/bin/wine-preloader", root.join("4444/exe")).unwrap();
        std::fs::create_dir_all(root.join("4444/fd")).unwrap();
        symlink("/dev/nvidia0", root.join("4444/fd/9")).unwrap();

        // 5555: Wine plumbing (must be skipped).
        write(&root.join("5555/comm"), b"wineserver");
        write(&root.join("5555/cmdline"), b"wineserver\0");

        let mut windows = HashMap::new();
        windows.insert(
            1111,
            WindowInfo {
                id: 0x3a00007,
                name: "Terraria".to_string(),
                class: "terraria".to_string(),
                wm_pid: 1111,
            },
        );

        let found = scan_proc(&root, &windows);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(found.len(), 2, "only the two games must survive: {found:?}");
        let game = found.iter().find(|c| c.pid == 1111).unwrap();
        assert!(game.uses_gpu);
        assert!(!game.is_wine);
        assert_eq!(game.steam_app_id, Some(105600));
        assert_eq!(game.window.as_ref().unwrap().class, "terraria");
        assert_eq!(game.exe_basename(), "terraria");
        assert_eq!(game.fallback_title(), "Terraria");

        let wine = found.iter().find(|c| c.pid == 4444).unwrap();
        assert!(wine.is_wine);
        assert!(wine.uses_gpu);
        assert_eq!(
            wine_exe_from_cmdline(&wine.cmdline).as_deref(),
            Some("Z:\\games\\eldenring.exe")
        );
    }

    #[test]
    #[ignore = "needs a live X11/XWayland session (developer/spike check)"]
    fn live_x11_windows_are_found() {
        let wins = x11_windows();
        eprintln!("x11 windows by pid: {}", wins.len());
        for (pid, w) in wins.iter().take(8) {
            eprintln!(
                "  pid={pid} id={} class={:?} name={:?}",
                w.id, w.class, w.name
            );
        }
        assert!(!wins.is_empty(), "no X11 windows found (no X server?)");
    }

    #[test]
    #[ignore = "developer/spike check against the live system"]
    fn live_candidates_are_resolved() {
        let cands = scan();
        let resolved = resolve_all(cands);
        let games: Vec<_> = resolved
            .iter()
            .filter(|r| r.uses_gpu || r.steam_app_id.is_some() || r.is_wine || r.window_match.is_some())
            .collect();
        eprintln!("resolved candidates: {}", games.len());
        for r in games {
            eprintln!(
                "  pid={} title={:?} key={} source={} gpu={} wine={} x11={}",
                r.pid,
                r.title,
                r.game_key,
                r.source,
                r.uses_gpu,
                r.is_wine,
                r.window_match.is_some()
            );
        }
    }

    #[test]
    fn scan_of_real_proc_never_panics() {
        let found = scan();
        // Our own test process must never be a candidate (not blacklisted,
        // but it has no GPU FD / window: still fine to be present).
        assert!(found.iter().all(|c| c.pid != std::process::id()));
        assert!(found.iter().all(|c| !is_blacklisted(&c.comm)));
    }

    #[test]
    fn manifest_lookups_from_fixtures() {
        let root = std::env::temp_dir().join(format!("moonclip-detect-idx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        write(
            &root.join("steamapps/appmanifest_105600.acf"),
            br#""AppState"
{
	"appid"		"105600"
	"name"		"Terraria"
}"#,
        );
        let names = load_steam_names(&[root.join("steamapps")]);
        assert_eq!(names.get(&105600).map(String::as_str), Some("Terraria"));

        write(
            &root.join("heroic/Terraria.json"),
            br#"{ "Terraria": { "title": "Terraria", "executable": "Terraria.exe" } }"#,
        );
        let heroic = load_heroic_titles_in(&[root.join("heroic")]);
        assert_eq!(heroic.get("terraria.exe").map(String::as_str), Some("Terraria"));

        write(&root.join("prism/MiMundo/instance.cfg"), b"name=Mi Mundo\n");
        let prism = load_prism_names_in(&[root.join("prism")]);
        assert_eq!(prism.get("MiMundo").map(String::as_str), Some("Mi Mundo"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
