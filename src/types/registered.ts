/** Mirrors Rust RegisteredInput (one OBS source per registered game). */
export interface RegisteredInput {
  id: string;
  input_name: string;
  input_kind: "window" | "screen";
  display_name: string;
  /** Window title learned from the picker (autopilot matches by it). */
  window_title?: string | null;
  /** Window app id/class when the platform reports one (KDE). */
  window_app_id?: string | null;
  target_exe: string;
  input_settings?: string | null;
  source_uuid: string;
  icon_path?: string | null;
}
