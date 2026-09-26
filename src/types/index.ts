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

/** Mirrors Rust ResolvedCandidate (game detection). */
export interface ResolvedCandidate {
  pid: number;
  exe: string;
  comm: string;
  title: string;
  game_key: string;
  source: string;
  uses_gpu: boolean;
  is_wine: boolean;
  steam_app_id?: number | null;
  window_match?: string | null;
  source_kind: "Window" | "Portal";
  registered: boolean;
  custom_id?: string | null;
  auto_buffer: boolean;
  clip_duration_seconds?: number | null;
  icon_path?: string | null;
}

/** Mirrors Rust CustomApp. */
export interface CustomApp {
  id: string;
  display_name: string;
  target_exe: string;
  match_strategy: string;
  clip_duration_seconds?: number | null;
  icon_path?: string | null;
  is_wine_proton: boolean;
  game_key?: string | null;
  capture_mode: string;
  source_kind?: string | null;
  window_match?: string | null;
  portal_token?: string | null;
  auto_buffer: boolean;
  last_seen_ms?: number | null;
}

export interface RegisterAppInput {
  display_name: string;
  target_exe: string;
  match_strategy: string;
  clip_duration_seconds?: number | null;
  is_wine_proton?: boolean | null;
  game_key?: string | null;
  source_kind?: string | null;
  window_match?: string | null;
  auto_buffer?: boolean | null;
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
  /** Medal recommended (min,max) kbps per codec, in codec order. */
  recommended: [number, number][];
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
  /** Wayland portal: a persisted screen choice exists (silent restore). */
  portal_ready: boolean;
  buffer_height: number;
  transcoding: boolean;
  max_source_height: number;
  vendor: string;
  /** Encoder preference: gpu | cpu. */
  encoder: string;
  /** Hardware monitors can be oversampled (24..144). */
  fps_options: number[];
  /** Custom picker: pinned catalog entries with live availability. */
  encoders: EncoderOpt[];
  /** Custom form: full option schema per encoder family. */
  encoder_schema: EncoderSchema[];
  /** Color formats each catalog encoder accepts. */
  encoder_colors: EncoderColors[];
  /** [Video] tab enums. */
  scale_filters: string[];
  fps_types: string[];
  fps_common_values: number[];
  color_spaces: string[];
  color_ranges: string[];
  /** Current Custom selection (null when ladder). */
  custom: CustomSelection | null;
}

/** Mirrors Rust EncoderOpt. */
export interface EncoderOpt {
  id: string;
  codec: string;
  family: string;
  validated: boolean;
  available: boolean;
}

/** Mirrors Rust OptionSpecJson (resolved for one encoder). */
export interface OptionSpecJson {
  key: string;
  kind: "enum" | "int" | "bool" | "text";
  values: string[];
  int_values: number[];
  min: number;
  max: number;
  step: number;
  default: string | number | boolean | null;
  i18n: string;
  visible_when: { key: string; values: string[]; and_key: string | null; and_values: string[] } | null;
  /** False when the option does not exist for this codec: render greyed out, never send. */
  supported: boolean;
  /** Values only valid with P010 color format (greyed out otherwise). */
  p010_values: string[];
  p010_ints: number[];
}

/** Mirrors Rust EncoderSchema (per encoder id, codec already resolved). */
export interface EncoderSchema {
  encoder: string;
  family: string;
  codec: string;
  validated: boolean;
  options: OptionSpecJson[];
}

/** Mirrors Rust EncoderColors. */
export interface EncoderColors {
  id: string;
  formats: string[];
}

/** Mirrors Rust CustomSelection. */
export interface CustomSelection {
  encoder: string;
  settings: Record<string, string | number | boolean>;
  video: CustomVideo | null;
}

/** Mirrors Rust CustomVideo. */
export interface CustomVideo {
  out_width: number;
  out_height: number;
  scale_type: string;
  fps_type: string;
  fps_common: number;
  fps_int: number;
  fps_num: number;
  fps_den: number;
  color_format: string;
  color_space: string;
  color_range: string;
}

/** Mirrors Rust VideoProbe. */
export interface VideoProbe {
  codec_name: string;
  profile: string;
  width: number;
  height: number;
  fps: number;
}

/** Mirrors Rust ObsInfo (embedded, isolated OBS engine). */
export interface ObsInfo {
  present: boolean;
  version: string;
  config_dir: string;
  profile: string;
  collection: string;
  websocket_port: number;
  source: string;
  events_tail: string[];
}

/** Mirrors Rust HardwareTestResult. */
export interface HardwareTestResult {
  ok: boolean;
  height: number;
  fps: number;
  codec: string;
  encoder: string;
  bitrate_kbps: number;
  requested_seconds: number;
  startup_ms: number;
  measured_duration_ms: number;
  size_bytes: number;
  fallback_height: number | null;
  fallback_fps: number | null;
  probe: VideoProbe | null;
  error: string | null;
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
