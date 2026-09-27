/**
 * Single-clock audio engine for the editor.
 *
 * Everything audible is scheduled on ONE AudioContext: each project segment
 * contributes its stems (Mix/Game/Mic) as AudioBufferSourceNodes started at an
 * exact context time, through per-track and per-segment GainNodes. The
 * context clock (`currentTime`) is the master: the playhead, the scrub bar and
 * the waveforms all derive from it, so nothing can drift. The <video> is muted
 * and slaved to this clock (picture only), like other web editors do.
 */

import type { EditorSourceInfo, Segment } from "./types";

export interface DecodedSource {
  clipId: string;
  durationMs: number;
  /** One buffer per stem that decoded successfully (labels keep their names). */
  stems: { label: string; buffer: AudioBuffer; peaks: number[][] }[];
}

/** Min/max pairs per channel, ready for canvas rendering. */
export function computePeaks(buffer: AudioBuffer, buckets = 900): number[][] {
  const channels = Math.min(2, buffer.numberOfChannels);
  const out: number[][] = [];
  const size = Math.max(1, Math.floor(buffer.length / buckets));
  for (let c = 0; c < channels; c++) {
    const data = buffer.getChannelData(Math.min(c, buffer.numberOfChannels - 1));
    const arr: number[] = [];
    for (let b = 0; b < buckets; b++) {
      let min = 1;
      let max = -1;
      const start = b * size;
      const end = Math.min(buffer.length, start + size);
      for (let i = start; i < end; i += 3) {
        const v = data[i];
        if (v < min) min = v;
        if (v > max) max = v;
      }
      arr.push(max, min);
    }
    out.push(arr);
  }
  return out;
}

/** How far the (muted) video is allowed to drift before a corrective seek. */
export const VIDEO_SYNC_TOLERANCE_MS = 120;

export class AudioTimeline {
  private ctx: AudioContext;
  private master: GainNode;
  private trackGains = new Map<string, GainNode>();
  private sources = new Map<string, DecodedSource>();
  private nodes: AudioBufferSourceNode[] = [];
  private segGains: GainNode[] = [];

  /** Timeline ms at `t0`; playback position = base + (ctx.now - t0). */
  private baseMs = 0;
  private t0 = 0;
  private started = false;
  private paused = true;

  private trackVolume: Record<string, number> = { mix: 1, game: 1, mic: 1 };

