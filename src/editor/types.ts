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

export interface Overlay {
  id: string;
  kind: "text" | "sticker" | "gif" | "image";
  text?: string | null;
  asset?: string | null;
  startMs: number;
  durationMs: number;
  x: number;
  y: number;
  scale: number;
  rotation: number;
  opacity: number;
  fontSize: number;
  color: string;
  strokeColor: string;
  strokeWidth: number;
  shadow: boolean;
  align: string;
}

export interface EditProject {
  version: number;
  id: string;
  name: string;
  sourceClipId: string;
  /** Track mix: master scales the stem mix; Game/Mic are the stems. Track 1
   *  of the recording is the SUM of both and is never played. */
  gainMaster: number;
  gainGame: number;
  gainMic: number;
  output: OutputSettings;
  segments: Segment[];
  audioTracks: AudioTrack[];
  overlays: Overlay[];
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

export interface EditorSourceInfo {
  clipId: string;
  fileName: string;
  gameTitle: string;
  durationMs: number;
  width: number;
  height: number;
  fps: number;
  codec: string;
  videoUrl: string;
  usingProxy: boolean;
  stems: EditorAudioTrack[];
}

export interface EditorOpenResult {
  sessionId: string;
  project: EditProject;
  clip: EditorClipInfo;
  videoUrl: string;
  usingProxy: boolean;
  audioTracks: EditorAudioTrack[];
  sources: EditorSourceInfo[];
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

export function defaultTextOverlay(startMs: number, durationMs: number): Overlay {
  return {
    id: crypto.randomUUID(),
    kind: "text",
    text: "Texto",
    asset: null,
    startMs: Math.max(0, Math.round(startMs)),
    durationMs: Math.max(300, Math.round(durationMs)),
    x: 0.5,
    y: 0.78,
    scale: 1,
    rotation: 0,
    opacity: 1,
    fontSize: 64,
    color: "#ffffff",
    strokeColor: "#000000",
    strokeWidth: 3,
    shadow: true,
    align: "center",
  };
}

export function projectDurationMs(p: EditProject): number {
  return p.segments.reduce(
    (max, s) => Math.max(max, s.timelineStartMs + segmentDurationMs(s)),
    0,
  );
}
