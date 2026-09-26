import type { RunningApp } from "../types";

/** Mirrors Rust RegisteredInput (one OBS source per registered app). */
export interface RegisteredInput {
  id: string;
  input_name: string;
  input_kind: "window" | "screen";
  display_name: string;
  target_exe: string;
  match_strategy: string;
  input_settings?: string | null;
  source_uuid: string;
  icon_path?: string | null;
}

/** Mirrors Rust RunningApp (short, filtered list for registration). */
export type { RunningApp };
