//! Medal-style auto buffer: a pure state machine (fully unit tested with an
//! injected clock) plus the candidate picker. The platform scanner and the
//! Tauri worker only feed it; all timing lives here.

use super::resolve::{ResolvedCandidate, SRC_FALLBACK};

/// Debounce before starting with a game whose window is up.
pub const START_DEBOUNCE_MS: i64 = 3_000;
/// Debounce before stopping once the game is gone.
pub const STOP_DEBOUNCE_MS: i64 = 7_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoAction {
    None,
    Start,
    Stop,
}

/// The game as seen by the state machine. `known` means Steam/Heroic/Prism/
/// Wine or a user registration; unknown `fallback` candidates never auto-start
/// (the user gets the one-time registration prompt instead).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoGame {
    pub key: String,
    pub title: String,
    pub known: bool,
    pub auto_buffer: bool,
}

#[derive(Debug, Clone, Default)]
pub struct AutoState {
    /// Key of the game currently tracked while the buffer is off.
    pub pending_key: Option<String>,
    pending_since_ms: Option<i64>,
    /// When the tracked game disappeared while an auto session runs.
    missing_since_ms: Option<i64>,
    /// The running session was started by the worker (never auto-stop manual).
    pub auto_started: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct AutoInput<'a> {
    pub now_ms: i64,
    pub detected: Option<&'a AutoGame>,
    pub buffer_running: bool,
}

/// Decide the next action and update the machine. Call every poll tick.
pub fn decide(state: &mut AutoState, input: AutoInput) -> AutoAction {
    match input.detected {
        Some(game) if game.known && game.auto_buffer => {
            state.missing_since_ms = None;
            if input.buffer_running {
                // Already running (manual or auto): nothing to do; keep the
                // session flag as-is so a manual session is never stopped.
                state.pending_key = None;
                state.pending_since_ms = None;
                return AutoAction::None;
            }
            let since = match (&state.pending_key, state.pending_since_ms) {
                (Some(key), Some(since)) if *key == game.key => since,
                _ => {
                    state.pending_key = Some(game.key.clone());
                    state.pending_since_ms = Some(input.now_ms);
                    input.now_ms
                }
            };
            if input.now_ms - since >= START_DEBOUNCE_MS {
                state.auto_started = true;
                state.pending_key = None;
                state.pending_since_ms = None;
                AutoAction::Start
            } else {
                AutoAction::None
            }
        }
        _ => {
            state.pending_key = None;
            state.pending_since_ms = None;
            if !input.buffer_running {
                state.missing_since_ms = None;
                state.auto_started = false;
                return AutoAction::None;
            }
            if !state.auto_started {
                return AutoAction::None; // manual session: never auto-stop
            }
            let since = *state.missing_since_ms.get_or_insert(input.now_ms);
            if input.now_ms - since >= STOP_DEBOUNCE_MS {
                state.missing_since_ms = None;
                state.auto_started = false;
                AutoAction::Stop
            } else {
                AutoAction::None
            }
        }
    }
}

/// Effective clip seconds: explicit override (Probar) > the duration the
/// user chose for this game > the global setting. Clamped like the setting.
pub fn effective_duration(override_s: Option<u32>, game_s: Option<u32>, global_s: u32) -> u32 {
    override_s.or(game_s).unwrap_or(global_s).clamp(5, 3600)
}

/// Pick the game the worker should track: registered rows win, then known
/// manifest sources, then the first GPU-owning window candidate. Fallback
/// (unknown) entries are returned too so the UI can prompt, but they never
/// reach `decide` as `known`.
pub fn pick_game(resolved: &[ResolvedCandidate]) -> Option<&ResolvedCandidate> {
    resolved
        .iter()
        .filter(|r| r.uses_gpu || r.steam_app_id.is_some() || r.is_wine || r.window_match.is_some())
        .min_by_key(|r| {
            let rank = if r.registered {
                0
            } else if r.source != SRC_FALLBACK {
                1
            } else if r.window_match.is_some() {
                2
            } else {
                3
            };
            (rank, std::cmp::Reverse(r.uses_gpu))
        })
}

