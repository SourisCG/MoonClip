//! Encoder option registry for the pinned embedded OBS (32.2.2).
//!
//! Single source of truth for every video-encoder setting MoonClip writes
//! or offers in Custom mode. OBS silently ignores unknown keys, so every
//! key/value here is pinned against the plugin sources and covered by tests.
//!
//! Provenance (obs-studio@32.2.2):
//! - NVENC: `plugins/obs-nvenc/nvenc-properties.c`
//!   (keys: rate_control/bitrate/target_quality/max_bitrate/cqp/keyint_sec/
//!   preset/tune/multipass/profile/lookahead/adaptive_quantization/device/
//!   bf/bframe_ref_mode/split_encode/opts + hidden repeat_headers)
//! - x264: `plugins/obs-x264/obs-x264.c`
//! - QSV: `plugins/obs-qsv11/obs-qsv11.c` v2 properties
//!   (ids obs_qsv11_v2 / obs_qsv11_hevc / obs_qsv11_av1)
//! - AMF: `plugins/obs-ffmpeg/texture-amf.cpp` (+ string check against the
//!   shipped obs-ffmpeg.dll: encoder ids, rate_control QVBR/VBR_LAT/HQVBR,
//!   AMF.Preset.*, BFrames, pre_analysis, params)
//! - VAAPI: `plugins/obs-ffmpeg/obs-ffmpeg-vaapi.c`
//!   (ids ffmpeg_vaapi / hevc_ffmpeg_vaapi / av1_ffmpeg_vaapi; profile/level
//!   are FFmpeg enum ints, qp is 0-51 with x5 internally for AV1)

use serde_json::{Map, Value};

use crate::os::api::{CustomEncoder, CustomVideo};

/// Encoder family behind one OBS encoder id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderFamily {
    Nvenc,
    X264,
    Qsv,
    Amf,
    Vaapi,
}

/// Which OBS encoder id belongs to which family. Strict: only exact ids
/// from the pinned catalogs resolve. Unknown/legacy ids are rejected loudly
/// (a typo must never produce a mis-rendered config; legacy `ffmpeg_nvenc`
/// is not in OBS 32's output list).
pub fn family_of(encoder_id: &str) -> Option<EncoderFamily> {
    catalog_windows()
        .iter()
        .chain(catalog_linux())
        .find(|e| e.id == encoder_id)
        .map(|e| e.family)
}

/// Families MoonClip has validated live on real hardware. Unvalidated
/// families render the Auto recipe (CBR + bitrate only, OBS picks the rest)
// TODO(qsv/amf/vaapi): flip to true once a hardware owner runs "Probar 10 s".
pub fn family_validated(family: EncoderFamily) -> bool {
    matches!(family, EncoderFamily::Nvenc | EncoderFamily::X264)
}

/// One OBS encoder with its app codec and family.
#[derive(Debug, Clone, Copy)]
pub struct EncoderEntry {
    pub id: &'static str,
    pub codec: &'static str,
    pub family: EncoderFamily,
}

/// Encoders compiled into the pinned OBS Windows build.
pub fn catalog_windows() -> &'static [EncoderEntry] {
    &[
        EncoderEntry {
            id: "obs_nvenc_h264_tex",
            codec: "h264",
            family: EncoderFamily::Nvenc,
        },
        EncoderEntry {
            id: "obs_nvenc_hevc_tex",
            codec: "hevc",
            family: EncoderFamily::Nvenc,
        },
        EncoderEntry {
            id: "obs_nvenc_av1_tex",
            codec: "av1",
            family: EncoderFamily::Nvenc,
        },
        EncoderEntry {
            id: "h264_texture_amf",
            codec: "h264",
            family: EncoderFamily::Amf,
        },
        EncoderEntry {
            id: "h265_texture_amf",
            codec: "hevc",
            family: EncoderFamily::Amf,
        },
        EncoderEntry {
            id: "av1_texture_amf",
            codec: "av1",
            family: EncoderFamily::Amf,
        },
        EncoderEntry {
            id: "obs_qsv11_v2",
            codec: "h264",
            family: EncoderFamily::Qsv,
        },
        EncoderEntry {
            id: "obs_qsv11_hevc",
            codec: "hevc",
            family: EncoderFamily::Qsv,
        },
        EncoderEntry {
            id: "obs_qsv11_av1",
            codec: "av1",
            family: EncoderFamily::Qsv,
        },
        EncoderEntry {
            id: "obs_x264",
            codec: "h264",
            family: EncoderFamily::X264,
        },
    ]
}

/// Encoders compiled into the pinned OBS Linux build (no AMF upstream).
pub fn catalog_linux() -> &'static [EncoderEntry] {
    &[
        EncoderEntry {
            id: "obs_nvenc_h264_tex",
            codec: "h264",
            family: EncoderFamily::Nvenc,
        },
        EncoderEntry {
            id: "obs_nvenc_hevc_tex",
            codec: "hevc",
            family: EncoderFamily::Nvenc,
        },
        EncoderEntry {
            id: "obs_nvenc_av1_tex",
            codec: "av1",
            family: EncoderFamily::Nvenc,
        },
        EncoderEntry {
            id: "obs_qsv11_v2",
            codec: "h264",
            family: EncoderFamily::Qsv,
        },
        EncoderEntry {
            id: "obs_qsv11_hevc",
            codec: "hevc",
            family: EncoderFamily::Qsv,
        },
        EncoderEntry {
            id: "obs_qsv11_av1",
            codec: "av1",
            family: EncoderFamily::Qsv,
        },
        EncoderEntry {
            id: "ffmpeg_vaapi",
            codec: "h264",
            family: EncoderFamily::Vaapi,
        },
        EncoderEntry {
            id: "hevc_ffmpeg_vaapi",
            codec: "hevc",
            family: EncoderFamily::Vaapi,
        },
        EncoderEntry {
            id: "av1_ffmpeg_vaapi",
            codec: "av1",
            family: EncoderFamily::Vaapi,
        },
        EncoderEntry {
            id: "obs_x264",
            codec: "h264",
            family: EncoderFamily::X264,
        },
    ]
}

/// Families the detected GPU vendor can drive (auto-selection backend).
pub fn families_for_vendor(vendor: &str) -> &'static [EncoderFamily] {
    match vendor {
        "nvidia" => &[EncoderFamily::Nvenc, EncoderFamily::X264],
        "amd" => &[
            EncoderFamily::Amf,
            EncoderFamily::Vaapi,
            EncoderFamily::X264,
        ],
        "intel" => &[EncoderFamily::Qsv, EncoderFamily::X264],
        _ => &[EncoderFamily::X264],
    }
}

// ---------------------------------------------------------------------------
// Option schema
// ---------------------------------------------------------------------------

/// Schema kind of one encoder option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecKind {
    /// String from a fixed list (`values`).
    Enum,
    /// Integer: free range (`min..=max`, `step`) or fixed list (`int_values`).
    Int,
    Bool,
    /// Free text (x264opts / opts / ffmpeg_opts / params).
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecDefault {
    Str(&'static str),
    Int(i64),
    Bool(bool),
}

/// Visibility rule evaluated against the current draft: every `when`
/// condition must hold, and the `and_when` condition too when present.
/// Sibling values compare as strings (`"true"`/`"false"` for bools).
#[derive(Debug, Clone, Copy)]
pub struct VisibleRule {
    pub when: Option<(&'static str, &'static [&'static str])>,
    pub and_when: Option<(&'static str, &'static [&'static str])>,
}

impl VisibleRule {
    pub const ALWAYS: VisibleRule = VisibleRule {
        when: None,
        and_when: None,
    };
    pub const fn when(key: &'static str, values: &'static [&'static str]) -> VisibleRule {
        VisibleRule {
            when: Some((key, values)),
            and_when: None,
        }
    }
}

/// One encoder option exactly as OBS names it.
#[derive(Debug, Clone, Copy)]
pub struct OptionSpec {
    pub key: &'static str,
    pub kind: SpecKind,
    /// Enum string values (empty for non-enum).
    pub values: &'static [&'static str],
    /// Fixed int values (empty = free `min..=max` range).
    pub int_values: &'static [i64],
    pub min: i64,
    pub max: i64,
    pub step: i64,
    /// OBS default; `None` = unset by OBS defaults (writes nothing).
    pub default: Option<SpecDefault>,
    /// Frontend i18n key (no human text crosses IPC).
    pub i18n: &'static str,
    /// Visibility against the sibling draft (None parts = always).
    pub visible: VisibleRule,
    /// Codecs this option is valid for (empty = all). Others are rejected
    /// by validate() and greyed out by the UI.
    pub codecs: &'static [&'static str],
    /// Per-codec enum value lists (empty = `values` for every codec).
    pub codec_values: &'static [(&'static str, &'static [&'static str])],
    /// Per-codec int (min, max) (empty = `min..=max` for every codec).
    pub codec_range: &'static [(&'static str, i64, i64)],
    /// Per-codec fixed int values (empty = `int_values` for every codec).
    pub codec_int_values: &'static [(&'static str, &'static [i64])],
    /// Per-codec defaults (empty = `default` for every codec).
    pub codec_defaults: &'static [(&'static str, SpecDefault)],
    /// Enum values only valid with P010 color format (10-bit profiles).
    pub p010_values: &'static [&'static str],
    /// Int values only valid with P010 (VAAPI HEVC profile main10 = 2).
    pub p010_ints: &'static [i64],
}

