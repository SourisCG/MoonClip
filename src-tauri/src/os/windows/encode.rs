//! Encoder argument mapping per vendor (SPEC §9). The buffer is always CBR
//! (predictable RAM); export re-encodes use CQP (`video_quality::cqp_export`).
//!
//! NVENC default preset is **p5**, the preset OBS's own Auto Configuration
//! Wizard picks for this class of hardware (`MOONCLIP_NVENC_PRESET` swaps it:
//! `p4` more headroom, `p7` = Linux parity). The default recipe is the LIGHT
//! one measured in-game on the RTX 3060 (single-pass, no look-ahead); the §9
//! full recipe is opt-in via `MOONCLIP_ENCODER_HQ_FULL=1` because look-ahead /
//! two-pass / pre-analysis starve capture under a GPU-saturating game.
//!
//! Equivalents per vendor (SPEC §9), all in the light default:
//! - NVIDIA: `spatial-aq 1`, `multipass disabled`, no `rc-lookahead`.
//! - AMD AMF: `vbaq 1`, no `preencode`/`preanalysis` (its look-ahead).
//! - Intel QSV: `preset medium`, `async_depth 4`, no `look_ahead`.
//! - CPU x264: `veryfast + zerolatency` (look-ahead off by design).
//!
//! `MOONCLIP_ENCODER_HQ_FULL=1` adds each vendor's §9 heavy knobs.
//!
//! `preset_step_down` implements the documented P5→P4 fallback when the
//! encoder falls behind (`video_lag` > 0.8 s sustained); the caller applies it
//! with a restart notice, never silently mid-buffer.

pub const DEFAULT_NVENC_PRESET: &str = "p5";
#[allow(dead_code)] // used by the save telemetry recommendation (M4/M5)
pub const LAG_STEP_DOWN_MS: f64 = 800.0;

/// NVENC preset from the env override (test/A-B) or the measured default.
pub fn nvenc_preset(env: Option<&str>) -> String {
    env.map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
        .unwrap_or_else(|| DEFAULT_NVENC_PRESET.to_string())
}

/// Full §9 HQ recipe requested (`MOONCLIP_ENCODER_HQ_FULL=1`). Off by default:
/// the light recipe is the one measured to hold capture under game load.
pub fn encoder_hq_full(env: Option<&str>) -> bool {
    matches!(env.map(str::trim), Some("1") | Some("true") | Some("on"))
}

/// Documented GPU-savings step-down: when `video_lag` sits above 0.8 s with a
/// slow preset, the recommended preset is P4 Medium (SPEC §9). Pure, so the
/// caller decides how to apply it (restart + notice).
#[allow(dead_code)] // recommendation surfaced by save telemetry (M4/M5)
pub fn preset_step_down(preset: &str, video_lag_ms: f64) -> Option<&'static str> {
    if video_lag_ms <= LAG_STEP_DOWN_MS {
        return None;
    }
    match preset {
        "p5" | "p6" | "p7" => Some("p4"),
        _ => None,
    }
}