  constructor() {
    const Ctor =
      window.AudioContext ??
      (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
    this.ctx = new Ctor();
    this.master = this.ctx.createGain();
    this.master.gain.value = 1;
    this.master.connect(this.ctx.destination);
    for (const label of ["mix", "game", "mic"]) {
      const g = this.ctx.createGain();
      g.gain.value = this.trackVolume[label] ?? 1;
      g.connect(this.master);
      this.trackGains.set(label, g);
    }
  }

  get sampleRate(): number {
    return this.ctx.sampleRate;
  }

  /** Decode a source's stems once (cached). `null` tracks failed to decode. */
  async ensureSource(source: EditorSourceInfo): Promise<DecodedSource> {
    const cached = this.sources.get(source.clipId);
    if (cached) return cached;
    const stems: DecodedSource["stems"] = [];
    for (const stem of source.stems) {
      const buffer = await this.decode(stem.url);
      if (buffer) {
        stems.push({ label: stem.label, buffer, peaks: computePeaks(buffer) });
      } else {
        eprintln(`editor audio: stem ${stem.label} failed to decode`);
      }
    }
    const decoded: DecodedSource = {
      clipId: source.clipId,
      durationMs: source.durationMs,
      stems,
    };
    this.sources.set(source.clipId, decoded);
    return decoded;
  }

  private async decode(url: string): Promise<AudioBuffer | null> {
    try {
      const res = await fetch(url);
      if (!res.ok) return null;
      const bytes = await res.arrayBuffer();
      return await this.ctx.decodeAudioData(bytes);
    } catch {
      return null;
    }
  }

  /** Peaks per stem for a decoded source (empty until `ensureSource` ran). */
  peaksFor(clipId: string): { label: string; peaks: number[][] }[] {
    const src = this.sources.get(clipId);
    if (!src) return [];
    return src.stems.map((s) => ({ label: s.label, peaks: s.peaks }));
  }

  /** True when the source has playable decoded audio. */
  hasAudio(clipId: string): boolean {
    return (this.sources.get(clipId)?.stems.length ?? 0) > 0;
  }

  /** Live per-track gain (0..2). Applies to scheduled and future nodes. */
  setTrackVolume(label: string, value: number) {
    this.trackVolume[label] = value;
    const g = this.trackGains.get(label);
    if (g) g.gain.value = value;
  }

  /** Schedule every segment from `fromMs` and start the clock. */
  async play(
    segments: Segment[],
    fromMs: number,
    gainsOf: (segment: Segment) => Record<string, number>,
  ): Promise<void> {
    this.stopNodes();
    await this.ctx.resume();
    const lookahead = 0.06;
    this.t0 = this.ctx.currentTime + lookahead;
    this.baseMs = fromMs;
    for (const seg of segments) {
      const decoded = this.sources.get(seg.sourceClipId);
      if (!decoded || decoded.stems.length === 0) continue;
      const segEnd = seg.timelineStartMs + Math.max(0, seg.outMs - seg.inMs) / Math.max(0.05, seg.speed);
      if (segEnd <= fromMs) continue;
      const gains = gainsOf(seg);
      // A single-stem clip is its own mix; keep the label mapping sane.
      const single = decoded.stems.length === 1;
      for (const stem of decoded.stems) {
        const label = single ? "mix" : stem.label;
        // Per-segment gain only: the shared track GainNode applies the live
        // Mix/Game/Mic volume, so adjusting it never needs a reschedule.
        const gain = this.ctx.createGain();
        gain.gain.value = gains[label] ?? 1;
        gain.connect(this.trackGains.get(label) ?? this.master);
        const src = this.ctx.createBufferSource();
        src.buffer = stem.buffer;
        src.playbackRate.value = seg.speed;
        src.connect(gain);

        // Where in the source and when in the context this piece starts.
        const inside = fromMs > seg.timelineStartMs;
        const consumedMs = inside ? fromMs - seg.timelineStartMs : 0;
        const sourceOffset = (seg.inMs + consumedMs) / 1000;
        const sourceRemaining = Math.max(0, seg.outMs - (seg.inMs + consumedMs)) / 1000;
        const when =
          this.t0 + Math.max(0, seg.timelineStartMs - fromMs) / 1000;
        if (sourceRemaining <= 0.01) continue;
        try {
          src.start(when, sourceOffset, sourceRemaining);
        } catch {
          continue;
        }
        this.nodes.push(src);
        this.segGains.push(gain);
      }
    }
    this.started = true;
    this.paused = false;
  }

  /** Freeze the clock (nodes stay scheduled; context time stops advancing). */
  async pause(): Promise<void> {
    if (!this.started || this.paused) return;
    this.baseMs = this.currentTimeMs();
    this.paused = true;
    await this.ctx.suspend();
  }

  async resume(gainsRefresh?: () => void): Promise<void> {
    if (!this.started || !this.paused) return;
    await this.ctx.resume();
    this.t0 = this.ctx.currentTime;
    this.paused = false;
    gainsRefresh?.();
  }

  /** Rebuild the schedule at `ms` (used by seeking/scrubbing). */
  async seek(
    ms: number,
    segments: Segment[],
    keepPlaying: boolean,
    gainsOf: (segment: Segment) => Record<string, number>,
  ): Promise<void> {
    this.stopNodes();
    this.started = false;
    this.paused = true;
    if (keepPlaying) {
      await this.play(segments, ms, gainsOf);
    } else {
      this.baseMs = ms;
    }
  }

  currentTimeMs(): number {
    if (!this.started) return this.baseMs;
    if (this.paused) return this.baseMs;
    return this.baseMs + (this.ctx.currentTime - this.t0) * 1000;
  }

  isPlaying(): boolean {
    return this.started && !this.paused;
  }

  private stopNodes() {
    for (const n of this.nodes) {
      try {
        n.stop();
      } catch {
        /* already stopped */
      }
      n.disconnect();
    }
    this.nodes = [];
    for (const g of this.segGains) g.disconnect();
    this.segGains = [];
  }

  dispose() {
    this.stopNodes();
    for (const g of this.trackGains.values()) g.disconnect();
    this.trackGains.clear();
    this.master.disconnect();
    void this.ctx.close().catch(() => {});
    this.sources.clear();
  }
}

/** Logs through the same console the dev log shows. */
function eprintln(msg: string) {
  console.warn(`[moonclip] ${msg}`);
}