const fn en(
    key: &'static str,
    values: &'static [&'static str],
    default: &'static str,
    i18n: &'static str,
    visible: VisibleRule,
) -> OptionSpec {
    OptionSpec {
        key,
        kind: SpecKind::Enum,
        values,
        int_values: &[],
        min: 0,
        max: 0,
        step: 0,
        default: Some(SpecDefault::Str(default)),
        i18n,
        visible,
        codecs: &[],
        codec_values: &[],
        codec_range: &[],
        codec_int_values: &[],
        codec_defaults: &[],
        p010_values: &[],
        p010_ints: &[],
    }
}

const fn int(
    key: &'static str,
    min: i64,
    max: i64,
    step: i64,
    default: i64,
    i18n: &'static str,
    visible: VisibleRule,
) -> OptionSpec {
    OptionSpec {
        key,
        kind: SpecKind::Int,
        values: &[],
        int_values: &[],
        min,
        max,
        step,
        default: Some(SpecDefault::Int(default)),
        i18n,
        visible,
        codecs: &[],
        codec_values: &[],
        codec_range: &[],
        codec_int_values: &[],
        codec_defaults: &[],
        p010_values: &[],
        p010_ints: &[],
    }
}

const fn boolean(key: &'static str, default: bool, i18n: &'static str) -> OptionSpec {
    OptionSpec {
        key,
        kind: SpecKind::Bool,
        values: &[],
        int_values: &[],
        min: 0,
        max: 1,
        step: 1,
        default: Some(SpecDefault::Bool(default)),
        i18n,
        visible: VisibleRule::ALWAYS,
        codecs: &[],
        codec_values: &[],
        codec_range: &[],
        codec_int_values: &[],
        codec_defaults: &[],
        p010_values: &[],
        p010_ints: &[],
    }
}

const fn text(key: &'static str, i18n: &'static str) -> OptionSpec {
    OptionSpec {
        key,
        kind: SpecKind::Text,
        values: &[],
        int_values: &[],
        min: 0,
        max: 0,
        step: 0,
        default: None,
        i18n,
        visible: VisibleRule::ALWAYS,
        codecs: &[],
        codec_values: &[],
        codec_range: &[],
        codec_int_values: &[],
        codec_defaults: &[],
        p010_values: &[],
        p010_ints: &[],
    }
}

// Visibility groups shared by families (mirror the OBS property callbacks);
// `VisibleRule::when` takes the same shape so tables stay declarative.
const RC_BITRATE: VisibleRule = VisibleRule::when("rate_control", &["CBR", "VBR"]);
const RC_VBR: VisibleRule = VisibleRule::when("rate_control", &["VBR"]);
const RC_CQVBR: VisibleRule = VisibleRule::when("rate_control", &["CQVBR"]);
const RC_CQP: VisibleRule = VisibleRule::when("rate_control", &["CQP"]);
const RC_ICQ: VisibleRule = VisibleRule::when("rate_control", &["ICQ"]);
// NVENC: max_bitrate shows for VBR and CQVBR (nvenc-properties.c).
const RC_VBR_CQVBR: VisibleRule = VisibleRule::when("rate_control", &["VBR", "CQVBR"]);
// NVENC preset/tune/AQ hide on lossless (everything else shows them).
const RC_NOT_LOSSLESS: VisibleRule =
    VisibleRule::when("rate_control", &["CBR", "CQP", "VBR", "CQVBR"]);
// x264: bitrate/bufsize cover CBR+ABR+VBR; CRF shows for VBR (uses CRF
// internally) and CRF (obs-x264.c rate_control_modified).
const RC_X264_BITRATE: VisibleRule = VisibleRule::when("rate_control", &["CBR", "ABR", "VBR"]);
const RC_X264_CRF: VisibleRule = VisibleRule::when("rate_control", &["VBR", "CRF"]);
const RC_X264_BUFSIZE: VisibleRule = VisibleRule {
    when: Some(("rate_control", &["CBR", "ABR", "VBR"])),
    and_when: Some(("use_bufsize", &["true"])),
};
// VAAPI: qp covers CQP+QVBR, bitrate CBR+VBR+QVBR (vaapi_encode rc table).
const RC_VAAPI_QP: VisibleRule = VisibleRule::when("rate_control", &["CQP", "QVBR"]);
const RC_VAAPI_BITRATE: VisibleRule = VisibleRule::when("rate_control", &["CBR", "VBR", "QVBR"]);
// AMF: bitrate hides on CQP/QVBR, cqp shows on CQP/QVBR (texture-amf.cpp).
const RC_AMF_BITRATE: VisibleRule =
    VisibleRule::when("rate_control", &["CBR", "VBR", "VBR_LAT", "HQVBR", "HQCBR"]);
const RC_AMF_CQP: VisibleRule = VisibleRule::when("rate_control", &["CQP", "QVBR"]);

/// Full option list per family (all rendered into recordEncoder.json).
pub fn options_for(family: EncoderFamily) -> &'static [OptionSpec] {
    match family {
        EncoderFamily::Nvenc => NVENC_OPTS,
        EncoderFamily::X264 => X264_OPTS,
        EncoderFamily::Qsv => QSV_OPTS,
        EncoderFamily::Amf => AMF_OPTS,
        EncoderFamily::Vaapi => VAAPI_OPTS,
    }
}

// NVENC (obs-nvenc/nvenc-properties.c). `preset`/`adaptive_quantization`/
// `device` are the OBS 30+ keys; the OBS 29-era `preset2`/`psycho_aq`/`gpu`
// are ignored by 32.2.2 and must never be written.
static NVENC_OPTS: &[OptionSpec] = &[
    en(
        "rate_control",
        &["CBR", "CQP", "VBR", "CQVBR", "lossless"],
        "CBR",
        "obsopt.rate_control",
        VisibleRule::ALWAYS,
    ),
    int(
        "bitrate",
        50,
        4_294_967,
        50,
        10000,
        "obsopt.bitrate",
        RC_BITRATE,
    ),
    int(
        "max_bitrate",
        0,
        4_294_967,
        50,
        10000,
        "obsopt.max_bitrate",
        RC_VBR_CQVBR,
    ),
    // CQVBR-only target quality (AV1 allows 0-63, H.264/HEVC 1-51).
    OptionSpec {
        key: "target_quality",
        kind: SpecKind::Int,
        values: &[],
        int_values: &[],
        min: 1,
        max: 51,
        step: 1,
        default: Some(SpecDefault::Int(20)),
        i18n: "obsopt.target_quality",
        visible: RC_CQVBR,
        codecs: &[],
        codec_values: &[],
        codec_range: &[("av1", 1, 63)],
        codec_int_values: &[],
        codec_defaults: &[],
        p010_values: &[],
        p010_ints: &[],
    },
    // CQP level (AV1 uses 0-63, H.264/HEVC 0-51).
    OptionSpec {
        key: "cqp",
        kind: SpecKind::Int,
        values: &[],
        int_values: &[],
        min: 1,
        max: 51,
        step: 1,
        default: Some(SpecDefault::Int(20)),
        i18n: "obsopt.cqp",
        visible: RC_CQP,
        codecs: &[],
        codec_values: &[],
        codec_range: &[("av1", 1, 63)],
        codec_int_values: &[],
        codec_defaults: &[],
        p010_values: &[],
        p010_ints: &[],
    },
    int(
        "keyint_sec",
        0,
        10,
        1,
        0,
        "obsopt.keyint",
        VisibleRule::ALWAYS,
    ),
    en(
        "preset",
        &["p1", "p2", "p3", "p4", "p5", "p6", "p7"],
        "p5",
        "obsopt.preset",
        RC_NOT_LOSSLESS,
    ),
    // uhq needs Turing+; OBS gates it by caps at runtime.
    en(
        "tune",
        &["uhq", "hq", "ll", "ull"],
        "hq",
        "obsopt.tune",
        RC_NOT_LOSSLESS,
    ),
    en(
        "multipass",
        &["disabled", "qres", "fullres"],
        "qres",
        "obsopt.multipass",
        VisibleRule::ALWAYS,
    ),
    // Profile values are codec-scoped; 10-bit profiles need P010.
    OptionSpec {
        key: "profile",
        kind: SpecKind::Enum,
        values: &["high", "high10", "main", "main10", "baseline"],
        int_values: &[],
        min: 0,
        max: 0,
        step: 0,
        default: Some(SpecDefault::Str("high")),
        i18n: "obsopt.profile",
        visible: VisibleRule::ALWAYS,
        codecs: &[],
        codec_values: &[
            ("h264", &["high", "high10", "main", "baseline"]),
            ("hevc", &["main10", "main"]),
            ("av1", &["main"]),
        ],
        codec_range: &[],
        codec_int_values: &[],
        codec_defaults: &[
            ("hevc", SpecDefault::Str("main")),
            ("av1", SpecDefault::Str("main")),
        ],
        p010_values: &["high10", "main10"],
        p010_ints: &[],
    },
    boolean("lookahead", true, "obsopt.lookahead"),
    boolean(
        "adaptive_quantization",
        true,
        "obsopt.adaptive_quantization",
    ),
    // -1 = auto; MoonClip's gpu_index 0 maps to -1, N>0 to N.
    int("device", -1, 16, 1, -1, "obsopt.gpu", VisibleRule::ALWAYS),
    int("bf", 0, 4, 1, 2, "obsopt.bframes", VisibleRule::ALWAYS),
    text("opts", "obsopt.opts"),
];

