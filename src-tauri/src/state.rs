//! Shared app state: the capture engine behind an async mutex.
//! The Engine type comes from crate::os (per-OS backend); this file is OS-free.

use tokio::sync::Mutex;

pub type Engine = crate::os::Engine;

/// Registered-game polling state (simple Medal-style autopilot).
#[derive(Default)]
pub struct GameRuntime {
    /// Registered app currently running (None = no game).
    pub current: Option<crate::storage::models::RegisteredInput>,
    /// Input the running buffer is recording (game or screen), for restarts.
    pub active_input: Option<String>,
    /// The running buffer was started by the poller (never auto-stop manual).
    pub auto_started: bool,
    /// Consecutive polls with no game while an auto session runs.
    pub missing_ticks: u32,
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
    /// Registered-game polling runtime.
    pub game: Mutex<GameRuntime>,
}
