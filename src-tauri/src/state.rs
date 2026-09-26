//! Shared app state: the capture engine behind an async mutex.
//! The Engine type comes from crate::os (per-OS backend); this file is OS-free.

use tokio::sync::Mutex;

pub type Engine = crate::os::Engine;

/// Detection runtime: last picked game (UI/IPC) + auto-buffer machine.
#[derive(Default)]
pub struct DetectRuntime {
    pub current: Option<crate::os::shared::detect::ResolvedCandidate>,
    pub auto: crate::os::shared::detect::AutoState,
}

#[derive(Default)]
pub struct AppState {
    pub recorder: Mutex<Option<Engine>>,
    /// Serializes the whole save pipeline (engine flush → dedupe → scale →
    /// thumb → DB): two hotkey presses must never interleave file writes or
    /// race the DB insert (Windows names files per second and muxes with -y).
    pub save_lock: Mutex<()>,
    /// Last audio-gain apply outcome (None = ok/never). Shown in UI, no silent fails.
    pub audio_error: Mutex<Option<String>>,
    /// Last engine death/exit error (None = ok/never). Shown in UI.
    pub engine_error: Mutex<Option<String>>,
    /// Game detection + Medal-style auto buffer.
    pub detect: Mutex<DetectRuntime>,
}
