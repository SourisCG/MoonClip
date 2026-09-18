//! Audio DSP for the Windows engine: conversion, gain, mix, peaks and the
//! QPC-grid window builder. Pure and unit-tested; the WASAPI plumbing lives in
//! `audio.rs`.
//!
//! Rules (why this file exists):
//! - Gains land in OUR path (atomics read by the capture thread), never the OS
//!   mixer.
//! - `build_window` resamples each 10 ms block by its timestamp, so device
//!   clock drift (a few hundred ppm) cannot shift A/V over long buffers, and
//!   only real gaps (> FUSE) become silence — sub-ms jitter is absorbed, never
//!   materialized (that caused the old static).
//! - The Master stem is the clamped sum of the solo stems.

use std::collections::VecDeque;

pub const STEM_RATE: u32 = 48_000;
pub const STEM_CHANNELS: usize = 2;
/// Sub-50 ms gaps inside a stem are timestamp jitter (WASAPI block grid vs
/// sample count), not lost audio: the resampler bridges them. Larger gaps
/// are real and become silence.
pub const FUSE_NS: i128 = 50_000_000;
/// Block size handed to the ring (10 ms stereo).
pub const BLOCK_FRAMES: usize = 480;

/// Exactly `frames` stereo frames for a window duration in ns.
pub fn window_frames(window_ns: i128) -> usize {
    (window_ns.max(0) * STEM_RATE as i128 / 1_000_000_000) as usize
}

#[derive(Debug, Clone)]
pub struct Block {
    /// QPC of the block's first sample.
    pub qpc_ns: i128,
    /// Interleaved stereo i16.
    pub samples: Vec<i16>,
}

impl Block {
    pub fn frames(&self) -> usize {
        self.samples.len() / STEM_CHANNELS
    }

    pub fn end_ns(&self) -> i128 {
        self.qpc_ns + self.frames() as i128 * 1_000_000_000 / STEM_RATE as i128
    }
}

/// Rebuild `frames` stereo frames of a stem on the QPC grid starting at
/// `start_ns`, using each block's timestamp to resample (drift correction)
/// and inserting silence only for real gaps. Pure and unit-tested.
pub fn build_window(blocks: &VecDeque<Block>, start_ns: i128, frames: usize) -> Vec<i16> {
    let mut out = vec![0i16; frames * STEM_CHANNELS];
    if frames == 0 || blocks.is_empty() {
        return out;
    }
    // Flatten content for fractional lookups + per-block geometry. All times
    // fit i64 (QPC nanoseconds); i64 keeps the debug-build hot loop fast.
    let mut content: Vec<i16> = Vec::with_capacity(blocks.iter().map(|b| b.samples.len()).sum());
    let mut geo: Vec<(i64, i64, usize)> = Vec::with_capacity(blocks.len());
    for b in blocks {
        let c_start = content.len() / STEM_CHANNELS;
        let q_start = b.qpc_ns as i64;
        content.extend_from_slice(&b.samples);
        geo.push((q_start, b.end_ns() as i64, c_start));
    }
    let content_frames = content.len() / STEM_CHANNELS;
    if content_frames == 0 {
        return out;
    }
    let fuse = FUSE_NS as i64;
    // Output time advances by exactly 1e9/48000 ns = 20833 + 1/3: keep the
    // accumulator rational so the hot loop has NO division (debug builds were
    // spending seconds here on a 120 s window).
    let mut t = start_ns as i64;
    let mut rem = 0i64;
    let mut cursor = 0usize;
    for f in 0..frames {
        // Cursor = last block that already started at `t`.
        while cursor + 1 < geo.len() && t >= geo[cursor + 1].0 {
            cursor += 1;
        }
        let (q_start, q_end, c_start) = geo[cursor];
        let n_frames = (geo
            .get(cursor + 1)
            .map(|(_, _, next_c)| (*next_c).min(content_frames) - c_start)
            .unwrap_or(content_frames - c_start))
        .max(1);
        // Big gap: more than FUSE before the current block, or more than FUSE
        // after its end without a following block.
        if t + fuse < q_start {
            advance(&mut t, &mut rem);
            continue; // silence
        }
        if t > q_end + fuse && cursor + 1 == geo.len() {
            advance(&mut t, &mut rem);
            continue; // silence
        }
        if t > q_end + fuse && t < geo[cursor + 1].0 {
            advance(&mut t, &mut rem);
            continue; // silence between two distant blocks
        }
        // Map within the current block (drift-corrected); hold the last
        // sample while waiting for the next block (tiny jitter, no clicks).
        let frame_f = c_start as i64 + (t - q_start) * 48 / 1_000_000;
        let lo = c_start as i64;
        let hi = (c_start + n_frames - 1) as i64;
        let clamped = frame_f.clamp(lo, hi);
        let fi = clamped as usize;
        let ff = (clamped - fi as i64) as f64;
        let base = fi * STEM_CHANNELS;
        for ch in 0..STEM_CHANNELS {
            let a = content[base + ch] as f64;
            let b = if fi < (c_start + n_frames - 1) {
                content[base + STEM_CHANNELS + ch] as f64
            } else {
                a
            };
            out[f * STEM_CHANNELS + ch] = (a + (b - a) * ff).round() as i16;
        }
        advance(&mut t, &mut rem);
    }
    out
}