/// ffmpeg args for the live encoder **after** `-c:v <enc>`. CBR ladder +
/// 2 s GOP everywhere; HQ knobs only where valid (NVIDIA + h264/hevc).
#[allow(clippy::too_many_arguments)] // ffmpeg-args mapper, kept flat
pub fn live_encoder_args(
    enc_name: &str,
    codec: &str,
    bitrate_kbps: u32,
    fps: u32,
    out_height: u32,
    nvenc_hq: bool,
    nvenc_preset: &str,
    encoder_full: bool,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut flag = |k: &str, v: &str| {
        out.push(k.to_string());
        out.push(v.to_string());
    };
    if enc_name.ends_with("_nvenc") {
        if nvenc_hq {
            flag("-preset", nvenc_preset);
            flag("-tune", "hq");
            flag("-profile:v", if codec == "hevc" { "main" } else { "high" });
            flag("-bf", "2");
            flag("-spatial-aq", "1");
            if encoder_full {
                // SPEC §9 reference recipe: better compression, much heavier
                // (look-ahead + two-pass + temporal AQ). Opt-in only.
                flag("-temporal-aq", "1");
                flag("-multipass", "qres");
                flag("-rc-lookahead", "20");
            } else {
                flag("-multipass", "disabled");
            }
        }
        flag("-rc", "cbr");
    } else if enc_name.ends_with("_amf") {
        // AMF `balanced ≈ quality` with far more compatibility; Quality only
        // while it fits comfortably (<=1080p60, OBS PR #9352). `preencode` is
        // AMF's look-ahead/pre-analysis: light default leaves it OFF (SPEC §9:
        // it costs latency + GPU and OBS drops it above 1080p60).
        let quality = out_height <= 1080 && fps <= 60;
        flag("-rc", "cbr");
        flag("-quality", if quality { "quality" } else { "balanced" });
        flag("-vbaq", "1");
        if encoder_full {
            flag("-preencode", "1");
            flag("-preanalysis", "1");
            flag("-pa_taq_mode", "2");
        }
    } else if enc_name.ends_with("_qsv") {
        flag("-preset", "medium");
        flag("-async_depth", "4");
        if encoder_full {
            // QSV look-ahead is the §9 heavy knob (off by default).
            flag("-look_ahead", "1");
            flag("-look_ahead_depth", "20");
        }
    } else if enc_name == "libx264" {
        flag("-preset", "veryfast");
        flag("-tune", "zerolatency");
        flag("-profile:v", "high");
        flag("-bf", "2");
    }
    let gop = (fps.max(1) * 2).to_string();
    flag("-b:v", &format!("{bitrate_kbps}k"));
    flag("-maxrate", &format!("{bitrate_kbps}k"));
    flag("-bufsize", &format!("{bitrate_kbps}k"));
    flag("-g", &gop);
    out
}

