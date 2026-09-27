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

/** Project track mix (Master scales the Game+Mic stems). */
export interface TrackGains {
  master: number;
  game: number;
  mic: number;
}

export class AudioTimeline {
  private ctx: AudioContext;
  private master: GainNode;
  private game: GainNode;
  private mic: GainNode;
  private analyserGame: AnalyserNode;
  private analyserMic: AnalyserNode;
  private levelBuf: Float32Array<ArrayBuffer>;
  private sources = new Map<string, DecodedSource>();
  private nodes: AudioBufferSourceNode[] = [];

  /** Timeline ms at `t0`; playback position = base + (ctx.now - t0). */
  private baseMs = 0;
  private t0 = 0;
  private started = false;
  private paused = true;

  constructor() {
    const Ctor =
      window.AudioContext ??
      (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
    this.ctx = new Ctor();
    this.master = this.ctx.createGain();
    this.master.gain.value = 1;
    this.master.connect(this.ctx.destination);
    this.game = this.ctx.createGain();
    this.game.gain.value = 1;
    this.game.connect(this.master);
    this.mic = this.ctx.createGain();
    this.mic.gain.value = 1;
    this.mic.connect(this.master);
    // Level meters (diagnostics + UI): the analysers tap the track nodes.
    this.analyserGame = this.ctx.createAnalyser();
    this.analyserGame.fftSize = 256;
    this.game.connect(this.analyserGame);
    this.analyserMic = this.ctx.createAnalyser();
    this.analyserMic.fftSize = 256;
    this.mic.connect(this.analyserMic);
    this.levelBuf = new Float32Array(new ArrayBuffer(this.analyserGame.fftSize * 4));
  }

  /** Live RMS per track (0..1) for the level meters. */
  levels(): { game: number; mic: number } {
    const rms = (a: AnalyserNode) => {
      a.getFloatTimeDomainData(this.levelBuf);
      let sum = 0;
      for (let i = 0; i < this.levelBuf.length; i++) sum += this.levelBuf[i] * this.levelBuf[i];
      return Math.sqrt(sum / this.levelBuf.length);
    };
    return { game: rms(this.analyserGame), mic: rms(this.analyserMic) };
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

  /** Live track mix. Shared GainNodes, so no reschedule is ever needed. */
  setGains(gains: TrackGains) {
    this.master.gain.value = Math.max(0, Math.min(4, gains.master));
    this.game.gain.value = Math.max(0, Math.min(4, gains.game));
    this.mic.gain.value = Math.max(0, Math.min(4, gains.mic));
  }

  /** Schedule every segment from `fromMs` and start the clock. Track 1 of a
   *  recording is the sum of Game+Mic, so only the stems are scheduled. */
  async play(segments: Segment[], fromMs: number, gains: TrackGains): Promise<void> {
    this.stopNodes();
    await this.ctx.resume();
    this.setGains(gains);
    const lookahead = 0.06;
    this.t0 = this.ctx.currentTime + lookahead;
    this.baseMs = fromMs;
    for (const seg of segments) {
      const decoded = this.sources.get(seg.sourceClipId);
      if (!decoded || decoded.stems.length === 0) continue;
      const segEnd =
        seg.timelineStartMs + Math.max(0, seg.outMs - seg.inMs) / Math.max(0.05, seg.speed);
      if (segEnd <= fromMs) continue;
      // Multi-stem sources: Game/Mic only (the Mix stem duplicates them).
      const wanted =
        decoded.stems.length > 1
          ? decoded.stems.filter((s) => s.label === "game" || s.label === "mic")
          : decoded.stems;
      for (const stem of wanted) {
        const target = decoded.stems.length > 1
          ? stem.label === "mic"
            ? this.mic
            : this.game
          : this.master;
        const src = this.ctx.createBufferSource();
        src.buffer = stem.buffer;
        src.playbackRate.value = seg.speed;
        src.connect(target);

        const inside = fromMs > seg.timelineStartMs;
        const consumedMs = inside ? fromMs - seg.timelineStartMs : 0;
        const sourceOffset = (seg.inMs + consumedMs) / 1000;
        const sourceRemaining = Math.max(0, seg.outMs - (seg.inMs + consumedMs)) / 1000;
        const when = this.t0 + Math.max(0, seg.timelineStartMs - fromMs) / 1000;
        if (sourceRemaining <= 0.01) continue;
        try {
          src.start(when, sourceOffset, sourceRemaining);
        } catch {
          continue;
        }
        this.nodes.push(src);
      }
    }
    this.started = true;
    this.paused = false;
  }

  /** Silence by stopping the scheduled sources. The AudioContext is NEVER
   *  suspended: cork/uncork toggles made KDE show the stream as muted and
   *  caused dropouts. */
  pause(): void {
    if (!this.started || this.paused) return;
    this.baseMs = this.currentTimeMs();
    this.stopNodes();
    this.paused = true;
  }

  /** Rebuild the schedule at `ms` (used by seeking/scrubbing). */
  async seek(
    ms: number,
    segments: Segment[],
    keepPlaying: boolean,
    gains: TrackGains,
  ): Promise<void> {
    this.stopNodes();
    this.started = false;
    this.paused = true;
    if (keepPlaying) {
      await this.play(segments, ms, gains);
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
  }

  dispose() {
    this.stopNodes();
    this.game.disconnect();
    this.mic.disconnect();
    this.master.disconnect();
    void this.ctx.close().catch(() => {});
    this.sources.clear();
  }
}

/** Logs through the same console the dev log shows. */
function eprintln(msg: string) {
  console.warn(`[moonclip] ${msg}`);
}
