//! Video quality ladder (OS-free data + math).
//! Bitrates: Medal's official recommended table (CBR). NVENC HQ recipe:
//! old-MoonLit advanced table (CBR + P7 + HQ + AQ + BF2 + keyint 2s),
//! validated live against our bundled GSR (all keys accepted, bitrate on target).

/// Ladder heights offered in UI. 0 = source resolution (no -s flag).
pub const HEIGHTS: [u32; 6] = [360, 480, 720, 1080, 1440, 2160];

/// CBR kbps per (output height, codec). Medal parity (1080p h264 = 20M,
/// same figure as the old-MoonLit advanced table). `x264` (CPU) follows the
/// h264 row: software `veryfast` needs at least the same headroom.
pub fn bitrate_kbps(height: u32, codec: &str) -> u32 {
    match (height, codec) {
        (360, _) => 3000,
        (480, _) => 5000,
        (720, "h264" | "x264") => 10000,
        (720, _) => 7000,
        (1080, "h264" | "x264") => 20000,
        (1080, "hevc") => 12000,
        (1080, _) => 8000,
        (1440, "h264" | "x264") => 25000,
        (1440, "hevc") => 20000,
        (1440, _) => 15000,
        (2160, "h264" | "x264") => 60000,
        (2160, "hevc") => 35000,
        (2160, _) => 25000,
        (_, "h264" | "x264") => 20000,
        (_, "hevc") => 12000,
        (_, _) => 8000,
    }
}

/// Medal's recommended bitrate range (kbps) per height and codec — shown in
/// the settings UI next to the exact ladder cell so users understand where a
/// custom value sits (Medal support table, 2026).
pub fn recommended_kbps(height: u32, codec: &str) -> (u32, u32) {
    match (height, codec) {
        (360, _) | (480, _) => (3000, 5000),
        (720, "h264" | "x264") => (8000, 12000),
        (720, _) => (5000, 8000),
        (1080, "h264" | "x264") => (15000, 20000),
        (1080, "hevc") => (10000, 15000),
        (1080, _) => (7000, 10000),
        (1440, "h264" | "x264") => (20000, 30000),
        (1440, "hevc") => (15000, 25000),
        (1440, _) => (10000, 20000),
        (2160, "h264" | "x264") => (50000, 70000),
        (2160, "hevc") => (25000, 50000),
        (2160, _) => (20000, 35000),
        (_, "h264" | "x264") => (15000, 20000),
        (_, "hevc") => (10000, 15000),
        (_, _) => (7000, 10000),
    }
}

/// Export-time CQP/CRF per delivered height (Medal ladder, SPEC §4).
/// Only used for offline/re-encodes; the live buffer is always CBR.
#[allow(dead_code)] // wired into the export/editor path in a later phase
pub fn cqp_export(height: u32) -> u32 {
    match height {
        360 => 24,
        480 => 23,
        720 => 22,
        1440 => 19,
        2160 => 18,
        _ => 20,
    }
}

/// Exact RAM/VRAM-ring megabytes for N seconds at a CBR bitrate.
pub fn ring_mb(bitrate_kbps: u32, seconds: u32) -> u32 {
    ((bitrate_kbps as u64 * seconds as u64) / 8 / 1000) as u32
}

#[cfg(test)]
mod tests {
    use super::{bitrate_kbps, cqp_export, recommended_kbps, HEIGHTS};

    #[test]
    fn ladder_covers_medal_rows() {
        assert_eq!(bitrate_kbps(360, "h264"), 3000);
        assert_eq!(bitrate_kbps(480, "av1"), 5000);
        assert_eq!(bitrate_kbps(720, "h264"), 10000);
        assert_eq!(bitrate_kbps(720, "hevc"), 7000);
        assert_eq!(bitrate_kbps(1080, "h264"), 20000);
        assert_eq!(bitrate_kbps(1080, "hevc"), 12000);
        assert_eq!(bitrate_kbps(1080, "av1"), 8000);
        assert_eq!(bitrate_kbps(1440, "h264"), 25000);
        assert_eq!(bitrate_kbps(1440, "hevc"), 20000);
        assert_eq!(bitrate_kbps(1440, "av1"), 15000);
        assert_eq!(bitrate_kbps(2160, "h264"), 60000);
        assert_eq!(bitrate_kbps(2160, "hevc"), 35000);
        assert_eq!(bitrate_kbps(2160, "av1"), 25000);
        // x264 follows the H.264 row (software needs the same headroom).
        assert_eq!(bitrate_kbps(1080, "x264"), 20000);
        assert_eq!(bitrate_kbps(2160, "x264"), 60000);
    }

    #[test]
    fn recommended_ranges_bracket_the_ladder() {
        for &h in &HEIGHTS {
            for codec in ["h264", "hevc", "av1", "x264"] {
                let (min, max) = recommended_kbps(h, codec);
                assert!(min < max, "{h} {codec} {min}-{max}");
                let pick = bitrate_kbps(h, codec);
                assert!(
                    pick >= min && pick <= max,
                    "{h}p {codec}: ladder {pick} outside Medal range {min}-{max}"
                );
            }
        }
        // Known Medal rows.
        assert_eq!(recommended_kbps(1080, "h264"), (15000, 20000));
        assert_eq!(recommended_kbps(2160, "hevc"), (25000, 50000));
    }

    #[test]
    fn cqp_export_matches_ladder() {
        assert_eq!(cqp_export(360), 24);
        assert_eq!(cqp_export(480), 23);
        assert_eq!(cqp_export(720), 22);
        assert_eq!(cqp_export(1080), 20);
        assert_eq!(cqp_export(1440), 19);
        assert_eq!(cqp_export(2160), 18);
        // Unknown/source heights keep the 1080p value.
        assert_eq!(cqp_export(0), 20);
        assert_eq!(cqp_export(1234), 20);
    }
}
