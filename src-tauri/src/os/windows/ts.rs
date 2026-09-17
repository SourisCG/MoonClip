//! MPEG-TS ring with a video PES index (PTS + keyframes) and QPC calibration.
//!
//! The Windows engine no longer paces frames itself: ffmpeg captures (WGC via
//! `gfxcapture`), encodes and muxes to MPEG-TS on stdout with REAL timestamps.
//! This module is the Rust-side brain of that ring:
//!
//! - it parses PAT/PMT to find the video PID,
//! - indexes every video PES with its PTS (90 kHz -> ns) and whether it is a
//!   keyframe (IDR for H.264, IRAP for HEVC, scanned from the first bytes of
//!   the PES payload),
//! - calibrates the PTS clock against the local QPC clock continuously
//!   (`min(qpc_at_arrival - pts)`), so save-time audio windows can be mapped
//!   to video PTS with a few milliseconds of error and no per-codec constants.
//!
//! All parsing is pure and unit-tested with synthetic transport streams.

use std::collections::VecDeque;
use std::sync::Arc;

/// MPEG-TS packet size.
pub const TS_PACKET_LEN: usize = 188;
/// Ring chunk size: fixed blocks keep trimming O(1) (see `TsRing`).
const CHUNK_BYTES: usize = 1024 * 1024;
/// Bytes of PES payload scanned for NAL start codes (keyframe detection).
const PES_SCAN_LIMIT: usize = 4096;
/// 90 kHz PTS ticks per second.
const PTS_HZ: i128 = 90_000;
/// 33-bit PTS wrap period.
const PTS_WRAP: i64 = 1 << 33;
/// Extra bytes kept beyond the nominal ring capacity before trimming.
const RING_SLACK: usize = 1024 * 1024;
/// Window for the capture-rate telemetry (showinfo samples kept).
const CAPTURE_WINDOW_NS: i128 = 10_000_000_000;
/// How far before the chosen keyframe the staged cut starts (demuxer context:
/// PAT/PMT + SPS/PPS). `-ss` then starts the output exactly at the keyframe.
pub const CUT_PREROLL_NS: i64 = 2_000_000_000;

/// Default video-anchor bias in ms: the WGC/DDA frame reaches ffmpeg's filter
/// ~70 ms after its presentation timestamp (compositor + frame pool), measured
/// on this rig with the flash+beep reference (`live_av_offset_capture` +
/// `analyze_av.py`, both `gfxcapture` and `ddagrab` agree). Re-measure with
/// `MOONCLIP_SYNC_BIAS_MS=<ms>` when the compositor/driver stack changes.
pub const DEFAULT_SYNC_BIAS_MS: i64 = 70;

fn default_sync_bias_ns() -> i128 {
    let ms = std::env::var("MOONCLIP_SYNC_BIAS_MS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(DEFAULT_SYNC_BIAS_MS);
    ms as i128 * 1_000_000
}

/// One indexed video frame (PES packet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameEntry {
    /// Presentation time in nanoseconds (unwrapped 90 kHz PTS).
    pub pts_ns: i64,
    /// True for H.264 IDR / HEVC IRAP. `false` also means "unknown" for
    /// codecs without a scanner (AV1) — the engine falls back to an ffmpeg
    /// seek probe in that case.
    pub key: bool,
    /// Absolute byte offset (since engine start) of the PES's first TS packet.
    pub offset: u64,
}

/// Selected slice of the ring ready to stage and mux.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // one-shot wrapper kept for tests; the engine uses CutPlan
pub struct Cut {
    /// TS bytes to write to the staging file (pre-roll included).
    pub bytes: Vec<u8>,
    /// `-ss` seconds for the video input: exactly the chosen keyframe.
    pub ss_secs: f64,
    /// Span the audio stem must cover, in ns, starting at the keyframe:
    /// `(end - key) + one frame` so both streams end together.
    pub window_ns: i128,
    /// QPC instant of the keyframe (`calib + key_pts_ns`).
    pub qpc_start_ns: i128,
    /// PTS of the chosen keyframe.
    pub key_pts_ns: i64,
    /// PTS of the last (display-order) frame in the ring.
    pub end_pts_ns: i64,
}

/// Selected clip whose bytes are still referenced from the ring: the engine
/// snapshots a `CutPlan` under the ring lock (cheap `Arc` clones) and calls
/// [`CutPlan::assemble`] afterwards, so the encoder pipe never stalls behind
/// the ~300 MB save copy.
#[derive(Debug, Clone)]
pub struct CutPlan {
    /// Latest PAT/PMT, prepended so the staged TS starts exactly at the
    /// keyframe (only set on the no-seek path).
    pat: Option<Vec<u8>>,
    pmt: Option<Vec<u8>>,
    /// Chunk references to copy, in order.
    chunks: VecDeque<Arc<Vec<u8>>>,
    /// Byte offset inside the first referenced chunk.
    take_from: usize,
    /// Payload bytes in `chunks` (capacity hint for `assemble`).
    total_hint: usize,
    /// `-ss` seconds for the video input: exactly the chosen keyframe (0 when
    /// the stage already starts there).
    pub ss_secs: f64,
    /// Span the audio stem must cover, in ns, starting at the keyframe.
    pub window_ns: i128,
    /// QPC instant of the keyframe (`calib + key_pts_ns`).
    pub qpc_start_ns: i128,
    /// PTS of the chosen keyframe.
    pub key_pts_ns: i64,
    /// PTS of the last (display-order) frame in the ring.
    pub end_pts_ns: i64,
}