/// CBR target override from `MOONCLIP_CAPTURE_BITRATE_KBPS` (test/A-B only):
/// swaps the ladder bitrate without rebuilding. Values below 500 kbps are
/// ignored (a typo must not produce an unusable clip).
pub fn capture_bitrate_override(env: Option<&str>) -> Option<u32> {
    env?.trim().parse::<u32>().ok().filter(|v| *v >= 500)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvenc_hq_light_recipe_by_default() {
        let a = live_encoder_args("h264_nvenc", "h264", 20000, 60, 1080, true, "p5", false);
        let s = a.join(" ");
        assert!(s.contains("-preset p5"), "{s}");
        assert!(s.contains("-tune hq"), "{s}");
        assert!(s.contains("-profile:v high"), "{s}");
        assert!(s.contains("-spatial-aq 1"), "{s}");
        assert!(s.contains("-multipass disabled"), "{s}");
        assert!(!s.contains("-temporal-aq"), "{s}");
        assert!(!s.contains("-rc-lookahead"), "{s}");
        assert!(s.contains("-rc cbr"), "{s}");
        assert!(s.contains("-b:v 20000k"), "{s}");
        assert!(s.contains("-bufsize 20000k"), "{s}");
        assert!(s.contains("-g 120"), "{s}");
    }

    #[test]
    fn nvenc_hq_full_recipe_is_opt_in() {
        let a = live_encoder_args("h264_nvenc", "h264", 20000, 60, 1080, true, "p5", true);
        let s = a.join(" ");
        assert!(s.contains("-temporal-aq 1"), "{s}");
        assert!(s.contains("-multipass qres"), "{s}");
        assert!(s.contains("-rc-lookahead 20"), "{s}");
        assert!(!s.contains("-multipass disabled"), "{s}");
    }

    #[test]
    fn amf_light_has_no_preencode_full_has_it() {
        let light = live_encoder_args("h264_amf", "h264", 20000, 60, 1080, false, "p5", false)
            .join(" ");
        assert!(light.contains("-vbaq 1"), "{light}");
        assert!(!light.contains("-preencode"), "{light}");
        let full = live_encoder_args("h264_amf", "h264", 20000, 60, 1080, false, "p5", true)
            .join(" ");
        assert!(full.contains("-preencode 1"), "{full}");
        assert!(full.contains("-preanalysis 1"), "{full}");
        assert!(full.contains("-pa_taq_mode 2"), "{full}");
    }

    #[test]
    fn qsv_light_has_no_lookahead_full_has_it() {
        let light = live_encoder_args("h264_qsv", "h264", 20000, 60, 1080, false, "p5", false)
            .join(" ");
        assert!(!light.contains("-look_ahead"), "{light}");
        let full = live_encoder_args("h264_qsv", "h264", 20000, 60, 1080, false, "p5", true)
            .join(" ");
        assert!(full.contains("-look_ahead 1"), "{full}");
        assert!(full.contains("-look_ahead_depth 20"), "{full}");
    }

    #[test]
    fn encoder_full_env_parsing() {
        assert!(!encoder_hq_full(None));
        assert!(!encoder_hq_full(Some("")));
        assert!(!encoder_hq_full(Some("0")));
        assert!(encoder_hq_full(Some("1")));
        assert!(encoder_hq_full(Some("true")));
        assert!(encoder_hq_full(Some("on")));
    }

    #[test]
    fn hevc_profile_is_main() {
        let s = live_encoder_args("hevc_nvenc", "hevc", 12000, 60, 1080, true, "p5", false)
            .join(" ");
        assert!(s.contains("-profile:v main"), "{s}");
        assert!(!s.contains("profile:v high"), "{s}");
    }

    #[test]
    fn nvenc_plain_no_hq() {
        let a = live_encoder_args("av1_nvenc", "av1", 8000, 60, 1080, false, "p5", false);
        let s = a.join(" ");
        assert!(!s.contains("-preset"), "{s}");
        assert!(!s.contains("-temporal-aq"), "{s}");
        assert!(s.contains("-rc cbr"), "{s}");
    }

    #[test]
    fn x264_qsv_amf_shapes() {
        let x = live_encoder_args("libx264", "x264", 20000, 30, 1080, false, "p5", false).join(" ");
        assert!(x.contains("-preset veryfast"), "{x}");
        assert!(x.contains("zerolatency"), "{x}");
        assert!(x.contains("-g 60"), "{x}");
        let q = live_encoder_args("h264_qsv", "h264", 20000, 60, 1080, false, "p5", false).join(" ");
        assert!(q.contains("-preset medium"), "{q}");
        assert!(q.contains("-async_depth 4"), "{q}");
        // AMF: quality at 1080p60, balanced above.
        let a = live_encoder_args("h264_amf", "h264", 20000, 60, 1080, false, "p5", false).join(" ");
        assert!(a.contains("-quality quality"), "{a}");
        assert!(a.contains("-vbaq 1"), "{a}");
        let b = live_encoder_args("h264_amf", "h264", 60000, 60, 2160, false, "p5", false).join(" ");
        assert!(b.contains("-quality balanced"), "{b}");
    }

    #[test]
    fn preset_env_defaults_to_p5() {
        assert_eq!(nvenc_preset(None), "p5");
        assert_eq!(nvenc_preset(Some("")), "p5");
        assert_eq!(nvenc_preset(Some("  ")), "p5");
        assert_eq!(nvenc_preset(Some("p4")), "p4");
        assert_eq!(nvenc_preset(Some(" p7 ")), "p7");
    }

    #[test]
    fn lag_step_down_only_from_slow_presets() {
        assert_eq!(preset_step_down("p5", 900.0), Some("p4"));
        assert_eq!(preset_step_down("p6", 1200.0), Some("p4"));
        assert_eq!(preset_step_down("p7", 900.0), Some("p4"));
        assert_eq!(preset_step_down("p5", 800.0), None);
        assert_eq!(preset_step_down("p5", 200.0), None);
        assert_eq!(preset_step_down("p4", 1500.0), None);
        assert_eq!(preset_step_down("p1", 1500.0), None);
    }

    #[test]
    fn capture_bitrate_override_parses_and_guards() {
        assert_eq!(capture_bitrate_override(None), None);
        assert_eq!(capture_bitrate_override(Some("")), None);
        assert_eq!(capture_bitrate_override(Some("10000")), Some(10_000));
        assert_eq!(capture_bitrate_override(Some(" 20000 ")), Some(20_000));
        assert_eq!(capture_bitrate_override(Some("499")), None);
        assert_eq!(capture_bitrate_override(Some("junk")), None);
    }
}
