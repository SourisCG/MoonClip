//! Shared app state: the capture engine behind an async mutex.
//! The Engine type comes from crate::os (per-OS backend); this file is OS-free.

use tokio::sync::Mutex;

pub type Engine = crate::os::Engine;

#[derive(Default)]
pub struct AppState {
    pub recorder: Mutex<Option<Engine>>,
    /// Last audio-gain apply outcome (None = ok/never). Shown in UI, no silent fails.
    pub audio_error: Mutex<Option<String>>,
    /// Last engine death/exit error (None = ok/never). Shown in UI.
    pub engine_error: Mutex<Option<String>>,
}
