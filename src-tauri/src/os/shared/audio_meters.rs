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

/// Peak markers hold for a moment before sliding down (OBS behavior).
pub const PEAK_HOLD: Duration = Duration::from_millis(700);

#[derive(Debug, Clone, Copy, Default)]
struct Level {
    value: f32,
    peak: f32,
    at: Option<Instant>,
    peak_at: Option<Instant>,
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
        peak_at: None,
    },
    mic: Level {
        value: 0.0,
        peak: 0.0,
        at: None,
        peak_at: None,
    },
});

/// Map one obs-websocket event to (game, mic) `(level, peak)` pairs. Each
/// channel of `inputLevelsMul` carries `[current, peak, inputPeak]`; we take
/// the loudest channel of each. Level and peak stay SEPARATE: painting the
/// peak as the level made silent mics read 0 dB.
pub type MeterPair = Option<(f32, f32)>;

pub fn levels_from_event(event: &Event) -> (MeterPair, MeterPair) {
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
            name if name == GAME_SOURCE_NAME => game = Some((level, peak)),
            name if name == MIC_SOURCE_NAME => mic = Some((level, peak)),
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

/// Peak marker: holds at its value, then slides down over `DECAY`.
pub fn decay_peak(peak: f32, elapsed: Duration) -> f32 {
    if elapsed <= PEAK_HOLD {
        return peak;
    }
    let over = elapsed - PEAK_HOLD;
    if over >= DECAY {
        return 0.0;
    }
    peak * (1.0 - (over.as_secs_f32() / DECAY.as_secs_f32()))
}

fn store(slot: &mut Level, level: f32, peak: f32, now: Instant) {
    let elapsed = slot
        .peak_at
        .map(|at| now.saturating_duration_since(at))
        .unwrap_or(DECAY);
    let decayed = decay_peak(slot.peak, elapsed);
    let next = decayed.max(peak);
    // A fresh peak restarts the hold; otherwise keep the old timestamp so the
    // marker keeps sliding down.
    if next > decayed || slot.peak_at.is_none() {
        slot.peak_at = Some(now);
    }
    slot.peak = next;
    slot.value = level;
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
                if let Some((level, peak)) = game {
                    store(&mut meters.game, level, peak, now);
                }
                if let Some((level, peak)) = mic {
                    store(&mut meters.mic, level, peak, now);
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
    let game_level = decay(
        meters.game.value,
        meters.game.peak,
        meters.game.at.map(|at| now - at).unwrap_or(DECAY),
    )
    .0;
    let game_peak = decay_peak(
        meters.game.peak,
        meters.game.peak_at.map(|at| now - at).unwrap_or(DECAY),
    );
    let mic_level = decay(
        meters.mic.value,
        meters.mic.peak,
        meters.mic.at.map(|at| now - at).unwrap_or(DECAY),
    )
    .0;
    let mic_peak = decay_peak(
        meters.mic.peak,
        meters.mic.peak_at.map(|at| now - at).unwrap_or(DECAY),
    );
    Some((game_level, game_peak, mic_level, mic_peak))
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
    fn levels_map_the_moonclip_sources_with_separate_peaks() {
        let (game, mic) = levels_from_event(&meter_event(
            GAME_SOURCE_NAME,
            vec![[0.4, 0.6, 0.2], [0.5, 0.7, 0.3]],
        ));
        // Level = loudest CURRENT value, peak = loudest peak: never merged.
        assert_eq!(game, Some((0.5, 0.7)));
        assert_eq!(mic, None);

        let (game, mic) = levels_from_event(&meter_event(
            MIC_SOURCE_NAME,
            vec![[0.25, 0.9, 0.0]],
        ));
        assert_eq!(game, None);
        assert_eq!(mic, Some((0.25, 0.9)));
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
    fn peak_markers_hold_then_slide() {
        assert_eq!(decay_peak(0.8, Duration::ZERO), 0.8);
        assert_eq!(decay_peak(0.8, PEAK_HOLD), 0.8);
        let slid = decay_peak(0.8, PEAK_HOLD + DECAY / 2);
        assert!((slid - 0.4).abs() < 0.01, "{slid}");
        assert_eq!(decay_peak(0.8, PEAK_HOLD + DECAY), 0.0);
    }

    #[test]
    fn snapshot_is_none_when_inactive() {
        deactivate();
        assert!(snapshot().is_none());
    }
}