impl CutPlan {
    /// Materialize the staged TS. This is the expensive copy; it runs outside
    /// the ring lock and is safe even if the ring trimmed those chunks since
    /// the snapshot (`Arc` keeps them alive).
    pub fn assemble(&self) -> Vec<u8> {
        let extra = self.pat.as_ref().map_or(0, Vec::len) + self.pmt.as_ref().map_or(0, Vec::len);
        let mut out = Vec::with_capacity(self.total_hint + extra);
        if let Some(p) = &self.pat {
            out.extend_from_slice(p);
        }
        if let Some(p) = &self.pmt {
            out.extend_from_slice(p);
        }
        for (i, chunk) in self.chunks.iter().enumerate() {
            let start = if i == 0 { self.take_from } else { 0 };
            if start < chunk.len() {
                out.extend_from_slice(&chunk[start..]);
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Codec {
    H264,
    Hevc,
    Other,
}

#[derive(Default)]
struct PesState {
    active: bool,
    offset: u64,
    qpc_ns: i128,
    pts_ns: Option<i64>,
    key: bool,
    scan_done: bool,
    scan: Vec<u8>,
}

/// Streaming MPEG-TS parser driven by a byte ring.
///
/// Storage is a queue of fixed-size chunks and trimming pops whole chunks
/// from the front: **O(1) with no giant memmoves under the mutex**. The
/// previous contiguous `Vec` had to `drain` ~1 MB every ~0.4 s at a 120 s
/// buffer, which memmoved the remaining ~326 MB (measured 19-60 ms) and
/// stalled the encoder pipe in bursts.
pub struct TsRing {
    /// Fixed-width chunks (`CHUNK_BYTES`; the tail may be partial). `Arc` so a
    /// save can snapshot the ring (cheap handle clones) and copy the bytes
    /// after releasing the lock: trimming/pushing keeps working meanwhile and
    /// popped chunks stay alive behind the snapshot.
    chunks: VecDeque<Arc<Vec<u8>>>,
    /// Absolute offset (bytes captured since start) of `chunks[0][0]`.
    base: u64,
    /// Total bytes stored across all chunks.
    len: usize,
    /// Total bytes appended since start (`base + len`).
    total: u64,
    /// Absolute offset of the next unparsed byte.
    parse_abs: u64,
    /// Cursor: `parse_abs` lives at `chunks[parse_chunk][parse_off]`.
    parse_chunk: usize,
    parse_off: usize,
    cap: usize,
    // PSI
    pmt_pid: Option<u16>,
    video_pid: Option<u16>,
    codec: Codec,
    /// Latest PAT/PMT packets (full 188 bytes): prepended to a cut so the
    /// staged TS can start exactly at the keyframe with no `-ss` seek.
    last_pat: Option<Vec<u8>>,
    last_pmt: Option<Vec<u8>>,
    // PES assembly
    pes: PesState,
    // PTS unwrap
    last_raw_pts: Option<i64>,
    wrap_offset: i64,
    // Index
    frames: VecDeque<FrameEntry>,
    #[allow(dead_code)] // kept for diagnostics/future clipping modes
    first_pts_ns: Option<i64>,
    max_pts_ns: Option<i64>,
    frame_dur_ns: i64,
    /// `min(qpc - pts)` observed: QPC = calib + pts with a few ms of error.
    calib_ns: Option<i128>,
    calib_samples: u64,
    /// Pre-encoder clock samples `(qpc_ns, pts_ns)` from the last
    /// `CAPTURE_WINDOW_NS` (showinfo frames only). Windowing matters: a
    /// session-wide average hides in-game starvation behind idle desktop
    /// periods (measured 55 fps average while the saved clip ran at ~30).
    clock_samples: VecDeque<(i128, i64)>,
    /// Systematic capture-side lag of the video anchor (compositor/frame-pool
    /// delivery + stderr hop, measured with the flash+beep rig; overridable
    /// with `MOONCLIP_SYNC_BIAS_MS`). The anchor is taken pre-encoder, so this
    /// is the only remaining constant and it is NOT per-codec.
    sync_bias_ns: i128,
}

impl TsRing {
    /// `cap` = ring capacity in bytes; `frame_dur_ns` = one frame at the
    /// configured CFR rate (used to close the video window at save).
    pub fn new(cap: usize, fps: u32) -> Self {
        Self {
            chunks: VecDeque::new(),
            base: 0,
            len: 0,
            total: 0,
            parse_abs: 0,
            parse_chunk: 0,
            parse_off: 0,
            cap: cap.max(16 * 1024 * 1024),
            pmt_pid: None,
            video_pid: None,
            codec: Codec::Other,
            last_pat: None,
            last_pmt: None,
            pes: PesState::default(),
            last_raw_pts: None,
            wrap_offset: 0,
            frames: VecDeque::new(),
            first_pts_ns: None,
            max_pts_ns: None,
            frame_dur_ns: 1_000_000_000 / fps.max(1) as i64,
            calib_ns: None,
            calib_samples: 0,
            clock_samples: VecDeque::new(),
            sync_bias_ns: default_sync_bias_ns(),
        }
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
        self.base = 0;
        self.len = 0;
        self.total = 0;
        self.parse_abs = 0;
        self.parse_chunk = 0;
        self.parse_off = 0;
        self.pmt_pid = None;
        self.video_pid = None;
        self.codec = Codec::Other;
        self.last_pat = None;
        self.last_pmt = None;
        self.pes = PesState::default();
        self.last_raw_pts = None;
        self.wrap_offset = 0;
        self.frames.clear();
        self.first_pts_ns = None;
        self.max_pts_ns = None;
        self.calib_ns = None;
        self.calib_samples = 0;
        self.clock_samples.clear();
    }

    #[allow(dead_code)] // diagnostics + tests
    pub fn bytes_len(&self) -> usize {
        self.len
    }

    /// Full byte snapshot (legacy probe path: codecs without a keyframe
    /// scanner). The indexed path never copies the whole ring.
    pub fn bytes_snapshot(&self) -> Vec<u8> {
        self.slice_from(self.base).unwrap_or_default()
    }

    /// Contiguous copy of the ring from an absolute offset to the newest byte.
    fn slice_from(&self, abs: u64) -> Option<Vec<u8>> {
        let mut skip = abs.checked_sub(self.base)? as usize;
        if skip >= self.len {
            return None;
        }
        let mut out = Vec::with_capacity(self.len - skip);
        let mut started = false;
        for chunk in &self.chunks {
            if !started {
                if skip >= chunk.len() {
                    skip -= chunk.len();
                    continue;
                }
                out.extend_from_slice(&chunk[skip..]);
                started = true;
            } else {
                out.extend_from_slice(chunk);
            }
        }
        Some(out)
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// How many indexed frames are keyframes (diagnostics).
    pub fn key_count(&self) -> usize {
        self.frames.iter().filter(|f| f.key).count()
    }

    pub fn max_pts_ns(&self) -> Option<i64> {
        self.max_pts_ns
    }

    pub fn calib_ns(&self) -> Option<i128> {
        self.calib_ns
    }

    /// Test hook: disable the rig-measured anchor bias for pure math checks.
    #[cfg(test)]
    pub fn set_sync_bias_ns(&mut self, ns: i128) {
        self.sync_bias_ns = ns;
    }

    /// Test hook: allow a cap below one chunk to exercise chunk trimming.
    #[cfg(test)]
    pub fn new_test(cap: usize, fps: u32) -> Self {
        let mut ring = Self::new(cap, fps);
        ring.cap = cap;
        ring
    }

    /// Feed a `showinfo` clock sample: the frame's PTS (100 ns units, same
    /// domain as the WGC capture timestamps) and the local QPC at the moment
    /// the log line reached this process. showinfo runs BEFORE the encoder,
    /// so the minimum collapses the (large, constant) encoder lookahead that
    /// would otherwise poison the PTS<->QPC mapping.
    pub fn note_clock_sample(&mut self, pts_ns: i64, qpc_ns: i128) {
        let cand = qpc_ns - self.sync_bias_ns - pts_ns as i128;
        self.calib_ns = Some(self.calib_ns.map_or(cand, |c| c.min(cand)));
        self.calib_samples += 1;
        self.clock_samples.push_back((qpc_ns, pts_ns));
        while let Some(&(q, _)) = self.clock_samples.front() {
            if q < qpc_ns - CAPTURE_WINDOW_NS {
                self.clock_samples.pop_front();
            } else {
                break;
            }
        }
    }

    /// Real capture rate over the last `CAPTURE_WINDOW_NS`: `(frames, span_s,
    /// fps)`. `showinfo` only sees frames the source actually delivered, so
    /// this is the number to watch for GPU/compositor starvation; the window
    /// keeps it per-save instead of averaging idle desktop periods in.
    pub fn capture_rate(&self) -> (u64, f64, f64) {
        let Some(&(q0, _)) = self.clock_samples.front() else {
            return (0, 0.0, 0.0);
        };
        let Some(&(q1, _)) = self.clock_samples.back() else {
            return (0, 0.0, 0.0);
        };
        let span_s = (q1 - q0).max(0) as f64 / 1e9;
        let frames = self.clock_samples.len() as u64;
        if span_s <= 0.0 || frames < 2 {
            return (frames, span_s, 0.0);
        }
        (frames, span_s, (frames - 1) as f64 / span_s)
    }

    #[allow(dead_code)] // diagnostics + tests
    pub fn calib_samples(&self) -> u64 {
        self.calib_samples
    }

    /// Append fresh bytes from the encoder and index whatever is complete.
    /// `qpc_ns` must be the local QPC reading taken around the read that
    /// produced these bytes (calibration takes the minimum over the session).
    pub fn push(&mut self, bytes: &[u8], qpc_ns: i128) {
        if bytes.is_empty() {
            return;
        }
        self.total += bytes.len() as u64;
        let mut rest = bytes;
        while !rest.is_empty() {
            let tail_space = match self.chunks.back() {
                Some(tail) if tail.len() < CHUNK_BYTES => CHUNK_BYTES - tail.len(),
                _ => 0,
            };
            if tail_space == 0 {
                self.chunks
                    .push_back(Arc::new(Vec::with_capacity(CHUNK_BYTES)));
                continue;
            }
            let take = tail_space.min(rest.len());
            Arc::make_mut(self.chunks.back_mut().expect("tail chunk"))
                .extend_from_slice(&rest[..take]);
            self.len += take;
            rest = &rest[take..];
        }
        self.parse_pending(qpc_ns);
        self.trim();
    }

    /// Byte at an absolute offset (walks at most a couple of chunks).
    fn peek(&self, abs: u64) -> Option<u8> {
        let mut remaining = abs.checked_sub(self.parse_abs)?;
        let mut idx = self.parse_chunk;
        let mut off = self.parse_off;
        loop {
            let chunk = self.chunks.get(idx)?;
            let abs_in_chunk = off + remaining as usize;
            if abs_in_chunk < chunk.len() {
                return Some(chunk[abs_in_chunk]);
            }
            remaining -= (chunk.len() - off) as u64;
            idx += 1;
            off = 0;
        }
    }

    /// Advance the parse cursor by one byte (TS resync path).
    fn skip_byte(&mut self) {
        let len = self
            .chunks
            .get(self.parse_chunk)
            .map(|c| c.len())
            .unwrap_or(0);
        self.parse_abs += 1;
        self.parse_off += 1;
        if self.parse_off >= len && self.parse_chunk + 1 < self.chunks.len() {
            self.parse_chunk += 1;
            self.parse_off = 0;
        }
    }

    /// Give up on an unparseable head: consume everything unparsed.
    fn skip_to_end(&mut self) {
        self.parse_abs = self.base + self.len as u64;
        match self.chunks.len() {
            0 => {
                self.parse_chunk = 0;
                self.parse_off = 0;
            }
            n => {
                self.parse_chunk = n - 1;
                self.parse_off = self.chunks[n - 1].len();
            }
        }
    }

    /// Consume every complete TS packet from the parse cursor, assembling
    /// packets that span two chunks (188 bytes at a time, never a giant
    /// contiguous buffer).
    fn parse_pending(&mut self, qpc_ns: i128) {
        loop {
            // Sync check: valid head needs 0x47 at +0 (and +188 once known).
            match (
                self.peek(self.parse_abs),
                self.peek(self.parse_abs + TS_PACKET_LEN as u64),
            ) {
                (Some(0x47), Some(0x47)) | (Some(0x47), None) => {}
                (None, _) => break,
                _ => {
                    let mut ok = false;
                    for _ in 0..(TS_PACKET_LEN * 8) {
                        let head = self.peek(self.parse_abs) == Some(0x47);
                        let pair = self
                            .peek(self.parse_abs + TS_PACKET_LEN as u64)
                            .map_or(true, |b| b == 0x47);
                        if head && pair {
                            ok = true;
                            break;
                        }
                        self.skip_byte();
                    }
                    if !ok {
                        self.skip_to_end();
                        break;
                    }
                }
            }
            let Some(chunk_len) = self.chunks.get(self.parse_chunk).map(|c| c.len()) else {
                break;
            };
            let mut packet = [0u8; TS_PACKET_LEN];
            if self.parse_off + TS_PACKET_LEN <= chunk_len {
                let chunk = &self.chunks[self.parse_chunk];
                packet.copy_from_slice(&chunk[self.parse_off..self.parse_off + TS_PACKET_LEN]);
            } else {
                let first = chunk_len - self.parse_off;
                let need = TS_PACKET_LEN - first;
                let Some(next) = self.chunks.get(self.parse_chunk + 1) else {
                    break; // wait for more data
                };
                if next.len() < need {
                    break;
                }
                packet[..first].copy_from_slice(&self.chunks[self.parse_chunk][self.parse_off..]);
                packet[first..].copy_from_slice(&next[..need]);
            }
            let pos_abs = self.parse_abs;
            self.parse_packet(&packet, pos_abs, qpc_ns);
            self.parse_abs += TS_PACKET_LEN as u64;
            self.parse_off += TS_PACKET_LEN;
            while self.parse_chunk + 1 < self.chunks.len()
                && self.parse_off >= self.chunks[self.parse_chunk].len()
            {
                self.parse_off -= self.chunks[self.parse_chunk].len();
                self.parse_chunk += 1;
            }
        }
    }

    /// Pop whole chunks from the front: O(1), no memmove of the live ring
    /// (the old contiguous buffer memmoved ~326 MB every ~0.4 s at 120 s).
    fn trim(&mut self) {
        while self.len > self.cap + RING_SLACK && !self.chunks.is_empty() {
            // Never discard the chunk the parser is still working in.
            if self.parse_chunk == 0 && self.parse_off < self.chunks[0].len() {
                break;
            }
            let front = self.chunks.pop_front().expect("non-empty");
            self.base += front.len() as u64;
            self.len -= front.len();
            if self.parse_chunk > 0 {
                self.parse_chunk -= 1;
            } else {
                // Parser had fully consumed the front chunk.
                self.parse_off = 0;
            }
        }
        while let Some(f) = self.frames.front() {
            if f.offset < self.base {
                self.frames.pop_front();
            } else {
                break;
            }
        }
    }

    fn parse_packet(&mut self, pkt: &[u8; TS_PACKET_LEN], pos_abs: u64, qpc_ns: i128) {
        let pid = (((pkt[1] & 0x1F) as u16) << 8) | pkt[2] as u16;
        let pusi = pkt[1] & 0x40 != 0;
        let afc = (pkt[3] >> 4) & 0x03;
        if afc == 0 || afc == 2 {
            return; // no payload
        }
        let mut p = 4;
        if afc == 3 {
            p += 1 + pkt[4] as usize;
        }
        if p >= TS_PACKET_LEN {
            return;
        }
        let end = TS_PACKET_LEN;
        match pid {
            0 if pusi => {
                self.parse_pat(pkt, p, end);
                self.last_pat = Some(pkt.to_vec());
            }
            pid if Some(pid) == self.pmt_pid && pusi => {
                self.parse_pmt(pkt, p, end);
                self.last_pmt = Some(pkt.to_vec());
            }
            pid if Some(pid) == self.video_pid => {
                self.parse_video_pes(pkt, pos_abs, p, end, pusi, qpc_ns)
            }
            _ => {}
        }
    }

    /// PAT: pointer_field + table; first non-zero program -> PMT PID.
    fn parse_pat(&mut self, b: &[u8], mut p: usize, end: usize) {
        if p >= end {
            return;
        }
        p += 1 + b[p] as usize; // skip pointer_field
        if p + 8 > end || b[p] != 0x00 {
            return;
        }
        let section_len = (((b[p + 1] & 0x0F) as usize) << 8) | b[p + 2] as usize;
        let mut q = p + 8;
        let stop = (p + 3 + section_len).min(end);
        while q + 4 <= stop {
            let program = ((b[q] as u16) << 8) | b[q + 1] as u16;
            let pid = (((b[q + 2] & 0x1F) as u16) << 8) | b[q + 3] as u16;
            if program != 0 {
                self.pmt_pid = Some(pid);
                return;
            }
            q += 4;
        }
    }

    /// PMT: first elementary stream on the private stream list is the video
    /// (the engine runs `-an`, so there is exactly one stream).
    fn parse_pmt(&mut self, b: &[u8], mut p: usize, end: usize) {
        if p >= end {
            return;
        }
        p += 1 + b[p] as usize; // skip pointer_field
        if p + 12 > end || b[p] != 0x02 {
            return;
        }
        let section_len = (((b[p + 1] & 0x0F) as usize) << 8) | b[p + 2] as usize;
        let info_len = (((b[p + 10] & 0x0F) as usize) << 8) | b[p + 11] as usize;
        let q = p + 12 + info_len;
        let stop = (p + 3 + section_len).min(end);
        if q + 5 <= stop {
            let stream_type = b[q];
            let pid = (((b[q + 1] & 0x1F) as u16) << 8) | b[q + 2] as u16;
            self.video_pid = Some(pid);
            self.codec = match stream_type {
                0x1B => Codec::H264,
                0x24 => Codec::Hevc,
                _ => Codec::Other,
            };
        }
    }

    fn parse_video_pes(
        &mut self,
        b: &[u8],
        pos_abs: u64,
        p: usize,
        end: usize,
        pusi: bool,
        qpc_ns: i128,
    ) {
        if pusi {
            self.finish_pes();
            if p + 9 > end || b[p] != 0x00 || b[p + 1] != 0x00 || b[p + 2] != 0x01 {
                return;
            }
            // PES: start code + stream id (4) + length (2) + '10' flags (1) +
            // PTS/DTS flags (1) + header_data_length (1) + optional PTS/DTS.
            let flags = b[p + 7];
            let header_len = b[p + 8] as usize;
            let pts = if flags & 0x80 != 0 && p + 14 <= end {
                Some(parse_pts(&b[p + 9..p + 14]))
            } else {
                None
            };
            let payload = (p + 9 + header_len).min(end);
            self.pes = PesState {
                active: true,
                offset: pos_abs,
                qpc_ns,
                pts_ns: pts,
                key: false,
                scan_done: false,
                scan: Vec::new(),
            };
            self.scan_payload(&b[payload..end]);
        } else if self.pes.active && !self.pes.scan_done {
            self.scan_payload(&b[p..end]);
        }
    }

    /// Accumulate payload bytes and look for a keyframe NAL in the first KBs.
    fn scan_payload(&mut self, bytes: &[u8]) {
        let room = PES_SCAN_LIMIT.saturating_sub(self.pes.scan.len());
        if room == 0 {
            self.pes.scan_done = true;
            return;
        }
        let take = bytes.len().min(room);
        if take > 0 {
            self.pes.scan.extend_from_slice(&bytes[..take]);
        }
        if let Some(key) = nal_is_key(&self.pes.scan, self.codec) {
            self.pes.key = key;
            self.pes.scan_done = true;
        } else if self.pes.scan.len() >= PES_SCAN_LIMIT {
            self.pes.scan_done = true;
        }
    }

    fn finish_pes(&mut self) {
        if !self.pes.active {
            return;
        }
        self.pes.active = false;
        let Some(raw) = self.pes.pts_ns else { return };
        let pts_raw = self.unwrap_pts(raw);
        let pts_ns = (pts_raw as i128 * 1_000_000_000 / PTS_HZ) as i64;
        let frame = FrameEntry {
            pts_ns,
            key: self.pes.key,
            offset: self.pes.offset,
        };
        if self.first_pts_ns.is_none() {
            self.first_pts_ns = Some(pts_ns);
        }
        self.max_pts_ns = Some(self.max_pts_ns.map_or(pts_ns, |m| m.max(pts_ns)));
        self.frames.push_back(frame);
        if self.frames.len() > 65_536 {
            self.frames.pop_front();
        }
        // Calibration: the PES was captured at `pts`; it reached Rust at
        // `pes.qpc_ns` (encoder + muxer + pipe latency included). The minimum
        // over the session collapses that latency to its best case. The
        // pre-encoder `showinfo` samples (`note_clock_sample`) are tighter and
        // dominate this one; both carry the same bias correction.
        let cand = self.pes.qpc_ns - self.sync_bias_ns - pts_ns as i128;
        self.calib_ns = Some(self.calib_ns.map_or(cand, |c| c.min(cand)));
        self.calib_samples += 1;
    }

    fn unwrap_pts(&mut self, raw: i64) -> i64 {
        if let Some(last) = self.last_raw_pts {
            if raw < last && last - raw > (PTS_WRAP / 2) {
                self.wrap_offset += PTS_WRAP;
            }
        }
        self.last_raw_pts = Some(raw);
        raw + self.wrap_offset
    }

    /// Close the in-flight PES (the newest frame is otherwise only indexed
    /// when the NEXT PES starts). Call from the save path / tests before
    /// reading the index; live parsing stays lazy.
    pub fn finalize(&mut self) {
        self.finish_pes();
    }

    /// Select a clip: latest keyframe at/before `end - span` (falling back to
    /// the earliest keyframe when the buffer is shorter), staged with
    /// `CUT_PREROLL_NS` of context and mapped to QPC. `None` when no keyframe
    /// was detected (unsupported codec) or the ring has no content.
    /// One-shot convenience wrapper (tests + legacy paths): copies the bytes
    /// while nothing else touches the ring. The engine uses `cut_plan` +
    /// `assemble` so the copy happens outside the lock.
    #[allow(dead_code)]
    pub fn cut(&self, span_ns: i64, min_clip_ns: i64) -> Option<Cut> {
        let plan = self.cut_plan(span_ns, min_clip_ns)?;
        Some(Cut {
            bytes: plan.assemble(),
            ss_secs: plan.ss_secs,
            window_ns: plan.window_ns,
            qpc_start_ns: plan.qpc_start_ns,
            key_pts_ns: plan.key_pts_ns,
            end_pts_ns: plan.end_pts_ns,
        })
    }

    /// Phase 1 of a save: keyframe selection + cheap chunk snapshot. Holds no
    /// state and copies no payload, so the caller's lock is held for
    /// microseconds even on a 120 s ring. See [`CutPlan::assemble`].
    pub fn cut_plan(&self, span_ns: i64, min_clip_ns: i64) -> Option<CutPlan> {
        let end = self.max_pts_ns?;
        let calib = self.calib_ns?;
        let frames: Vec<&FrameEntry> = self.frames.iter().collect();
        if frames.is_empty() {
            return None;
        }
        let target = end - span_ns;
        let usable: Vec<&FrameEntry> = frames
            .iter()
            .copied()
            .filter(|f| f.key && f.pts_ns <= end - min_clip_ns)
            .collect();
        let key = usable
            .iter()
            .rev()
            .copied()
            .find(|f| f.pts_ns <= target)
            .or_else(|| usable.first().copied())?;
        // Preferred: stage the TS starting exactly at the keyframe's PES, with
        // the latest PAT/PMT in front. No `-ss`, so the output starts at 0 and
        // stays sample-aligned with the audio window (which starts at the same
        // instant). Falls back to pre-roll + `-ss` only with no PSI yet.
        if key.offset < self.base || key.offset >= self.base + self.len as u64 {
            return None;
        }
        let mut pat = None;
        let mut pmt = None;
        let mut ss_secs = 0.0;
        let (chunks, take_from, total_hint) = match (&self.last_pat, &self.last_pmt) {
            (Some(p), Some(pm)) => {
                pat = Some(p.clone());
                pmt = Some(pm.clone());
                self.chunk_snapshot_from(key.offset)?
            }
            _ => {
                // Pre-roll: start staging from a frame `CUT_PREROLL_NS` before
                // the keyframe (demuxer context), then `-ss` to the exact
                // keyframe. Only frames still inside the ring count.
                let preroll_floor = key.pts_ns - CUT_PREROLL_NS;
                let first = frames
                    .iter()
                    .copied()
                    .find(|f| f.pts_ns >= preroll_floor && f.offset >= self.base)
                    .unwrap_or(key);
                let slice_start = first.offset.max(self.base);
                // A hair BEFORE the keyframe: input `-ss` drops every packet
                // with PTS < target, so seeking past the keyframe would cut
                // the IDR and start the file one frame late.
                ss_secs = (key.pts_ns - first.pts_ns) as f64 / 1e9 - 0.001;
                self.chunk_snapshot_from(slice_start)?
            }
        };
        let window_ns = (end - key.pts_ns) as i128 + self.frame_dur_ns as i128;
        Some(CutPlan {
            pat,
            pmt,
            chunks,
            take_from,
            total_hint,
            ss_secs,
            window_ns,
            qpc_start_ns: calib + key.pts_ns as i128,
            key_pts_ns: key.pts_ns,
            end_pts_ns: end,
        })
    }

    /// Cheap snapshot (Arc handles) of the stored chunks covering
    /// `[abs, base + len)`: chunks, offset into the first one and total bytes.
    fn chunk_snapshot_from(&self, abs: u64) -> Option<(VecDeque<Arc<Vec<u8>>>, usize, usize)> {
        let mut skip = abs.checked_sub(self.base)? as usize;
        if skip >= self.len {
            return None;
        }
        let mut chunks = VecDeque::new();
        let mut take_from = 0usize;
        let mut total = 0usize;
        let mut started = false;
        for chunk in &self.chunks {
            if !started {
                if skip >= chunk.len() {
                    skip -= chunk.len();
                    continue;
                }
                take_from = skip;
                started = true;
                total += chunk.len() - skip;
            } else {
                total += chunk.len();
            }
            chunks.push_back(chunk.clone());
        }
        if !started {
            return None;
        }
        Some((chunks, take_from, total))
    }
}

/// Parse a 5-byte PES PTS field (33 bits).
fn parse_pts(b: &[u8]) -> i64 {
    (((b[0] as i64) & 0x0E) << 29)
        | ((b[1] as i64) << 22)
        | (((b[2] as i64) & 0xFE) << 14)
        | ((b[3] as i64) << 7)
        | ((b[4] as i64) >> 1)
}

/// Scan for a keyframe NAL unit in an H.264/HEVC elementary stream head.
/// Returns `Some(true)` when a keyframe was seen, `None` when no verdict can
/// be reached yet (or the codec has no scanner).
fn nal_is_key(buf: &[u8], codec: Codec) -> Option<bool> {
    if codec == Codec::Other {
        return None;
    }
    let mut i = 0usize;
    while i + 3 < buf.len() {
        if buf[i] == 0 && buf[i + 1] == 0 && buf[i + 2] == 1 {
            let hdr = buf[i + 3];
            let key = match codec {
                Codec::H264 => hdr & 0x1F == 5, // IDR slice
                Codec::Hevc => (hdr >> 1) & 0x3F >= 16 && (hdr >> 1) & 0x3F <= 23, // IRAP
                Codec::Other => false,
            };
            if key {
                return Some(true);
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build one TS packet (no adaptation field) for `pid`.
    fn ts_packet(pid: u16, pusi: bool, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![0u8; TS_PACKET_LEN];
        p[0] = 0x47;
        p[1] = ((pid >> 8) as u8 & 0x1F) | if pusi { 0x40 } else { 0 };
        p[2] = pid as u8;
        p[3] = 0x10; // payload only
        let n = payload.len().min(TS_PACKET_LEN - 4);
        p[4..4 + n].copy_from_slice(&payload[..n]);
        p
    }

    /// Continuation packet flagged so the parser knows it is payload-only.
    fn ts_cont(pid: u16, payload: &[u8]) -> Vec<u8> {
        ts_packet(pid, false, payload)
    }

    fn pat(program: u16, pmt_pid: u16) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&program.to_be_bytes());
        body.extend_from_slice(&(0xE000u16 | pmt_pid).to_be_bytes());
        let section_len = (5 + body.len() + 4) as u16; // after this field + CRC
        let mut s = vec![0x00];
        s.extend_from_slice(&(0xB000u16 | section_len).to_be_bytes());
        s.extend_from_slice(&[0x00, 0x01]); // tsid
        s.push(0xC1);
        s.push(0x00);
        s.push(0x00);
        s.extend_from_slice(&body);
        s.extend_from_slice(&[0, 0, 0, 0]); // CRC (unused)
        let mut payload = vec![0u8]; // pointer_field
        payload.extend_from_slice(&s);
        ts_packet(0, true, &payload)
    }

    fn pmt(pmt_pid: u16, stream_type: u8, es_pid: u16) -> Vec<u8> {
        let mut body = vec![0u8; 0];
        body.extend_from_slice(&0xE000u16.to_be_bytes()); // PCR pid
        body.extend_from_slice(&0xF000u16.to_be_bytes()); // program info len
        body.extend_from_slice(&[stream_type]);
        body.extend_from_slice(&(0xE000u16 | es_pid).to_be_bytes());
        body.extend_from_slice(&0xF000u16.to_be_bytes());
        let section_len = (9 + body.len() + 4) as u16;
        let mut s = vec![0x02];
        s.extend_from_slice(&(0xB000u16 | section_len).to_be_bytes());
        s.extend_from_slice(&[0x00, 0x01]); // program number
        s.push(0xC1);
        s.push(0x00);
        s.push(0x00);
        s.extend_from_slice(&body);
        s.extend_from_slice(&[0, 0, 0, 0]); // CRC
        let mut payload = vec![0u8]; // pointer_field
        payload.extend_from_slice(&s);
        ts_packet(pmt_pid, true, &payload)
    }

    /// PES packet with a PTS and the given elementary payload.
    fn pes(pts90k: u64, es: &[u8]) -> Vec<u8> {
        let mut h = vec![0x00, 0x00, 0x01, 0xE0];
        h.extend_from_slice(&[0x00, 0x00]); // PES length (unused for video)
        h.push(0x80); // '10' flags
        h.push(0x80); // PTS_DTS_flags: PTS present
        h.push(5); // header_data_length (PTS only)
        let pts = pts90k & ((1 << 33) - 1);
        h.push(0x20 | (((pts >> 30) as u8 & 0x07) << 1) | 1);
        h.push(((pts >> 22) as u8) & 0xFF);
        h.push(0x00 | (((pts >> 15) as u8 & 0x7F) << 1) | 1);
        h.push(((pts >> 7) as u8) & 0xFF);
        h.push((((pts) as u8 & 0x7F) << 1) | 1);
        h.extend_from_slice(es);
        h
    }

    fn nal(start: u8, kind: u8) -> Vec<u8> {
        vec![0x00, 0x00, 0x01, (kind & 0x1F) | (start << 5)]
    }

    struct Synth {
        bytes: Vec<u8>,
    }

    impl Synth {
        fn new(h264: bool) -> Self {
            let st = if h264 { 0x1B } else { 0x24 };
            let mut bytes = pat(1, 0x100);
            bytes.extend(pmt(0x100, st, 0x101));
            Self { bytes }
        }
        fn frame(&mut self, pts90k: u64, key: bool) {
            // PES header + start codes; IDR for H.264, IRAP (type 19) HEVC.
            let mut es = Vec::new();
            es.extend(nal(0, if key { 5 } else { 1 }));
            es.extend(std::iter::repeat(0xAA).take(600));
            let packet = pes(pts90k, &es);
            self.bytes.extend(ts_packet(0x101, true, &packet));
            let mut rest = &packet[TS_PACKET_LEN - 4..];
            while !rest.is_empty() {
                self.bytes.extend(ts_cont(0x101, rest));
                rest = &rest[(TS_PACKET_LEN - 4).min(rest.len())..];
            }
        }
    }

    #[test]
    fn indexes_pts_and_keyframes() {
        let mut s = Synth::new(true);
        s.frame(90_000, true); // t=1s
        s.frame(93_000, false);
        s.frame(96_000, true);
        let mut ring = TsRing::new(16 * 1024 * 1024, 60);
        ring.push(&s.bytes, 0);
        ring.finalize();
        assert_eq!(ring.frame_count(), 3);
        let f: Vec<_> = ring.frames.iter().copied().collect();
        assert!(f.iter().any(|e| e.key), "no keyframe indexed");
        assert_eq!(f[0].pts_ns, 1_000_000_000);
        assert!(f[0].key);
        assert!(!f[1].key);
        assert!(f[2].key);
        assert_eq!(ring.max_pts_ns(), Some(96_000 * 100_000 / 9));
    }

    #[test]
    fn parses_in_chunks_and_calibrates() {
        let mut s = Synth::new(true);
        s.frame(90_000, true);
        s.frame(93_000, false);
        // Feed 100-byte chunks with a fake QPC arrival per chunk.
        let mut ring = TsRing::new(16 * 1024 * 1024, 60);
        let mut qpc = 5_000_000_000i128;
        for chunk in s.bytes.chunks(100) {
            ring.push(chunk, qpc);
            qpc += 1_000_000; // 1 ms per chunk
        }
        ring.finalize();
        assert_eq!(ring.frame_count(), 2);
        // calib = min(qpc_at_pes_arrival - pts_ns) over both frames; the two
        // frames arrive a few chunks apart with ~1 s PTS spacing.
        let calib = ring.calib_ns().unwrap();
        assert!((3_500_000_000..=5_500_000_000).contains(&calib), "{calib}");
        assert_eq!(ring.calib_samples(), 2);
    }

    #[test]
    fn capture_rate_is_windowed() {
        let mut ring = TsRing::new(16 * 1024 * 1024, 60);
        let mut qpc = 1_000_000_000i128;
        let mut pts = 0i64;
        let step = 16_666_667i64;
        // 10 s of 60 fps samples: the window keeps them all.
        for _ in 0..600 {
            ring.note_clock_sample(pts, qpc);
            pts += step;
            qpc += step as i128;
        }
        let (n, _span, fps) = ring.capture_rate();
        assert!((590..=600).contains(&n), "{n}");
        assert!((55.0..=65.0).contains(&fps), "{fps}");
        // A 30 s gap drops every old sample: only the fresh second remains.
        qpc += 30_000_000_000;
        pts += 30_000_000_000;
        for _ in 0..60 {
            ring.note_clock_sample(pts, qpc);
            pts += step;
            qpc += step as i128;
        }
        let (n2, span2, fps2) = ring.capture_rate();
        assert!(n2 <= 61, "{n2}");
        assert!(span2 <= 1.1, "{span2}");
        assert!((55.0..=65.0).contains(&fps2), "{fps2}");
    }

    #[test]
    fn hevc_irap_detected() {
        // HEVC IRAP NAL: first byte (0x26 >> 1) = 19 after the start code.
        let mut es = vec![0x00, 0x00, 0x01, 19 << 1];
        es.extend(std::iter::repeat(0xBB).take(400));
        assert_eq!(nal_is_key(&es, Codec::Hevc), Some(true));
        let mut es2 = vec![0x00, 0x00, 0x01, 1 << 1]; // TRAIL_R
        es2.extend(std::iter::repeat(0xBB).take(300));
        assert_eq!(nal_is_key(&es2, Codec::Hevc), Some(false));
        assert_eq!(nal_is_key(&es2, Codec::H264), Some(false));
        assert_eq!(nal_is_key(&es2, Codec::Other), None);
    }

    #[test]
    fn cut_selects_latest_keyframe_at_or_before_target() {
        let mut s = Synth::new(true);
        // 60 fps: frames every 1500 ticks, keyframes every 2 s (120 frames).
        let mut pts = 0u64;
        for i in 0..300u64 {
            s.frame(pts, i % 120 == 0);
            pts += 1500;
        }
        let mut ring = TsRing::new(64 * 1024 * 1024, 60);
        ring.set_sync_bias_ns(0);
        ring.push(&s.bytes, 100_000_000_000);
        ring.finalize();
        let end = ring.max_pts_ns().unwrap();
        let span = 2_000_000_000i64; // last 2 s
        let cut = ring.cut(span, 1_000_000_000).expect("cut");
        // Latest keyframe <= end - 2s.
        assert!(cut.key_pts_ns <= end - span);
        assert!(end - cut.key_pts_ns >= span);
        assert!(end - cut.key_pts_ns < span + 2_100_000_000);
        // With PSI available the stage starts exactly at the keyframe (no
        // seek): PAT + PMT + the sliced ring.
        assert_eq!(cut.ss_secs, 0.0);
        assert!(cut.bytes.len() <= ring.bytes_len() + 2 * TS_PACKET_LEN);
        // All synthetic frames arrived at the same QPC, so the calibration
        // (min of qpc - pts) is anchored on the newest frame; the mapping is
        // still exact and shifts every PTS by the same constant.
        assert_eq!(
            cut.qpc_start_ns,
            100_000_000_000 + cut.key_pts_ns as i128 - end as i128
        );
        // Window closes one frame after the last display time.
        assert_eq!(
            cut.window_ns,
            (cut.end_pts_ns - cut.key_pts_ns) as i128 + 1_000_000_000 / 60
        );
    }

    #[test]
    fn cut_none_without_keys_or_calibration() {
        let mut s = Synth::new(true);
        s.frame(0, false);
        s.frame(1500, false);
        let mut ring = TsRing::new(16 * 1024 * 1024, 60);
        // No QPC ever observed -> no calibration.
        ring.push(&s.bytes, 0);
        ring.finalize();
        // Force "no calib" by clearing it (the push above set one).
        ring.calib_ns = None;
        assert!(ring.cut(1_000_000_000, 500_000_000).is_none());
        ring.calib_ns = Some(0);
        assert!(ring.cut(1_000_000_000, 500_000_000).is_none()); // no keyframes
    }

    #[test]
    fn ptrs_parse_roundtrip() {
        // PTS 1s = 90000 ticks; PES: 4B start+id, 2B len, 2B flags,
        // 1B header_data_length, then the 5 PTS bytes at offset 9.
        let es = pes(90_000, &[0u8; 8]);
        let pts = parse_pts(&es[9..14]);
        assert_eq!(pts, 90_000);
        let es2 = pes((1 << 33) - 2, &[0u8; 8]);
        let pts2 = parse_pts(&es2[9..14]);
        assert_eq!(pts2, (1 << 33) - 2);
    }

    #[test]
    fn packets_span_chunk_boundaries() {
        // >1 chunk of TS pushed in 64 KB reads (the engine's real cadence):
        // chunk boundaries fall mid-packet, the parser must assemble them.
        let mut s = Synth::new(true);
        let frames = 3200u64; // ~2.4 MB of TS (752 B per frame)
        for i in 0..frames {
            s.frame(i * 3000, i % 120 == 0);
        }
        assert!(s.bytes.len() > CHUNK_BYTES * 2, "need to cross chunks");
        let mut ring = TsRing::new(64 * 1024 * 1024, 60);
        ring.set_sync_bias_ns(0);
        let mut qpc = 1_000_000_000i128;
        for chunk in s.bytes.chunks(64 * 1024) {
            ring.push(chunk, qpc);
            qpc += 5_000_000;
        }
        ring.finalize();
        assert_eq!(ring.frame_count() as u64, frames);
        assert!(ring.calib_ns().is_some());
        assert!(ring.cut(5_000_000_000, 1_000_000_000).is_some());
        // The staged slice must be contiguous and TS-aligned from the keyframe.
        let cut = ring.cut(5_000_000_000, 1_000_000_000).unwrap();
        assert_eq!(cut.ss_secs, 0.0);
        assert!(cut.bytes.len() > 100 * TS_PACKET_LEN);
    }

    #[test]
    fn ring_trims_front_and_keeps_index_valid() {
        // ~1.65 MB of TS into a 300 KB ring (slack 1 MB): chunks get popped
        // while the index keeps absolute offsets valid.
        let mut s = Synth::new(true);
        for i in 0..2200u64 {
            s.frame(i * 3000, i % 5 == 0);
        }
        let mut ring = TsRing::new_test(300 * 1024, 60);
        ring.push(&s.bytes, 1);
        ring.finalize();
        assert!(ring.bytes_len() > 0);
        assert!(
            ring.bytes_len() <= 300 * 1024 + RING_SLACK + CHUNK_BYTES,
            "ring did not bound itself: {}",
            ring.bytes_len()
        );
        assert!(ring.base > 0, "nothing was trimmed");
        let base = ring.base;
        if let Some(f) = ring.frames.front() {
            assert!(f.offset >= base);
        }
        let cut = ring.cut(10_000_000, 0);
        assert!(cut.is_none() || !cut.unwrap().bytes.is_empty());
    }
}