/// Advance the rational output clock by exactly 1e9/48000 ns (20833 + 1/3).
#[inline]
fn advance(t: &mut i64, rem: &mut i64) {
    *t += 20_833;
    *rem += 1;
    if *rem == 3 {
        *rem = 0;
        *t += 1;
    }
}

/// Convert interleaved f32 samples to stereo i16 at 48 kHz with gain.
/// The WASAPI client is initialized with `autoconvert`, so the engine hands
/// us f32 48 kHz stereo regardless of the endpoint's native format.
pub fn convert_f32(data: &[f32], gain: f32, out: &mut Vec<i16>) {
    out.clear();
    out.reserve(data.len());
    for &s in data {
        out.push((s * gain).clamp(-1.0, 1.0).mul_add(32767.0, 0.0) as i16);
    }
}

/// Peak level (linear 0.0-1.0+) of an i16 buffer.
pub fn peak_i16(samples: &[i16]) -> f32 {
    let mut peak = 0f32;
    for &s in samples {
        let v = (s as f32 / 32768.0).abs();
        if v > peak {
            peak = v;
        }
    }
    peak
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring_from(spec: &[(i128, i16)]) -> VecDeque<Block> {
        let mut r = VecDeque::new();
        let mut qpc = spec.first().map(|(q, _)| *q).unwrap_or(0);
        for &(dt, v) in spec {
            qpc += dt;
            let samples: Vec<i16> = std::iter::repeat(v).take(BLOCK_FRAMES * 2).collect();
            r.push_back(Block { qpc_ns: qpc, samples });
        }
        r
    }

    #[test]
    fn window_perfect_timestamps_are_verbatim() {
        let blocks = ring_from(&[
            (0, 100),
            (BLOCK_FRAMES as i128 * 1_000_000_000 / STEM_RATE as i128, 200),
        ]);
        let start = blocks.front().unwrap().qpc_ns;
        let frames = BLOCK_FRAMES * 2;
        let out = build_window(&blocks, start, frames);
        assert_eq!(out.len(), frames * 2);
        assert!(out[..BLOCK_FRAMES * 2].iter().all(|&s| s == 100));
        assert!(out[BLOCK_FRAMES * 2..].iter().all(|&s| s == 200));
    }

    #[test]
    fn window_corrects_drift_without_clicks() {
        // Second block starts 1% later than its content duration: the builder
        // must stretch (interpolate), never insert silence.
        let step = BLOCK_FRAMES as i128 * 1_000_000_000 / STEM_RATE as i128;
        let blocks = ring_from(&[(0, 0), (step + step / 100, 10000)]);
        let start = blocks.front().unwrap().qpc_ns;
        let frames = BLOCK_FRAMES * 2;
        let out = build_window(&blocks, start, frames);
        let mid = BLOCK_FRAMES + 10;
        assert!(
            out[mid * 2] > 0,
            "expected the second block after the jittered boundary"
        );
    }

    #[test]
    fn window_real_gap_becomes_silence() {
        let step = BLOCK_FRAMES as i128 * 1_000_000_000 / STEM_RATE as i128;
        // 5 s gap between two blocks (> FUSE).
        let blocks = ring_from(&[(0, 500), (step + 5_000_000_000, 900)]);
        let start = blocks.front().unwrap().qpc_ns;
        let frames = BLOCK_FRAMES + 4800; // 110 ms: past q_end + FUSE
        let out = build_window(&blocks, start, frames);
        let late = frames - 1;
        assert_eq!(out[late * 2], 0, "gap must be silent");
        assert_eq!(out[10], 500);
    }

    #[test]
    fn window_pads_when_ring_is_short() {
        let blocks = ring_from(&[(0, 7)]);
        let start = blocks.front().unwrap().qpc_ns;
        let frames = BLOCK_FRAMES;
        let out = build_window(&blocks, start, frames);
        assert_eq!(out.len(), frames * 2);
        assert!(out.iter().all(|&s| s == 7));
    }

    #[test]
    fn conversion_handles_silence_and_gain() {
        let mut out = Vec::new();
        convert_f32(&[0.5f32, -0.5, 0.25, -0.25], 2.0, &mut out);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0], 32767); // 1.0 clamped
        assert_eq!(out[1], -32767);
        assert_eq!(out[2], 16383);
        convert_f32(&[], 1.0, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn peak_level() {
        assert!((peak_i16(&[i16::MAX, 0]) - 1.0).abs() < 0.001);
        assert_eq!(peak_i16(&[]), 0.0);
    }

    #[test]
    fn window_frames_is_seconds_times_rate() {
        assert_eq!(window_frames(1_000_000_000), 48_000);
        assert_eq!(window_frames(500_000_000), 24_000);
        assert_eq!(window_frames(-5), 0);
    }
}
