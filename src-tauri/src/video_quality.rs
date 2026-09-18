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

/// Exact NVENC HQ `-ffmpeg-video-opts` (old-MoonLit table). Dashes verified
/// live: GSR accepts every key, saves clean, bitrate lands on target.
/// Apply ONLY on NVIDIA + h264/hevc (meaningless/invalid elsewhere).
/// Profile is per-codec: `high` exists only in H.264 — HEVC uses `main`
/// (passing `high` to hevc_nvenc kills encoder init: no clip at all).
pub fn nvenc_hq_opts(codec: &str) -> String {
    let profile = if codec == "hevc" { "main" } else { "high" };
    format!("preset=p7;tune=hq;profile={profile};bf=2;spatial-aq=1;multipass=disabled")
}

#[cfg(test)]
mod tests {
    use super::{bitrate_kbps, cqp_export, nvenc_hq_opts};

    #[test]
    fn hevc_gets_main_profile() {
        let o = nvenc_hq_opts("hevc");
        assert!(o.contains("profile=main"), "{o}");
        assert!(!o.contains("profile=high"), "{o}");
    }

    #[test]
    fn h264_keeps_high_profile() {
        let o = nvenc_hq_opts("h264");
        assert!(o.contains("profile=high"), "{o}");
    }

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

/// Exact RAM/VRAM-ring megabytes for N seconds at a CBR bitrate.
pub fn ring_mb(bitrate_kbps: u32, seconds: u32) -> u32 {
    ((bitrate_kbps as u64 * seconds as u64) / 8 / 1000) as u32
}
