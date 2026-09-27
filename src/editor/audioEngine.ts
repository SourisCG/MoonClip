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

import { invoke } from "@tauri-apps/api/core";
import type { EditorSourceInfo, Segment } from "./types";

/** Diagnostics to the app log (visible in the dev terminal). */
function diag(message: string) {
  void invoke("editor_log", { message }).catch(() => {});
}

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

export class AudioTimeline {
  private ctx: AudioContext;
  private master: GainNode;
  private game: GainNode;
  private mic: GainNode;
  private analyserGame: AnalyserNode;
  private analyserMic: AnalyserNode;
  /** Post-master tap: proves what actually leaves the engine. */
  private analyserOut: AnalyserNode;
  private levelBuf: Float32Array<ArrayBuffer>;
  private sources = new Map<string, DecodedSource>();
  private nodes: AudioBufferSourceNode[] = [];
  /** Live per-segment gain nodes so each clip is mixed independently. */
  private segGains: { segId: string; kind: "mix" | "game" | "mic"; node: GainNode }[] = [];
  /** Last applied gain signature (diagnostics: log only real changes). */
  private lastGainsSig = "";

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
    // Post-master tap: measures exactly what leaves the engine.
    this.analyserOut = this.ctx.createAnalyser();
    this.analyserOut.fftSize = 256;
    this.master.connect(this.analyserOut);
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

  /** Live RMS per track (0..1) for the level meters. `out` is measured AFTER
   *  the master gain, i.e. what actually reaches the system output. */
  levels(): { game: number; mic: number; out: number } {
    const rms = (a: AnalyserNode) => {
      a.getFloatTimeDomainData(this.levelBuf);
      let sum = 0;
      for (let i = 0; i < this.levelBuf.length; i++) sum += this.levelBuf[i] * this.levelBuf[i];
      return Math.sqrt(sum / this.levelBuf.length);
    };
    // Channel meters are PRE-master: they must move with their slider even
    // when the master is muted (they used to be multiplied by it and went
    // dead). `out` is post-master: what leaves the engine.
    return {
      game: rms(this.analyserGame),
      mic: rms(this.analyserMic),
      out: rms(this.analyserOut),
    };
  }

  get sampleRate(): number {
    return this.ctx.sampleRate;
  }