// x264 (obs-x264/obs-x264.c). x264 preset/tune names are libx264 constants.
static X264_OPTS: &[OptionSpec] = &[
    en(
        "rate_control",
        &["CBR", "ABR", "VBR", "CRF"],
        "CBR",
        "obsopt.rate_control",
        VisibleRule::ALWAYS,
    ),
    int(
        "bitrate",
        50,
        10_000_000,
        50,
        6000,
        "obsopt.bitrate",
        RC_X264_BITRATE,
    ),
    boolean("use_bufsize", false, "obsopt.use_bufsize"),
    int(
        "buffer_size",
        0,
        10_000_000,
        1,
        6000,
        "obsopt.buffer_size",
        RC_X264_BUFSIZE,
    ),
    int("crf", 0, 51, 1, 23, "obsopt.crf", RC_X264_CRF),
    int(
        "keyint_sec",
        0,
        20,
        1,
        0,
        "obsopt.keyint",
        VisibleRule::ALWAYS,
    ),
    en(
        "preset",
        &[
            "ultrafast",
            "superfast",
            "veryfast",
            "faster",
            "fast",
            "medium",
            "slow",
            "slower",
            "veryslow",
            "placebo",
        ],
        "veryfast",
        "obsopt.preset",
        VisibleRule::ALWAYS,
    ),
    // "" = none.
    en(
        "profile",
        &["", "baseline", "main", "high"],
        "",
        "obsopt.profile",
        VisibleRule::ALWAYS,
    ),
    en(
        "tune",
        &[
            "",
            "film",
            "animation",
            "grain",
            "stillimage",
            "psnr",
            "ssim",
            "fastdecode",
            "zerolatency",
        ],
        "",
        "obsopt.tune",
        VisibleRule::ALWAYS,
    ),
    // Optional override (no OBS default): omitted unless the user sets it.
    OptionSpec {
        key: "bf",
        kind: SpecKind::Int,
        values: &[],
        int_values: &[],
        min: 0,
        max: 16,
        step: 1,
        default: None,
        i18n: "obsopt.bframes",
        visible: VisibleRule::ALWAYS,
        codecs: &[],
        codec_values: &[],
        codec_range: &[],
        codec_int_values: &[],
        codec_defaults: &[],
        p010_values: &[],
        p010_ints: &[],
    },
    text("x264opts", "obsopt.opts"),
    boolean("repeat_headers", false, "obsopt.repeat_headers"),
];

// QSV v2 (obs-qsv11/obs-qsv11.c). Legacy `preset`/`async_depth` are migrated
// away by OBS itself (`preset`→TU, `async_depth`→latency); write the TU keys.
static QSV_OPTS: &[OptionSpec] = &[
    en(
        "rate_control",
        &["CBR", "VBR", "CQP", "ICQ"],
        "CBR",
        "obsopt.rate_control",
        VisibleRule::ALWAYS,
    ),
    int(
        "bitrate",
        50,
        10_000_000,
        50,
        5000,
        "obsopt.bitrate",
        RC_BITRATE,
    ),
    int(
        "max_bitrate",
        50,
        10_000_000,
        50,
        6000,
        "obsopt.max_bitrate",
        RC_VBR,
    ),
    OptionSpec {
        key: "cqp",
        kind: SpecKind::Int,
        values: &[],
        int_values: &[],
        min: 1,
        max: 51,
        step: 1,
        default: Some(SpecDefault::Int(23)),
        i18n: "obsopt.cqp",
        visible: RC_CQP,
        codecs: &[],
        codec_values: &[],
        codec_range: &[("av1", 1, 63)],
        codec_int_values: &[],
        codec_defaults: &[],
        p010_values: &[],
        p010_ints: &[],
    },
    int("icq_quality", 1, 51, 1, 23, "obsopt.icq", RC_ICQ),
    en(
        "target_usage",
        &["TU1", "TU2", "TU3", "TU4", "TU5", "TU6", "TU7"],
        "TU4",
        "obsopt.target_usage",
        VisibleRule::ALWAYS,
    ),
    OptionSpec {
        key: "profile",
        kind: SpecKind::Enum,
        values: &["baseline", "main", "high", "main10"],
        int_values: &[],
        min: 0,
        max: 0,
        step: 0,
        default: Some(SpecDefault::Str("high")),
        i18n: "obsopt.profile",
        visible: VisibleRule::ALWAYS,
        codecs: &[],
        codec_values: &[
            ("h264", &["baseline", "main", "high"]),
            ("hevc", &["main", "main10"]),
            ("av1", &["main"]),
        ],
        codec_range: &[],
        codec_int_values: &[],
        codec_defaults: &[
            ("hevc", SpecDefault::Str("main")),
            ("av1", SpecDefault::Str("main")),
        ],
        p010_values: &["main10"],
        p010_ints: &[],
    },
    int(
        "keyint_sec",
        0,
        20,
        1,
        0,
        "obsopt.keyint",
        VisibleRule::ALWAYS,
    ),
    en(
        "latency",
        &["normal", "ultra-low", "low"],
        "normal",
        "obsopt.latency",
        VisibleRule::ALWAYS,
    ),
    int("bframes", 0, 3, 1, 3, "obsopt.bframes", VisibleRule::ALWAYS),
    boolean("repeat_headers", false, "obsopt.repeat_headers"),
];

