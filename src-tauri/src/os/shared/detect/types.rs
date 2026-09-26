//! Detection data shared by every platform scanner.

/// How a detected game's window will be captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum SourceKind {
    /// X11/XWayland window captured with `xcomposite_input` (no portal).
    X11,
    /// Wayland-native window through the portal (per-game restore token).
    Portal,
}

/// X11 window identity (only present for X11/XWayland games).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WindowInfo {
    /// X window id (`xcb_window_t`).
    pub id: u32,
    /// `_NET_WM_NAME` / `WM_NAME`.
    pub name: String,
    /// `WM_CLASS` res_class (second string).
    pub class: String,
    /// `_NET_WM_PID` of the owning process.
    pub wm_pid: u32,
}

impl WindowInfo {
    /// Encoded match string `xcomposite_input` understands:
    /// `<id>\r\n<name>\r\n<class>`. The plugin tries the id first and then
    /// falls back to an exact name+class match (survives id changes).
    pub fn xcomposite_match(&self) -> String {
        format!("{}\r\n{}\r\n{}", self.id, self.name, self.class)
    }
}

/// One running process the detector considers a game candidate.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CandidateProcess {
    pub pid: u32,
    pub ppid: u32,
    /// Resolved `/proc/<pid>/exe` (falls back to `cmdline[0]`).
    pub exe: String,
    pub comm: String,
    pub cmdline: Vec<String>,
    /// Holds a GPU device FD (`/dev/dri/renderD*`, `/dev/nvidia*`).
    pub uses_gpu: bool,
    /// Wine/Proton process (preloader or `.exe` in the cmdline).
    pub is_wine: bool,
    /// Flatpak app id parsed from the cgroup (`app-<id>-...`).
    pub flatpak_id: Option<String>,
    /// `SteamAppId` from the process environment.
    pub steam_app_id: Option<u32>,
    /// Matching X11/XWayland window, when the compositor exposes it.
    pub window: Option<WindowInfo>,
}

impl CandidateProcess {
    /// Lowercased file name of `exe` (`eldenring.exe`, `java`, ...).
    pub fn exe_basename(&self) -> String {
        self.exe
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&self.exe)
            .to_lowercase()
    }

    /// Prefer the X11 window name, then the exe stem, humanized.
    pub fn fallback_title(&self) -> String {
        if let Some(w) = &self.window {
            if !w.name.trim().is_empty() {
                return w.name.clone();
            }
        }
        let stem = self.exe_basename();
        let stem = stem.rsplit_once('.').map(|(s, _)| s).unwrap_or(&stem);
        let mut chars = stem.chars();
        match chars.next() {
            Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            None => "Unknown".to_string(),
        }
    }
}
