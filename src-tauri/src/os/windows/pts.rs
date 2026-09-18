//! Shared time domain for the Windows engine.
//!
//! One clock for everything: audio blocks, video PTS and the QPC grid all map
//! through `qpc_ns()`. The video anchor carries a single rig-measured bias
//! (`DEFAULT_SYNC_BIAS_MS`, override `MOONCLIP_SYNC_BIAS_MS`) that absorbs the
//! compositor/frame-pool delivery lag the PES timestamps cannot see; it is NOT
//! per codec and must be re-measured after driver/monitor/compositor changes.

use std::sync::OnceLock;
use windows::Win32::System::Performance::{
    QueryPerformanceCounter, QueryPerformanceFrequency,
};

/// Default video-anchor bias in ms: the captured frame reaches the encoder
/// ~70 ms after its presentation timestamp (compositor + frame pool), measured
/// with the flash+beep rig (`live_av_offset_capture` + `analyze_av.py`).
pub const DEFAULT_SYNC_BIAS_MS: i64 = 70;

fn qpc_freq_hz() -> u64 {
    static FREQ: OnceLock<u64> = OnceLock::new();
    *FREQ.get_or_init(|| {
        let mut f = 0i64;
        unsafe {
            let _ = QueryPerformanceFrequency(&mut f);
        }
        (f.max(1)) as u64
    })
}

/// Local QPC in nanoseconds (the clock WASAPI/WGC timestamps use).
pub fn qpc_ns() -> i128 {
    qpc_ticks_to_ns(qpc_ticks())
}

/// Raw QPC ticks.
fn qpc_ticks() -> u64 {
    let mut c = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut c);
    }
    c.max(0) as u64
}

/// Convert raw QPC counter ticks to nanoseconds. WASAPI's `QPCPosition`
/// (`BufferInfo::timestamp`) carries raw counter ticks; mapping them through
/// this function keeps audio and video in the same clock domain.
pub(super) fn qpc_ticks_to_ns(ticks: u64) -> i128 {
    ticks as i128 * 1_000_000_000 / qpc_freq_hz() as i128
}

fn bias_ns_from(env: Option<&str>) -> i128 {
    let ms = env
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(DEFAULT_SYNC_BIAS_MS);
    ms as i128 * 1_000_000
}

/// Video-anchor bias in ns, env-overridable (pure part unit-tested).
pub(super) fn default_sync_bias_ns() -> i128 {
    bias_ns_from(std::env::var("MOONCLIP_SYNC_BIAS_MS").ok().as_deref())
}

#[cfg(test)]
mod tests {
    use super::{bias_ns_from, qpc_ns, DEFAULT_SYNC_BIAS_MS};

    #[test]
    fn bias_defaults_and_parses_override() {
        assert_eq!(bias_ns_from(None), DEFAULT_SYNC_BIAS_MS as i128 * 1_000_000);
        assert_eq!(bias_ns_from(Some("")), DEFAULT_SYNC_BIAS_MS as i128 * 1_000_000);
        assert_eq!(bias_ns_from(Some("120")), 120_000_000);
        assert_eq!(bias_ns_from(Some(" 0 ")), 0);
        assert_eq!(bias_ns_from(Some("junk")), DEFAULT_SYNC_BIAS_MS as i128 * 1_000_000);
    }

    #[test]
    fn qpc_is_monotonic_and_nanosecond_scaled() {
        let a = qpc_ns();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = qpc_ns();
        assert!(b > a, "{a} -> {b}");
        let delta_ms = (b - a) as f64 / 1e6;
        assert!((4.0..=200.0).contains(&delta_ms), "{delta_ms} ms");
    }
}