// AMF (obs-ffmpeg/texture-amf.cpp, verified against source 32.2.2).
// UI keys: rate_control/bitrate/cqp/keyint_sec/preset/profile/pre_analysis/
// bf/ffmpeg_opts. `vbaq`/`enforce_hrd` are NOT UI keys (hardcoded true
// inside OBS) and must never be written; `bf` exists only for AVC/AV1
// (0..5); HEVC has no profile list at all.
static AMF_OPTS: &[OptionSpec] = &[
    en(
        "rate_control",
        &["CBR", "CQP", "VBR", "VBR_LAT", "QVBR", "HQVBR", "HQCBR"],
        "CBR",
        "obsopt.rate_control",
        VisibleRule::ALWAYS,
    ),
    int(
        "bitrate",
        50,
        100_000,
        50,
        20000,
        "obsopt.bitrate",
        RC_AMF_BITRATE,
    ),
    OptionSpec {
        key: "cqp",
        kind: SpecKind::Int,
        values: &[],
        int_values: &[],
        min: 0,
        max: 51,
        step: 1,
        default: Some(SpecDefault::Int(20)),
        i18n: "obsopt.cqp",
        visible: RC_AMF_CQP,
        codecs: &[],
        codec_values: &[],
        codec_range: &[("av1", 0, 63)],
        codec_int_values: &[],
        codec_defaults: &[],
        p010_values: &[],
        p010_ints: &[],
    },
    int(
        "keyint_sec",
        0,
        10,
        1,
        0,
        "obsopt.keyint",
        VisibleRule::ALWAYS,
    ),
    OptionSpec {
        key: "preset",
        kind: SpecKind::Enum,
        values: &["speed", "balanced", "quality", "highQuality"],
        int_values: &[],
        min: 0,
        max: 0,
        step: 0,
        default: Some(SpecDefault::Str("quality")),
        i18n: "obsopt.amf_preset",
        visible: VisibleRule::ALWAYS,
        codecs: &[],
        codec_values: &[
            ("h264", &["speed", "balanced", "quality"]),
            ("hevc", &["speed", "balanced", "quality"]),
            ("av1", &["highQuality", "quality", "balanced", "speed"]),
        ],
        codec_range: &[],
        codec_int_values: &[],
        codec_defaults: &[("av1", SpecDefault::Str("highQuality"))],
        p010_values: &[],
        p010_ints: &[],
    },
    OptionSpec {
        key: "profile",
        kind: SpecKind::Enum,
        values: &["high", "main", "baseline"],
        int_values: &[],
        min: 0,
        max: 0,
        step: 0,
        default: Some(SpecDefault::Str("high")),
        i18n: "obsopt.profile",
        visible: VisibleRule::ALWAYS,
        codecs: &["h264", "av1"],
        codec_values: &[("h264", &["high", "main", "baseline"]), ("av1", &["main"])],
        codec_range: &[],
        codec_int_values: &[],
        codec_defaults: &[("av1", SpecDefault::Str("main"))],
        p010_values: &[],
        p010_ints: &[],
    },
    OptionSpec {
        key: "bf",
        kind: SpecKind::Int,
        values: &[],
        int_values: &[],
        min: 0,
        max: 5,
        step: 1,
        default: Some(SpecDefault::Int(2)),
        i18n: "obsopt.bframes",
        visible: VisibleRule::ALWAYS,
        codecs: &["h264", "av1"],
        codec_values: &[],
        codec_range: &[],
        codec_int_values: &[],
        codec_defaults: &[],
        p010_values: &[],
        p010_ints: &[],
    },
    boolean("pre_analysis", false, "obsopt.preanalysis"),
    text("ffmpeg_opts", "obsopt.opts"),
];

// VAAPI per-codec int lists (FFmpeg ABI values; slices need names to live
// in statics).
static VAAPI_PROFILE_H264: &[i64] = &[578, 77, 100];
static VAAPI_PROFILE_HEVC: &[i64] = &[1, 2];
static VAAPI_PROFILE_AV1: &[i64] = &[0];
static VAAPI_LEVEL_H264: &[i64] = &[30, 31, 40, 41, 42, 50, 51, 52];
static VAAPI_LEVEL_HEVC: &[i64] = &[90, 93, 120, 123, 150, 153, 156];
static VAAPI_LEVEL_AV1: &[i64] = &[4, 5, 8, 9, 12, 13, 14, 15];

// VAAPI (obs-ffmpeg/obs-ffmpeg-vaapi.c). profile/level are FFmpeg enum ints.
static VAAPI_OPTS: &[OptionSpec] = &[
    text("vaapi_device", "obsopt.vaapi_device"),
    en(
        "rate_control",
        &["CBR", "VBR", "QVBR", "CQP"],
        "CBR",
        "obsopt.rate_control",
        VisibleRule::ALWAYS,
    ),
    // Codec-scoped ints (FFmpeg AVProfile / level values).
    OptionSpec {
        key: "profile",
        kind: SpecKind::Int,
        values: &[],
        int_values: &[],
        min: -1,
        max: 1000,
        step: 1,
        default: None,
        i18n: "obsopt.profile",
        visible: VisibleRule::ALWAYS,
        codecs: &[],
        codec_values: &[],
        codec_range: &[],
        codec_int_values: &[
            ("h264", VAAPI_PROFILE_H264),
            ("hevc", VAAPI_PROFILE_HEVC),
            ("av1", VAAPI_PROFILE_AV1),
        ],
        codec_defaults: &[],
        p010_values: &[],
        // HEVC Main10 (2): only with P010.
        p010_ints: &[2],
    },
    OptionSpec {
        key: "level",
        kind: SpecKind::Int,
        values: &[],
        int_values: &[],
        min: -99,
        max: 156,
        step: 1,
        default: Some(SpecDefault::Int(-99)),
        i18n: "obsopt.level",
        visible: VisibleRule::ALWAYS,
        codecs: &[],
        codec_values: &[],
        codec_range: &[],
        codec_int_values: &[
            ("h264", VAAPI_LEVEL_H264),
            ("hevc", VAAPI_LEVEL_HEVC),
            ("av1", VAAPI_LEVEL_AV1),
        ],
        codec_defaults: &[],
        p010_values: &[],
        p010_ints: &[],
    },
    int(
        "bitrate",
        0,
        300_000,
        50,
        6000,
        "obsopt.bitrate",
        RC_VAAPI_BITRATE,
    ),
    // maxrate visible for VBR/QVBR (CBR forces maxrate=bitrate in OBS).
    OptionSpec {
        key: "maxrate",
        kind: SpecKind::Int,
        values: &[],
        int_values: &[],
        min: 0,
        max: 300_000,
        step: 50,
        default: Some(SpecDefault::Int(0)),
        i18n: "obsopt.maxrate",
        visible: VisibleRule::when("rate_control", &["VBR", "QVBR"]),
        codecs: &[],
        codec_values: &[],
        codec_range: &[],
        codec_int_values: &[],
        codec_defaults: &[],
        p010_values: &[],
        p010_ints: &[],
    },
    // QP is 0-51 in the UI (AV1 multiplies by 5 internally); QVBR also
    // takes a QP target (vaapi_encode rc table).
    int("qp", 0, 51, 1, 20, "obsopt.qp", RC_VAAPI_QP),
    int(
        "keyint_sec",
        0,
        20,
        1,
        0,
        "obsopt.keyint",
        VisibleRule::ALWAYS,
    ),
    int("bf", 0, 4, 1, 0, "obsopt.bframes", VisibleRule::ALWAYS),
    text("ffmpeg_opts", "obsopt.opts"),
];

/// Color formats each family accepts for the given app codec.
/// NV12 stays the safe default everywhere.
pub fn valid_color_formats(family: EncoderFamily, codec: &str) -> &'static [&'static str] {
    match (family, codec) {
        // x264 rejects high-precision formats (obs-x264.c create).
        (EncoderFamily::X264, _) => &["NV12", "I420", "I444"],
        // QSV H.264 is NV12-only (obs-qsv11.c valid_format).
        (EncoderFamily::Qsv, "h264") => &["NV12"],
        // QSV/AMF/VAAPI HEVC+AV1 accept NV12 and P010.
        (EncoderFamily::Qsv, _)
        | (EncoderFamily::Amf, "hevc")
        | (EncoderFamily::Amf, "av1")
        | (EncoderFamily::Vaapi, "hevc")
        | (EncoderFamily::Vaapi, "av1") => &["NV12", "P010"],
        // AMF/VAAPI H.264: NV12 (+I420 accepted by OBS negotiation).
        (EncoderFamily::Amf, _) | (EncoderFamily::Vaapi, _) => &["NV12", "I420"],
        // NVENC accepts all pipeline formats.
        (EncoderFamily::Nvenc, _) => &["NV12", "I420", "I444", "P010"],
    }
}

/// Stored `[Video]` color/range/scale strings (libobs convention).
pub const COLOR_SPACES: &[&str] = &["601", "709", "sRGB", "2100PQ", "2100HLG"];
pub const COLOR_RANGES: &[&str] = &["Partial", "Full"];
pub const SCALE_FILTERS: &[&str] = &["disable", "point", "bilinear", "bicubic", "lanczos", "area"];

// ---------------------------------------------------------------------------
// Recipes
// ---------------------------------------------------------------------------

fn kv(map: &mut Map<String, Value>, k: &str, v: Value) {
    map.insert(k.to_string(), v);
}

/// Auto recipe for unvalidated families: CBR + bitrate only. OBS applies its
/// own defaults for everything else (this is the "automático" users asked
/// for: never write a quality key we cannot validate).
pub fn auto_recipe(bitrate_kbps: u32) -> Map<String, Value> {
    let mut m = Map::new();
    kv(&mut m, "bitrate", Value::from(bitrate_kbps));
    kv(&mut m, "rate_control", Value::from("CBR"));
    m
}

