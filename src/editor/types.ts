/** Mirrors the Rust editor DTOs (camelCase wire keys). */

export interface OutputSettings {
  height: number;
  aspect: string;
  fps: number;
  bitrateKbps: number;
  encoder: string;
  container: string;
}

export interface Segment {
  id: string;
  sourceClipId: string;
  inMs: number;
  outMs: number;
  timelineStartMs: number;
  speed: number;
  freezeAtMs: number;
  freezeMs: number;
  gainMix: number;
  gainGame: number;
  gainMic: number;
}

export interface AudioTrack {
  id: string;
  fileName: string;
  startMs: number;
  durationMs: number;
  gain: number;
  loopPlayback: boolean;
}

export interface EditProject {
  version: number;
  id: string;
  name: string;
  sourceClipId: string;
  output: OutputSettings;
  segments: Segment[];
  audioTracks: AudioTrack[];
  overlays: unknown[];
  updatedAt: string;
}

export interface EditorClipInfo {
  id: string;
  fileName: string;
  gameTitle: string;
  durationMs: number;
  width: number;
  height: number;
  fps: number;
  codec: string;
}

export interface EditorAudioTrack {
  label: string;
  url: string;
}

export interface EncoderInfo {
  id: string;
  label: string;
  hw: boolean;
  available: boolean;
}

export interface EditorOpenResult {
  sessionId: string;
  project: EditProject;
  clip: EditorClipInfo;
  videoUrl: string;
  usingProxy: boolean;
  audioTracks: EditorAudioTrack[];
  encoders: EncoderInfo[];
}

export interface EditProgress {
  op: string;
  clipId: string;
  sessionId?: string;
  stage?: string;
  percent: number;
  done: boolean;
}

export function segmentDurationMs(s: Segment): number {
  const base = Math.max(0, s.outMs - s.inMs) / Math.max(0.05, s.speed);
  return Math.round(base + Math.max(0, s.freezeMs));
}

export function projectDurationMs(p: EditProject): number {
  return p.segments.reduce(
    (max, s) => Math.max(max, s.timelineStartMs + segmentDurationMs(s)),
    0,
  );
}
