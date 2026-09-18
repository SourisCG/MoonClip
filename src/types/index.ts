export interface HotkeyEvent {
  shortcut: string;
  pressed_at: string;
}

export interface Phase1Status {
  hotkey: string;
  presses: number;
  lastPress: string | null;
}

/** Mirrors Rust ClipRecord. file_name is RELATIVE to the clips directory. */
export interface ClipMetadata {
  id: string;
  file_name: string;
  thumbnail_name: string;
  game_title: string;
  duration_ms: number;
  file_size_bytes: number;
  created_at: string;
  is_favorite: boolean;
  drive_file_id?: string | null;
  drive_web_url?: string | null;
  /** Computed: file still on disk? */
  exists: boolean;
}

export type AppSettings = Record<string, string>;

/** Mirrors Rust CustomApp. */
export interface CustomApp {
  id: string;
  display_name: string;
  target_exe: string;
  match_strategy: string;
  clip_duration_seconds?: number | null;
  icon_path?: string | null;
  is_wine_proton: boolean;
}

export interface RegisterAppInput {
  display_name: string;
  target_exe: string;
  match_strategy: string;
  clip_duration_seconds?: number | null;
  is_wine_proton?: boolean | null;
}

/** Mirrors Rust EngineStatus (payload keys stay snake_case on the wire). */
export interface EngineStatus {
  running: boolean;
  backend: string;
  tracks_linked: number;
  audio_error: string | null;
  engine_error: string | null;
}

/** Mirrors Rust CodecOpt. */
export interface CodecOpt {
  id: string;
}

/** Mirrors Rust HeightOpt. */
export interface HeightOpt {
  height: number;
  label: string;
  /** CBR kbps per codec, in codec order. */
  bitrates: number[];
  /** Exact 60 s ring megabytes per codec, in codec order. */
  ring_mb_60s: number[];
}

/** Mirrors Rust MonitorOpt. */
export interface MonitorOpt {
  name: string;
  label: string;
}

/** Mirrors Rust VideoOptions. */
export interface VideoOptions {
  codecs: CodecOpt[];
  heights: HeightOpt[];
  monitors: MonitorOpt[];
  current_codec: string;
  current_height: number;
  current_fps: number;
  current_monitor: string;
  buffer_height: number;
  transcoding: boolean;
  max_source_height: number;
  vendor: string;
}

/** Mirrors Rust GsrInfo (Linux-only fields; harmless on Windows). */
export interface GsrInfo {
  path: string;
  source: string;
  caps_ok: boolean;
  present: boolean;
}

/** Mirrors Rust AudioDevice. */
export interface AudioDevice {
  id: string;
  description: string;
  kind: string;
}

/** Mirrors Rust AudioPeaks. */
export interface AudioPeaks {
  game: number;
  mic: number;
}

/** Mirrors Rust TrackGains. */
export interface TrackGains {
  game: number;
  mic: number;
  mute_game: boolean;
  mute_mic: boolean;
}

/** Mirrors Rust SystemMemory. */
export interface SystemMemory {
  free_mb: number | null;
}

/** Mirrors Rust SettingPair for the bulk `set_settings` command. */
export interface SettingPair {
  key: string;
  value: string;
}