/// Measured NVENC recipe (validated live; old-MoonLit advanced table).
/// `gpu_index` 0 means auto (-1); N>0 selects device N.
pub fn measured_nvenc(bitrate_kbps: u32, gpu_index: u32, codec: &str) -> Map<String, Value> {
    let mut m = Map::new();
    kv(&mut m, "bitrate", Value::from(bitrate_kbps));
    kv(&mut m, "max_bitrate", Value::from(bitrate_kbps));
    kv(&mut m, "rate_control", Value::from("CBR"));
    kv(&mut m, "preset", Value::from("p5"));
    kv(&mut m, "tune", Value::from("hq"));
    kv(&mut m, "multipass", Value::from("disabled"));
    kv(&mut m, "bf", Value::from(2));
    kv(&mut m, "adaptive_quantization", Value::from(true));
    kv(&mut m, "lookahead", Value::from(false));
    kv(&mut m, "keyint_sec", Value::from(2));
    let device: i64 = if gpu_index == 0 { -1 } else { gpu_index as i64 };
    kv(&mut m, "device", Value::from(device));
    let profile = match codec {
        "hevc" | "av1" => "main",
        _ => "high",
    };
    kv(&mut m, "profile", Value::from(profile));
    m
}

/// Measured x264 recipe (deterministic software encoder).
pub fn measured_x264(bitrate_kbps: u32) -> Map<String, Value> {
    let mut m = Map::new();
    kv(&mut m, "bitrate", Value::from(bitrate_kbps));
    kv(&mut m, "rate_control", Value::from("CBR"));
    kv(&mut m, "preset", Value::from("veryfast"));
    kv(&mut m, "profile", Value::from("high"));
    kv(&mut m, "keyint_sec", Value::from(2));
    m
}

/// Ladder recipe: measured where validated, Auto where the hardware cannot
/// be tested (`validated=false` families + unknown ids never get a recipe).
pub fn ladder_recipe(
    encoder_id: &str,
    bitrate_kbps: u32,
    gpu_index: u32,
    codec: &str,
) -> Option<Map<String, Value>> {
    let family = family_of(encoder_id)?;
    if !family_validated(family) {
        return Some(auto_recipe(bitrate_kbps));
    }
    Some(match family {
        EncoderFamily::Nvenc => measured_nvenc(bitrate_kbps, gpu_index, codec),
        EncoderFamily::X264 => measured_x264(bitrate_kbps),
        _ => auto_recipe(bitrate_kbps),
    })
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

fn codec_str<'a>(
    pairs: &'a [(&'static str, &'static [&'static str])],
    codec: &str,
) -> Option<&'a [&'static str]> {
    pairs.iter().find(|(c, _)| *c == codec).map(|(_, v)| *v)
}

fn codec_ints(pairs: &[(&'static str, &'static [i64])], codec: &str) -> Option<Vec<i64>> {
    pairs
        .iter()
        .find(|(c, _)| *c == codec)
        .map(|(_, v)| v.to_vec())
}

/// One option resolved for a concrete codec: value lists, ranges and
/// defaults already scoped, plus whether the UI must grey it out.
#[derive(Debug, Clone)]
pub struct ResolvedOption {
    pub key: &'static str,
    pub kind: SpecKind,
    pub values: Vec<&'static str>,
    pub int_values: Vec<i64>,
    pub min: i64,
    pub max: i64,
    pub step: i64,
    pub default: Option<SpecDefault>,
    pub i18n: &'static str,
    pub visible: VisibleRule,
    /// False when the option does not exist for this codec (grey out +
    /// excluded from explicit settings + rejected by validate).
    pub supported: bool,
    pub p010_values: Vec<&'static str>,
    pub p010_ints: Vec<i64>,
}

/// Resolve every option of a family for one codec (single source of truth
/// for the UI schema, the sanitizer and the validator).
pub fn resolved_options(family: EncoderFamily, codec: &str) -> Vec<ResolvedOption> {
    options_for(family)
        .iter()
        .map(|s| {
            let supported = s.codecs.is_empty() || s.codecs.contains(&codec);
            let values: Vec<&'static str> = codec_str(s.codec_values, codec)
                .map(|v| v.to_vec())
                .unwrap_or_else(|| s.values.to_vec());
            let (mut min, mut max) = (s.min, s.max);
            for (c, lo, hi) in s.codec_range {
                if *c == codec {
                    min = *lo;
                    max = *hi;
                }
            }
            let int_values: Vec<i64> =
                codec_ints(s.codec_int_values, codec).unwrap_or_else(|| s.int_values.to_vec());
            let default = s
                .codec_defaults
                .iter()
                .find(|(c, _)| *c == codec)
                .map(|(_, d)| *d)
                .or(s.default);
            ResolvedOption {
                key: s.key,
                kind: s.kind,
                values,
                int_values,
                min,
                max,
                step: s.step,
                default,
                i18n: s.i18n,
                visible: s.visible,
                supported,
                p010_values: s.p010_values.to_vec(),
                p010_ints: s.p010_ints.to_vec(),
            }
        })
        .collect()
}

/// Validate Custom settings against the registry: unknown keys,
/// codec-invalid options/values and out-of-range numbers are rejected
/// (OBS would silently ignore a bad key and mis-encode on a bad value).
/// `color_format` gates 10-bit profiles when known (None = skip the gate,
/// e.g. single-payload parsing without video context). Returns the map.
pub fn validate_pair(
    family: EncoderFamily,
    codec: &str,
    settings: &Map<String, Value>,
    color_format: Option<&str>,
) -> Result<Map<String, Value>, String> {
    let resolved = resolved_options(family, codec);
    let find = |k: &str| resolved.iter().find(|r| r.key == k);
    let mut out = Map::new();
    for (k, v) in settings {
        let r = find(k).ok_or_else(|| format!("unknown option '{k}' for this encoder"))?;
        if !r.supported {
            return Err(format!(
                "option '{k}' not valid for {codec} on this encoder"
            ));
        }
        match r.kind {
            SpecKind::Enum => {
                let s = v
                    .as_str()
                    .ok_or_else(|| format!("option '{k}' must be text"))?;
                if !r.values.contains(&s) {
                    return Err(format!("invalid value '{s}' for '{k}'"));
                }
                if r.p010_values.contains(&s) && color_format.map(|c| c != "P010").unwrap_or(false)
                {
                    return Err(format!("profile '{s}' needs P010 color format (10-bit)"));
                }
                kv(&mut out, k, Value::from(s));
            }
            SpecKind::Int => {
                let n = v
                    .as_i64()
                    .ok_or_else(|| format!("option '{k}' must be an integer"))?;
                if !r.int_values.is_empty() {
                    if !r.int_values.contains(&n) {
                        return Err(format!("invalid value {n} for '{k}'"));
                    }
                } else if n < r.min || n > r.max {
                    return Err(format!(
                        "option '{k}' must be {}-{} (got {n})",
                        r.min, r.max
                    ));
                }
                if r.p010_ints.contains(&n) && color_format.map(|c| c != "P010").unwrap_or(false) {
                    return Err(format!("profile {n} needs P010 color format (10-bit)"));
                }
                kv(&mut out, k, Value::from(n));
            }
            SpecKind::Bool => {
                let b = v
                    .as_bool()
                    .ok_or_else(|| format!("option '{k}' must be true/false"))?;
                kv(&mut out, k, Value::from(b));
            }
            SpecKind::Text => {
                let s = v
                    .as_str()
                    .ok_or_else(|| format!("option '{k}' must be text"))?;
                kv(&mut out, k, Value::from(s));
            }
        }
    }
    Ok(out)
}

/// Validate without video context (no 10-bit gate); the pair check in
/// `validate_custom_pair` / build applies the color gate when known.
pub fn validate(
    family: EncoderFamily,
    codec: &str,
    settings: &Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    validate_pair(family, codec, settings, None)
}

// ---------------------------------------------------------------------------
// Persisted Custom payloads (DB JSON <-> validated structs)
// ---------------------------------------------------------------------------

/// Find an encoder id in either pinned catalog. Ids never collide across
/// platforms (AMF is Windows-only, VAAPI Linux-only, the rest share family
/// and codec), so family+codec resolve without any cfg.
pub fn lookup_encoder(id: &str) -> Option<EncoderEntry> {
    catalog_windows()
        .iter()
        .chain(catalog_linux())
        .find(|e| e.id == id)
        .copied()
}

/// Parse + validate `custom_encoder_json` (`{"encoder": id, "settings": {...}}`).
pub fn parse_custom_encoder(raw: &str) -> Result<CustomEncoder, String> {
    let v: Value =
        serde_json::from_str(raw).map_err(|e| format!("custom encoder is not valid JSON: {e}"))?;
    let encoder = v
        .get("encoder")
        .and_then(|s| s.as_str())
        .ok_or("custom encoder needs an 'encoder' id")?;
    let entry = lookup_encoder(encoder).ok_or_else(|| format!("unknown encoder '{encoder}'"))?;
    let settings = v
        .get("settings")
        .and_then(|s| s.as_object())
        .ok_or("custom encoder needs a 'settings' object")?;
    let clean = validate(entry.family, entry.codec, settings)?;
    Ok(CustomEncoder {
        encoder: encoder.to_string(),
        settings: clean,
    })
}