/// `AutoGame` for the picked candidate.
pub fn auto_game(r: &ResolvedCandidate) -> AutoGame {
    AutoGame {
        key: r.game_key.clone(),
        title: r.title.clone(),
        known: r.registered || r.source != SRC_FALLBACK,
        auto_buffer: r.auto_buffer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(key: &str) -> AutoGame {
        AutoGame {
            key: key.to_string(),
            title: "Game".to_string(),
            known: true,
            auto_buffer: true,
        }
    }

    #[test]
    fn starts_only_after_the_debounce() {
        let mut s = AutoState::default();
        let g = game("steam:1");
        let input = |now| AutoInput {
            now_ms: now,
            detected: Some(&g),
            buffer_running: false,
        };
        assert_eq!(decide(&mut s, input(0)), AutoAction::None);
        assert_eq!(decide(&mut s, input(2_999)), AutoAction::None);
        assert_eq!(decide(&mut s, input(3_000)), AutoAction::Start);
        assert!(s.auto_started);
    }

    #[test]
    fn game_switch_restarts_the_debounce() {
        let mut s = AutoState::default();
        let a = game("steam:1");
        let b = game("steam:2");
        assert_eq!(
            decide(
                &mut s,
                AutoInput {
                    now_ms: 0,
                    detected: Some(&a),
                    buffer_running: false
                }
            ),
            AutoAction::None
        );
        assert_eq!(
            decide(
                &mut s,
                AutoInput {
                    now_ms: 2_000,
                    detected: Some(&b),
                    buffer_running: false
                }
            ),
            AutoAction::None
        );
        assert_eq!(
            decide(
                &mut s,
                AutoInput {
                    now_ms: 4_000,
                    detected: Some(&b),
                    buffer_running: false
                }
            ),
            AutoAction::None
        );
        assert_eq!(
            decide(
                &mut s,
                AutoInput {
                    now_ms: 5_000,
                    detected: Some(&b),
                    buffer_running: false
                }
            ),
            AutoAction::Start
        );
    }

    #[test]
    fn stops_only_auto_sessions_after_the_debounce() {
        let mut s = AutoState {
            auto_started: true,
            ..Default::default()
        };
        let none = AutoInput {
            now_ms: 0,
            detected: None,
            buffer_running: true,
        };
        assert_eq!(decide(&mut s, none), AutoAction::None);
        assert_eq!(
            decide(
                &mut s,
                AutoInput {
                    now_ms: 6_999,
                    ..none
                }
            ),
            AutoAction::None
        );
        assert_eq!(
            decide(
                &mut s,
                AutoInput {
                    now_ms: 7_000,
                    ..none
                }
            ),
            AutoAction::Stop
        );
        assert!(!s.auto_started);
    }

    #[test]
    fn manual_sessions_are_never_stopped_nor_double_started() {
        let mut s = AutoState::default(); // auto_started = false
        let g = game("steam:1");
        assert_eq!(
            decide(
                &mut s,
                AutoInput {
                    now_ms: 0,
                    detected: Some(&g),
                    buffer_running: true
                }
            ),
            AutoAction::None
        );
        assert_eq!(
            decide(
                &mut s,
                AutoInput {
                    now_ms: 60_000,
                    detected: None,
                    buffer_running: true
                }
            ),
            AutoAction::None
        );
    }

    #[test]
    fn duration_precedence_is_override_game_global() {
        assert_eq!(effective_duration(Some(15), Some(45), 30), 15);
        assert_eq!(effective_duration(None, Some(45), 30), 45);
        assert_eq!(effective_duration(None, None, 30), 30);
        assert_eq!(effective_duration(None, None, 1), 5);
        assert_eq!(effective_duration(None, None, 99_999), 3600);
    }

    #[test]
    fn unknown_or_disabled_games_never_start() {
        let mut s = AutoState::default();
        let unknown = AutoGame {
            key: "exe:foo".into(),
            title: "Foo".into(),
            known: false,
            auto_buffer: true,
        };
        let disabled = AutoGame {
            key: "steam:9".into(),
            title: "Off".into(),
            known: true,
            auto_buffer: false,
        };
        for g in [&unknown, &disabled] {
            for now in [0, 60_000] {
                assert_eq!(
                    decide(
                        &mut s,
                        AutoInput {
                            now_ms: now,
                            detected: Some(g),
                            buffer_running: false
                        }
                    ),
                    AutoAction::None
                );
            }
        }
    }
}
