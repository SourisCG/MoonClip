//! Live capture levels for the mixer (OBS-style meters).
//!
//! obs-websocket's `InputVolumeMeters` high-volume event reports the levels of
//! every active input every ~50 ms. We keep the latest value per MoonClip
//! source and decay it to zero when the event stream goes quiet (a muted or
//! stopped source stops emitting).
//!
//! `levels_from_event` and `decay` are pure so they can be unit tested without
//! OBS.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use obws::events::{Event, EventStream};

use super::engine::{GAME_SOURCE_NAME, MIC_SOURCE_NAME};

/// How long a level is trusted after its last event.
pub const DECAY: Duration = Duration::from_millis(320);

#[derive(Debug, Clone, Copy, Default)]
struct Level {
    value: f32,
    peak: f32,
    at: Option<Instant>,
}

#[derive(Debug, Default)]
struct Meters {
    active: bool,
    game: Level,
    mic: Level,
}

static METERS: Mutex<Meters> = Mutex::new(Meters {
    active: false,
    game: Level {
        value: 0.0,
        peak: 0.0,
        at: None,
    },
    mic: Level {
        value: 0.0,
        peak: 0.0,
        at: None,
    },
});

/// Map one obs-websocket event to (game, mic) linear levels. Each channel of
/// `inputLevelsMul` carries `[current, peak, inputPeak]`; we take the loudest
/// channel of each.
pub fn levels_from_event(event: &Event) -> (Option<f32>, Option<f32>) {
    let Event::InputVolumeMeters { inputs } = event else {
        return (None, None);
    };
    let mut game = None;
    let mut mic = None;
    for input in inputs {
        let level = input
            .levels
            .iter()
            .map(|channel| channel.first().copied().unwrap_or(0.0))
            .fold(0.0f32, f32::max);
        let peak = input
            .levels
            .iter()
            .map(|channel| channel.get(1).copied().unwrap_or(level))
            .fold(0.0f32, f32::max);
        match input.name.as_str() {
            name if name == GAME_SOURCE_NAME => game = Some(level.max(peak)),
            name if name == MIC_SOURCE_NAME => mic = Some(level.max(peak)),
            _ => {}
        }
    }
    (game, mic)
}

/// Decay a stored level: full value immediately, zero after `DECAY`.
pub fn decay(value: f32, peak: f32, elapsed: Duration) -> (f32, f32) {
    if elapsed >= DECAY {
        return (0.0, 0.0);
    }
    let factor = 1.0 - (elapsed.as_secs_f32() / DECAY.as_secs_f32());
    (value * factor, peak * factor)
}

fn store(slot: &mut Level, level: f32, peak: f32, now: Instant) {
    slot.value = level;
    if peak > slot.peak || slot.at.is_none() {
        slot.peak = peak;
    }
    slot.at = Some(now);
}

/// Consume the event stream until OBS disconnects.
pub fn spawn(stream: EventStream) {
    if let Ok(mut meters) = METERS.lock() {
        meters.active = true;
        meters.game = Level::default();
        meters.mic = Level::default();
    }
    tokio::spawn(async move {
        let mut stream = stream;
        while let Some(event) = stream.next().await {
            let (game, mic) = levels_from_event(&event);
            if game.is_none() && mic.is_none() {
                continue;
            }
            let now = Instant::now();
            if let Ok(mut meters) = METERS.lock() {
                if let Some(level) = game {
                    store(&mut meters.game, level, level, now);
                }
                if let Some(level) = mic {
                    store(&mut meters.mic, level, level, now);
                }
            }
        }
        deactivate();
    });
}

/// Buffer stopped: no meters until the next start.
pub fn deactivate() {
    if let Ok(mut meters) = METERS.lock() {
        meters.active = false;
        meters.game = Level::default();
        meters.mic = Level::default();
    }
}

/// (game level, game peak, mic level, mic peak) with decay applied; `None`
/// when the engine is not running.
pub fn snapshot() -> Option<(f32, f32, f32, f32)> {
    let meters = METERS.lock().ok()?;
    if !meters.active {
        return None;
    }
    let now = Instant::now();
    let game = decay(
        meters.game.value,
        meters.game.peak,
        meters.game.at.map(|at| now - at).unwrap_or(DECAY),
    );
    let mic = decay(
        meters.mic.value,
        meters.mic.peak,
        meters.mic.at.map(|at| now - at).unwrap_or(DECAY),
    );
    Some((game.0, game.1, mic.0, mic.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use obws::events::InputVolumeMeter;

    fn meter_event(name: &str, channels: Vec<[f32; 3]>) -> Event {
        Event::InputVolumeMeters {
            inputs: vec![InputVolumeMeter {
                name: name.to_string(),
                levels: channels,
            }],
        }
    }

    #[test]
    fn levels_map_the_moonclip_sources() {
        let (game, mic) = levels_from_event(&meter_event(
            GAME_SOURCE_NAME,
            vec![[0.4, 0.6, 0.2], [0.5, 0.7, 0.3]],
        ));
        assert_eq!(game, Some(0.7)); // loudest channel, peak included
        assert_eq!(mic, None);

        let (game, mic) = levels_from_event(&meter_event(
            MIC_SOURCE_NAME,
            vec![[0.25, 0.25, 0.0]],
        ));
        assert_eq!(game, None);
        assert_eq!(mic, Some(0.25));
    }

    #[test]
    fn unrelated_inputs_are_ignored_and_bad_events_safe() {
        let (game, mic) = levels_from_event(&meter_event("Desktop Audio", vec![[1.0; 3]]));
        assert_eq!((game, mic), (None, None));
        let (game, mic) = levels_from_event(&Event::ExitStarted);
        assert_eq!((game, mic), (None, None));
    }

    #[test]
    fn decay_reaches_zero_after_the_window() {
        assert_eq!(decay(1.0, 0.8, Duration::ZERO), (1.0, 0.8));
        let (value, peak) = decay(1.0, 0.8, DECAY / 2);
        assert!((value - 0.5).abs() < 0.01, "{value}");
        assert!((peak - 0.4).abs() < 0.01, "{peak}");
        assert_eq!(decay(1.0, 0.8, DECAY), (0.0, 0.0));
    }

    #[test]
    fn snapshot_is_none_when_inactive() {
        deactivate();
        assert!(snapshot().is_none());
    }
}