/// Canonical FPS values for the "common" picker (OBS UI list; NTSC rates
/// are covered by the fractional type).
pub const FPS_COMMON_VALUES: &[u32] = &[10, 20, 24, 25, 30, 48, 50, 60, 120, 144, 240];

fn canon(list: &[&str], v: &str) -> Option<String> {
    list.iter()
        .find(|s| s.eq_ignore_ascii_case(v))
        .map(|s| s.to_string())
}

fn even_clamp(v: u32) -> u32 {
    v.clamp(2, 8192) & !1
}

/// Parse + validate `custom_video_json` for the running family/codec.
pub fn parse_custom_video(
    family: EncoderFamily,
    codec: &str,
    raw: &str,
) -> Result<CustomVideo, String> {
    let v: Value =
        serde_json::from_str(raw).map_err(|e| format!("custom video is not valid JSON: {e}"))?;
    let get = |k: &str| v.get(k).ok_or_else(|| format!("custom video needs '{k}'"));
    let out_width = get("out_width")?
        .as_u64()
        .ok_or("out_width must be a number")? as u32;
    let out_height = get("out_height")?
        .as_u64()
        .ok_or("out_height must be a number")? as u32;
    let scale_type = get("scale_type")?
        .as_str()
        .ok_or("scale_type must be text")?;
    let fps_type = get("fps_type")?.as_str().ok_or("fps_type must be text")?;
    let color_format = get("color_format")?
        .as_str()
        .ok_or("color_format must be text")?;
    let color_space = get("color_space")?
        .as_str()
        .ok_or("color_space must be text")?;
    let color_range = get("color_range")?
        .as_str()
        .ok_or("color_range must be text")?;
    let num = |k: &str| -> Result<u32, String> {
        get(k)?
            .as_u64()
            .map(|n| n as u32)
            .ok_or_else(|| format!("{k} must be a number"))
    };

    if !(2..=8192).contains(&out_width) || !(2..=8192).contains(&out_height) {
        return Err("output resolution must be 2-8192 px".into());
    }
    let scale_type =
        canon(SCALE_FILTERS, scale_type).ok_or("unknown scale filter (use bicubic/lanczos/…)")?;
    let fps_type = canon(&["common", "integer", "fractional"], fps_type)
        .ok_or("fps_type must be common, integer or fractional")?;
    let color_format = color_format.to_uppercase();
    if !valid_color_formats(family, codec).contains(&color_format.as_str()) {
        return Err(format!(
            "color format {color_format} not valid for this encoder"
        ));
    }
    let color_space = canon(COLOR_SPACES, color_space).ok_or("unknown color space")?;
    let color_range =
        canon(COLOR_RANGES, color_range).ok_or("color range must be Partial or Full")?;

    let (fps_common, fps_int, fps_num, fps_den) = match fps_type.as_str() {
        "common" => {
            let f = num("fps_common")?;
            if !FPS_COMMON_VALUES.contains(&f) {
                return Err("fps_common must be a common OBS value (10/24/30/60/120/…)".into());
            }
            (f, 60, 60, 1)
        }
        "integer" => {
            let f = num("fps_int")?;
            if !(1..=360).contains(&f) {
                return Err("fps_int must be 1-360".into());
            }
            (f, f, f, 1)
        }
        _ => {
            let n = num("fps_num")?;
            let d = num("fps_den")?;
            if !(1..=1_000_000).contains(&n) || !(1..=1_000_000).contains(&d) {
                return Err("fps_num/fps_den must be 1-1000000".into());
            }
            ((n / d.max(1)).max(1), 60, n, d)
        }
    };

    Ok(CustomVideo {
        // OBS requires even dimensions; normalize deterministically.
        out_width: even_clamp(out_width),
        out_height: even_clamp(out_height),
        scale_type,
        fps_type,
        fps_common,
        fps_int,
        fps_num,
        fps_den,
        color_format,
        color_space,
        color_range,
    })
}