  /** Decode a source's stems once (cached). `null` tracks failed to decode. */
  async ensureSource(source: EditorSourceInfo): Promise<DecodedSource> {
    const cached = this.sources.get(source.clipId);
    if (cached) return cached;
    const stems: DecodedSource["stems"] = [];
    diag(`source ${source.clipId}: ${source.stems.length} stems (${source.stems
      .map((s) => s.label)
      .join("+")})`);
    for (const stem of source.stems) {
      const buffer = await this.decode(stem.url);
      if (buffer) {
        diag(
          `stem ${stem.label} decoded: ${buffer.duration.toFixed(2)}s ` +
            `${buffer.numberOfChannels}ch ${buffer.sampleRate}Hz`,
        );
        stems.push({ label: stem.label, buffer, peaks: computePeaks(buffer) });
      } else {
        diag(`stem ${stem.label} FAILED to decode (${stem.url})`);
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

  /** Dev diagnostic: proves GainNode automation works in this WebKit build.
   *  Fully silent (the tone goes through a -80 dB sink) and logs the measured
   *  RMS at gain 0.1 / 1.0 / 0.0. */
  async gainSelfTest(): Promise<void> {
    const ctx = this.ctx;
    await ctx.resume();
    const osc = ctx.createOscillator();
    osc.frequency.value = 440;
    const gain = ctx.createGain();
    const an = ctx.createAnalyser();
    an.fftSize = 1024;
    const sink = ctx.createGain();
    sink.gain.value = 0.0001;
    osc.connect(gain);
    gain.connect(an);
    gain.connect(sink);
    sink.connect(ctx.destination);
    const buf = new Float32Array(an.fftSize);
    const rms = () => {
      an.getFloatTimeDomainData(buf);
      let s = 0;
      for (const v of buf) s += v * v;
      return Math.sqrt(s / buf.length);
    };
    const wait = () => new Promise((r) => setTimeout(r, 150));
    osc.start();
    gain.gain.setValueAtTime(0.1, ctx.currentTime);
    await wait();
    const low = rms();
    gain.gain.setValueAtTime(1.0, ctx.currentTime);
    await wait();
    const high = rms();
    gain.gain.setValueAtTime(0.0, ctx.currentTime);
    await wait();
    const zero = rms();
    try {
      osc.stop();
    } catch {
      /* not started twice */
    }
    osc.disconnect();
    gain.disconnect();
    sink.disconnect();
    diag(
      `gainSelfTest low=${low.toFixed(3)} high=${high.toFixed(3)} zero=${zero.toFixed(3)}`,
    );
  }

  /** Global output level. Shared GainNode: no reschedule is ever needed.
   *  ALWAYS apply, never compare with `gain.value`: in WebKit that getter
   *  does not reflect values scheduled with setValueAtTime, so a
   *  `value !== target` guard silently skipped restoring the master (or a
   *  channel) after it had been lowered. */
  setMaster(value: number) {
    const v = Math.max(0, Math.min(4, value));
    this.master.gain.setValueAtTime(v, this.ctx.currentTime);
    diag(`master=${v.toFixed(2)}`);
  }

  /** Silent routing probe: plays the real segments at -60 dB and measures
   *  the post-master output with the per-clip gains on, muted and restored.
   *  Proves the slider->node->bus->master path end to end without being
   *  audible. */
  async gainPathProbe(segments: Segment[], master: number): Promise<void> {
    const first = segments.slice().sort((a, b) => a.timelineStartMs - b.timelineStartMs)[0];
    if (!first) return;
    // Start where the source actually has signal (scanning its peaks), or the
    // probe would measure silence and prove nothing.
    let fromMs = first.timelineStartMs;
    const src = this.sources.get(first.sourceClipId);
    const stem = src?.stems.find((s) => s.label === "game") ?? src?.stems[0];
    if (stem) {
      const per = stem.peaks[0]?.length ? stem.peaks[0].length / 2 : 0;
      const srcDur = Math.max(1, src?.durationMs ?? 1);
      for (let b = 0; b < per; b++) {
        const amp = Math.max(
          Math.abs(stem.peaks[0][b * 2] ?? 0),
          Math.abs(stem.peaks[0][b * 2 + 1] ?? 0),
        );
        if (amp > 0.2) {
          fromMs = first.timelineStartMs + first.inMs + (b / per) * srcDur;
          break;
        }
      }
    }
    await this.play(segments, fromMs, 0.01);
    await new Promise((r) => setTimeout(r, 450));
    const on = this.levels().out;
    const muted = segments.map((s) => ({ ...s, gainMix: 0, gainGame: 0, gainMic: 0 }));
    this.applySegmentGains(muted);
    await new Promise((r) => setTimeout(r, 300));
    const off = this.levels().out;
    this.applySegmentGains(segments);
    await new Promise((r) => setTimeout(r, 300));
    const back = this.levels().out;
    this.stopNodes();
    this.started = false;
    this.paused = true;
    this.setMaster(master);
    diag(`gainPath on=${on.toFixed(4)} off=${off.toFixed(4)} back=${back.toFixed(4)}`);
  }

  /** Length of the decoded stems (ms), if this source was decoded. */
  durationMsFor(clipId: string): number | null {
    const buf = this.sources.get(clipId)?.stems[0]?.buffer;
    return buf ? Math.round(buf.duration * 1000) : null;
  }

  /** Reflect per-clip slider moves on the already-scheduled nodes.
   *  `setValueAtTime` is used instead of the `.value` setter: WebKit applies
   *  it deterministically while the context is running. */
  applySegmentGains(segments: Segment[]) {
    if (this.segGains.length === 0) return;
    const byId = new Map(segments.map((s) => [s.id, s]));
    let sig = "";
    for (const g of this.segGains) {
      const seg = byId.get(g.segId);
      if (!seg) continue;
      const value = Math.max(
        0,
        Math.min(4, g.kind === "mic" ? seg.gainMic : g.kind === "game" ? seg.gainGame : seg.gainMix),
      );
      // Always apply: WebKit's `gain.value` getter lags scheduled changes, so
      // a `value !== target` guard skipped restoring a lowered channel.
      g.node.gain.setValueAtTime(value, this.ctx.currentTime);
      sig += `${g.segId.slice(0, 4)}/${g.kind}=${value.toFixed(2)} `;
    }
    if (sig && sig !== this.lastGainsSig) {
      this.lastGainsSig = sig;
      diag(`applyGains ${sig.trim()}`);
    }
  }

  /** Schedule every segment from `fromMs` and start the clock. Track 1 of a
   *  recording is the sum of Game+Mic, so only the stems are scheduled. Each
   *  clip's own gains (and the global master) are applied on its own nodes. */
  async play(segments: Segment[], fromMs: number, master: number): Promise<void> {
    this.stopNodes();
    await this.ctx.resume();
    this.setMaster(master);
    const lookahead = 0.06;
    this.t0 = this.ctx.currentTime + lookahead;
    this.baseMs = fromMs;
    for (const seg of segments) {
      const decoded = this.sources.get(seg.sourceClipId);
      if (!decoded || decoded.stems.length === 0) continue;
      const segEnd =
        seg.timelineStartMs + Math.max(0, seg.outMs - seg.inMs) / Math.max(0.05, seg.speed);
      if (segEnd <= fromMs) continue;
      diag(
        `seg ${seg.id.slice(0, 8)} src=${seg.sourceClipId.slice(0, 8)} ` +
          `tl=${Math.round(seg.timelineStartMs)} in=${Math.round(seg.inMs)} ` +
          `out=${Math.round(seg.outMs)} speed=${seg.speed} ` +
          `g=${seg.gainGame} m=${seg.gainMic} mix=${seg.gainMix}`,
      );
      // ALL channels play, each through its own per-clip gain: Mix, Game and
      // Mic are independent sliders (the user mixes what they want to hear).
      for (const stem of decoded.stems) {
        const kind: "mix" | "game" | "mic" =
          stem.label === "mic"
            ? "mic"
            : stem.label === "game"
              ? "game"
              : stem.label === "mix"
                ? "mix"
                : "mix";
        const value =
          kind === "mic" ? seg.gainMic : kind === "game" ? seg.gainGame : seg.gainMix;
        const target =
          kind === "mic" ? this.mic : kind === "game" ? this.game : this.master;
        const src = this.ctx.createBufferSource();
        src.buffer = stem.buffer;
        src.playbackRate.value = seg.speed;
        const gainNode = this.ctx.createGain();
        gainNode.gain.setValueAtTime(Math.max(0, Math.min(4, value)), this.ctx.currentTime);
        src.connect(gainNode);
        gainNode.connect(target);
        this.segGains.push({ segId: seg.id, kind, node: gainNode });

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
    diag(
      `play from ${Math.round(fromMs)}ms: scheduled ${this.nodes.length} node(s), ` +
        `master=${master}`,
    );
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
    master: number,
  ): Promise<void> {
    this.stopNodes();
    this.started = false;
    this.paused = true;
    if (keepPlaying) {
      await this.play(segments, ms, master);
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
    for (const g of this.segGains) g.node.disconnect();
    this.segGains = [];
    this.lastGainsSig = "";
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