/// Effective whole fps of a validated CustomVideo (reporting + ring math).
pub fn custom_effective_fps(v: &CustomVideo) -> u32 {
    match v.fps_type.as_str() {
        "integer" => v.fps_int,
        "fractional" => (v.fps_num / v.fps_den.max(1)).max(1),
        _ => v.fps_common,
    }
    .clamp(1, 360)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn keys(m: &Map<String, Value>) -> Vec<&str> {
        let mut v: Vec<&str> = m.keys().map(|s| s.as_str()).collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn family_ids_cover_both_catalogs() {
        for e in catalog_windows().iter().chain(catalog_linux().iter()) {
            assert_eq!(
                family_of(e.id),
                Some(e.family),
                "catalog id '{}' must resolve to its family",
                e.id
            );
        }
        assert_eq!(family_of("obs_x264"), Some(EncoderFamily::X264));
        assert_eq!(family_of("game_capture"), None);
        assert_eq!(family_of(""), None);
    }

    #[test]
    fn catalog_has_no_duplicate_ids() {
        for catalog in [catalog_windows(), catalog_linux()] {
            let mut ids: Vec<&str> = catalog.iter().map(|e| e.id).collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), catalog.len());
        }
    }

    #[test]
    fn validated_families_are_nvenc_and_x264_only() {
        assert!(family_validated(EncoderFamily::Nvenc));
        assert!(family_validated(EncoderFamily::X264));
        assert!(!family_validated(EncoderFamily::Amf));
        assert!(!family_validated(EncoderFamily::Qsv));
        assert!(!family_validated(EncoderFamily::Vaapi));
    }

    #[test]
    fn nvenc_uses_obs32_keys_not_legacy() {
        // OBS 30+ renamed preset2/psycho_aq/gpu -> preset/adaptive_quantization/device.
        let all: Vec<&str> = options_for(EncoderFamily::Nvenc)
            .iter()
            .map(|s| s.key)
            .collect();
        for stale in ["preset2", "psycho_aq", "gpu"] {
            assert!(!all.contains(&stale), "stale key '{stale}' must not exist");
        }
        for required in [
            "rate_control",
            "bitrate",
            "max_bitrate",
            "target_quality",
            "cqp",
            "keyint_sec",
            "preset",
            "tune",
            "multipass",
            "profile",
            "lookahead",
            "adaptive_quantization",
            "device",
            "bf",
            "opts",
        ] {
            assert!(all.contains(&required), "missing NVENC key '{required}'");
        }
    }

    #[test]
    fn qsv_uses_target_usage_not_preset() {
        // OBS migrates legacy preset/async_depth itself; MoonClip writes TU keys.
        let all: Vec<&str> = options_for(EncoderFamily::Qsv)
            .iter()
            .map(|s| s.key)
            .collect();
        assert!(!all.contains(&"preset"), "QSV must not write 'preset'");
        assert!(
            !all.contains(&"async_depth"),
            "QSV must not write 'async_depth'"
        );
        for required in [
            "rate_control",
            "bitrate",
            "max_bitrate",
            "cqp",
            "icq_quality",
            "target_usage",
            "profile",
            "keyint_sec",
            "latency",
            "bframes",
        ] {
            assert!(all.contains(&required), "missing QSV key '{required}'");
        }
    }

    #[test]
    fn auto_recipe_is_minimal_and_cross_family_safe() {
        let auto = auto_recipe(20000);
        assert_eq!(keys(&auto), ["bitrate", "rate_control"]);
        assert_eq!(auto["rate_control"], json!("CBR"));
        // Every family must accept these keys (validate against all).
        for family in [
            EncoderFamily::Nvenc,
            EncoderFamily::X264,
            EncoderFamily::Qsv,
            EncoderFamily::Amf,
            EncoderFamily::Vaapi,
        ] {
            validate(family, "h264", &auto).expect("auto recipe must validate everywhere");
        }
    }

    #[test]
    fn ladder_recipes_match_validation_state() {
        // Measured families get full recipes.
        let nv = ladder_recipe("obs_nvenc_h264_tex", 20000, 0, "h264").unwrap();
        assert!(keys(&nv).contains(&"preset"));
        assert!(keys(&nv).contains(&"adaptive_quantization"));
        assert_eq!(nv["device"], json!(-1)); // gpu_index 0 = auto
        let nv2 = ladder_recipe("obs_nvenc_h264_tex", 20000, 2, "h264").unwrap();
        assert_eq!(nv2["device"], json!(2));
        let hevc = ladder_recipe("obs_nvenc_hevc_tex", 12000, 0, "hevc").unwrap();
        assert_eq!(hevc["profile"], json!("main"));
        let x = ladder_recipe("obs_x264", 20000, 0, "h264").unwrap();
        assert_eq!(x["preset"], json!("veryfast"));
        // Unvalidated families get the Auto recipe only.
        for id in [
            "h264_texture_amf",
            "h265_texture_amf",
            "obs_qsv11_v2",
            "obs_qsv11_hevc",
            "ffmpeg_vaapi",
        ] {
            let auto = ladder_recipe(id, 20000, 0, "h264").unwrap();
            assert_eq!(keys(&auto), ["bitrate", "rate_control"], "{id}");
        }
        // Unknown encoder ids fail loudly, never silently.
        assert!(ladder_recipe("ffmpeg_nvenc", 20000, 0, "h264").is_none());
    }

    #[test]
    fn validate_accepts_documented_values() {
        let mut m = Map::new();
        kv(&mut m, "rate_control", json!("CBR"));
        kv(&mut m, "preset", json!("p7"));
        kv(&mut m, "tune", json!("ll"));
        kv(&mut m, "profile", json!("high"));
        kv(&mut m, "bf", json!(4));
        kv(&mut m, "device", json!(-1));
        kv(&mut m, "lookahead", json!(true));
        kv(&mut m, "opts", json!("keyint=120"));
        let out = validate(EncoderFamily::Nvenc, "h264", &m).unwrap();
        assert_eq!(out["preset"], json!("p7"));

        let mut q = Map::new();
        kv(&mut q, "rate_control", json!("CQP"));
        kv(&mut q, "cqp", json!(23));
        kv(&mut q, "target_usage", json!("TU1"));
        kv(&mut q, "latency", json!("ultra-low"));
        validate(EncoderFamily::Qsv, "h264", &q).unwrap();

        let mut x = Map::new();
        kv(&mut x, "rate_control", json!("CRF"));
        kv(&mut x, "crf", json!(20));
        kv(&mut x, "preset", json!("medium"));
        kv(&mut x, "tune", json!("zerolatency"));
        kv(&mut x, "x264opts", json!("keyint=120:min-keyint=60"));
        validate(EncoderFamily::X264, "h264", &x).unwrap();
    }

    #[test]
    fn validate_rejects_unknown_keys_and_values() {
        let mut m = Map::new();
        kv(&mut m, "preset2", json!("p7")); // stale OBS 29 key
        assert!(validate(EncoderFamily::Nvenc, "h264", &m).is_err());

        let mut m = Map::new();
        kv(&mut m, "preset", json!("p9"));
        assert!(validate(EncoderFamily::Nvenc, "h264", &m).is_err());

        let mut m = Map::new();
        kv(&mut m, "bitrate", json!(10)); // below min 50
        assert!(validate(EncoderFamily::Nvenc, "h264", &m).is_err());

        let mut m = Map::new();
        kv(&mut m, "lookahead", json!("yes")); // bool, not text
        assert!(validate(EncoderFamily::Nvenc, "h264", &m).is_err());

        let mut m = Map::new();
        kv(&mut m, "profile", json!("main10")); // hevc-only on NVENC
        assert!(validate(EncoderFamily::Nvenc, "h264", &m).is_err());

        let mut m = Map::new();
        kv(&mut m, "target_usage", json!("medium")); // legacy string
        assert!(validate(EncoderFamily::Qsv, "h264", &m).is_err());
    }

    #[test]
    fn resolver_scopes_options_per_codec() {
        // NVENC profile lists and defaults follow the codec.
        let p = resolved_options(EncoderFamily::Nvenc, "hevc")
            .into_iter()
            .find(|r| r.key == "profile")
            .unwrap();
        assert!(p.supported);
        assert_eq!(p.values, vec!["main10", "main"]);
        assert_eq!(p.default, Some(SpecDefault::Str("main")));
        assert_eq!(p.p010_values, vec!["high10", "main10"]);
        let p = resolved_options(EncoderFamily::Nvenc, "av1")
            .into_iter()
            .find(|r| r.key == "profile")
            .unwrap();
        assert_eq!(p.values, vec!["main"]);
        // NVENC cqp range is 1-51 except AV1 (1-63).
        let cqp = |codec: &str| {
            resolved_options(EncoderFamily::Nvenc, codec)
                .into_iter()
                .find(|r| r.key == "cqp")
                .unwrap()
        };
        assert_eq!((cqp("h264").min, cqp("h264").max), (1, 51));
        assert_eq!((cqp("av1").min, cqp("av1").max), (1, 63));
        // AMF bf exists only for H.264/AV1 (0-5); HEVC has no bf key.
        let bf = |codec: &str| {
            resolved_options(EncoderFamily::Amf, codec)
                .into_iter()
                .find(|r| r.key == "bf")
                .unwrap()
        };
        assert!(bf("h264").supported);
        assert!(bf("av1").supported);
        assert_eq!((bf("h264").min, bf("h264").max), (0, 5));
        assert!(!bf("hevc").supported);
        // AMF profile: no list at all on HEVC.
        let p = resolved_options(EncoderFamily::Amf, "hevc")
            .into_iter()
            .find(|r| r.key == "profile")
            .unwrap();
        assert!(!p.supported);
        // AMF has no vbaq/enforce_hrd/params keys (hardcoded in OBS);
        // the free-text field is ffmpeg_opts.
        let keys: Vec<&str> = resolved_options(EncoderFamily::Amf, "h264")
            .iter()
            .map(|r| r.key)
            .collect();
        for stale in ["vbaq", "enforce_hrd", "params", "preanalysis"] {
            assert!(!keys.contains(&stale), "stale AMF key '{stale}'");
        }
        assert!(keys.contains(&"ffmpeg_opts"));
        assert!(keys.contains(&"pre_analysis"));
        // VAAPI level lists are per codec.
        let lvl = |codec: &str| {
            resolved_options(EncoderFamily::Vaapi, codec)
                .into_iter()
                .find(|r| r.key == "level")
                .unwrap()
        };
        assert!(lvl("h264").int_values.contains(&52));
        assert!(!lvl("h264").int_values.contains(&4));
        assert!(lvl("av1").int_values.contains(&4));
        assert!(!lvl("av1").int_values.contains(&52));
        // Codec-scoped defaults for un-Auto seeding.
        fn default_of(family: EncoderFamily, codec: &str, key: &str) -> Option<SpecDefault> {
            resolved_options(family, codec)
                .into_iter()
                .find(|r| r.key == key)
                .and_then(|r| r.default)
        }
        assert_eq!(
            default_of(EncoderFamily::Nvenc, "hevc", "profile"),
            Some(SpecDefault::Str("main"))
        );
        assert_eq!(
            default_of(EncoderFamily::Qsv, "av1", "profile"),
            Some(SpecDefault::Str("main"))
        );
        assert_eq!(
            default_of(EncoderFamily::Amf, "av1", "preset"),
            Some(SpecDefault::Str("highQuality"))
        );
        assert_eq!(
            default_of(EncoderFamily::Nvenc, "h264", "profile"),
            Some(SpecDefault::Str("high"))
        );
    }

    #[test]
    fn validate_pair_enforces_codec_scope_and_p010() {
        // cqp 60 is AV1-only on NVENC.
        let mut m = Map::new();
        kv(&mut m, "cqp", json!(60));
        assert!(validate_pair(EncoderFamily::Nvenc, "h264", &m, None).is_err());
        assert!(validate_pair(EncoderFamily::Nvenc, "av1", &m, None).is_ok());
        // 10-bit profiles need P010.
        let mut m = Map::new();
        kv(&mut m, "profile", json!("main10"));
        assert!(validate_pair(EncoderFamily::Nvenc, "hevc", &m, Some("NV12")).is_err());
        assert!(validate_pair(EncoderFamily::Nvenc, "hevc", &m, Some("P010")).is_ok());
        assert!(validate_pair(EncoderFamily::Nvenc, "hevc", &m, None).is_ok());
        // AMF bf on HEVC is rejected (no such OBS key).
        let mut m = Map::new();
        kv(&mut m, "bf", json!(2));
        assert!(validate_pair(EncoderFamily::Amf, "hevc", &m, None).is_err());
        assert!(validate_pair(EncoderFamily::Amf, "h264", &m, None).is_ok());
        // AMF preset highQuality is AV1-only.
        let mut m = Map::new();
        kv(&mut m, "preset", json!("highQuality"));
        assert!(validate_pair(EncoderFamily::Amf, "h264", &m, None).is_err());
        assert!(validate_pair(EncoderFamily::Amf, "av1", &m, None).is_ok());
        // VAAPI HEVC level 4 (an AV1 level) is rejected.
        let mut m = Map::new();
        kv(&mut m, "level", json!(4));
        assert!(validate_pair(EncoderFamily::Vaapi, "hevc", &m, None).is_err());
        assert!(validate_pair(EncoderFamily::Vaapi, "av1", &m, None).is_ok());
    }

    #[test]
    fn profile_values_are_codec_scoped() {
        fn profile(family: EncoderFamily, codec: &str) -> Vec<&'static str> {
            resolved_options(family, codec)
                .into_iter()
                .find(|r| r.key == "profile")
                .map(|r| r.values)
                .unwrap()
        }
        assert!(profile(EncoderFamily::Nvenc, "h264").contains(&"high"));
        assert!(!profile(EncoderFamily::Nvenc, "h264").contains(&"main10"));
        assert_eq!(profile(EncoderFamily::Nvenc, "av1"), vec!["main"]);
        assert_eq!(profile(EncoderFamily::Qsv, "hevc"), vec!["main", "main10"]);
        assert!(profile(EncoderFamily::Amf, "h264").contains(&"baseline"));
        assert!(!profile(EncoderFamily::Amf, "h264").contains(&"constrained_baseline"));
        assert_eq!(profile(EncoderFamily::Amf, "av1"), vec!["main"]);
        // VAAPI profiles resolve to FFmpeg ABI ints per codec.
        fn profile_ints(codec: &str) -> Vec<i64> {
            resolved_options(EncoderFamily::Vaapi, codec)
                .into_iter()
                .find(|r| r.key == "profile")
                .map(|r| r.int_values)
                .unwrap()
        }
        assert_eq!(profile_ints("h264"), vec![578, 77, 100]);
        assert_eq!(profile_ints("hevc"), vec![1, 2]);
        assert_eq!(profile_ints("av1"), vec![0]);
    }

    #[test]
    fn color_formats_respect_encoder_limits() {
        // x264 + QSV-H264 reject high-precision formats (source-verified).
        assert!(!valid_color_formats(EncoderFamily::X264, "h264").contains(&"P010"));
        assert_eq!(valid_color_formats(EncoderFamily::Qsv, "h264"), &["NV12"]);
        assert!(valid_color_formats(EncoderFamily::Qsv, "hevc").contains(&"P010"));
        assert!(valid_color_formats(EncoderFamily::Nvenc, "h264").contains(&"NV12"));
    }

    #[test]
    fn every_spec_has_i18n_key_and_unique_key() {
        for family in [
            EncoderFamily::Nvenc,
            EncoderFamily::X264,
            EncoderFamily::Qsv,
            EncoderFamily::Amf,
            EncoderFamily::Vaapi,
        ] {
            let opts = options_for(family);
            assert!(!opts.is_empty());
            let mut seen = Vec::new();
            for s in opts {
                assert!(s.i18n.starts_with("obsopt."), "{}", s.key);
                assert!(!seen.contains(&s.key), "dup {}", s.key);
                seen.push(s.key);
                if matches!(s.kind, SpecKind::Enum) {
                    assert!(!s.values.is_empty(), "{}", s.key);
                    if let Some(SpecDefault::Str(d)) = s.default {
                        assert!(s.values.contains(&d), "{} default {d}", s.key);
                    }
                }
                if matches!(s.kind, SpecKind::Int) && s.int_values.is_empty() {
                    assert!(s.min <= s.max, "{}", s.key);
                }
                // Codec tables must be self-consistent: every codec entry has
                // values, and every codec default is inside its codec list.
                for (c, vs) in s.codec_values {
                    assert!(!vs.is_empty(), "{} {c}", s.key);
                }
                for (c, d) in s.codec_defaults {
                    if let SpecDefault::Str(d) = d {
                        let allowed = codec_str(s.codec_values, c).unwrap_or(s.values);
                        assert!(allowed.contains(d), "{} {c} default {d} not in list", s.key);
                    }
                }
                for c in s.codecs {
                    assert!(
                        ["h264", "hevc", "av1"].contains(c),
                        "{} unknown codec {c}",
                        s.key
                    );
                }
            }
        }
    }

    #[test]
    fn vaapi_profile_ints_match_ffmpeg_abi() {
        // AV_PROFILE_H264_{CONSTRAINED_BASELINE=578, MAIN=77, HIGH=100},
        // HEVC {MAIN=1, MAIN_10=2}, AV1 {MAIN=0} — via the resolver.
        fn profile_ints(codec: &str) -> Vec<i64> {
            resolved_options(EncoderFamily::Vaapi, codec)
                .into_iter()
                .find(|r| r.key == "profile")
                .map(|r| r.int_values)
                .unwrap()
        }
        assert_eq!(profile_ints("h264"), vec![578, 77, 100]);
        assert_eq!(profile_ints("hevc"), vec![1, 2]);
        assert_eq!(profile_ints("av1"), vec![0]);
        let mut m = Map::new();
        kv(&mut m, "profile", json!(77));
        validate(EncoderFamily::Vaapi, "h264", &m).unwrap();
        let mut m = Map::new();
        kv(&mut m, "profile", json!(100));
        assert!(validate(EncoderFamily::Vaapi, "hevc", &m).is_err());
    }

    #[test]
    fn custom_encoder_payload_roundtrip() {
        let raw = r#"{"encoder":"obs_nvenc_h264_tex","settings":{"rate_control":"CQP","cqp":20,"preset":"p7"}}"#;
        let c = parse_custom_encoder(raw).unwrap();
        assert_eq!(c.encoder, "obs_nvenc_h264_tex");
        assert_eq!(c.settings["cqp"], json!(20));
        // Unknown encoder id fails loudly.
        assert!(parse_custom_encoder(r#"{"encoder":"nope","settings":{}}"#).is_err());
        // Stale key inside fails loudly.
        assert!(parse_custom_encoder(
            r#"{"encoder":"obs_nvenc_h264_tex","settings":{"preset2":"p7"}}"#
        )
        .is_err());
        // Not JSON fails loudly.
        assert!(parse_custom_encoder("junk").is_err());
    }

    #[test]
    fn custom_video_payload_validates() {
        let raw = r#"{"out_width":2560,"out_height":1440,"scale_type":"lanczos","fps_type":"common","fps_common":120,"fps_int":60,"fps_num":60000,"fps_den":1001,"color_format":"NV12","color_space":"709","color_range":"Partial"}"#;
        let v = parse_custom_video(EncoderFamily::Nvenc, "h264", raw).unwrap();
        assert_eq!((v.out_width, v.out_height), (2560, 1440));
        assert_eq!(custom_effective_fps(&v), 120);
        // Odd dimensions normalize to even, deterministically.
        let odd = raw.replace("\"out_width\":2560", "\"out_width\":2559");
        let v = parse_custom_video(EncoderFamily::Nvenc, "h264", &odd).unwrap();
        assert_eq!(v.out_width, 2558);
        // Case-insensitive enums canonicalize.
        let ci = raw
            .replace("\"scale_type\":\"lanczos\"", "\"scale_type\":\"Lanczos\"")
            .replace("\"color_range\":\"Partial\"", "\"color_range\":\"partial\"");
        let v = parse_custom_video(EncoderFamily::Nvenc, "h264", &ci).unwrap();
        assert_eq!(v.scale_type, "lanczos");
        assert_eq!(v.color_range, "Partial");
        // Fractional fps reports the quotient.
        let frac = raw.replace("\"fps_type\":\"common\"", "\"fps_type\":\"fractional\"");
        let v = parse_custom_video(EncoderFamily::Nvenc, "h264", &frac).unwrap();
        assert_eq!(custom_effective_fps(&v), 59);
        // x264 rejects P010.
        let p010 = raw.replace("\"color_format\":\"NV12\"", "\"color_format\":\"P010\"");
        assert!(parse_custom_video(EncoderFamily::X264, "h264", &p010).is_err());
        // QSV H.264 rejects I444.
        let i444 = raw.replace("\"color_format\":\"NV12\"", "\"color_format\":\"I444\"");
        assert!(parse_custom_video(EncoderFamily::Qsv, "h264", &i444).is_err());
        // Unknown filter / out-of-range fps fail.
        let bad = raw.replace("\"scale_type\":\"lanczos\"", "\"scale_type\":\"spline\"");
        assert!(parse_custom_video(EncoderFamily::Nvenc, "h264", &bad).is_err());
        let bad = raw.replace("\"fps_common\":120", "\"fps_common\":90");
        assert!(parse_custom_video(EncoderFamily::Nvenc, "h264", &bad).is_err());
    }
}
