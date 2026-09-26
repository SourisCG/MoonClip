import type { RegisterAppInput, ResolvedCandidate } from "../types";

/** `window_match` is encoded as `id\r\nname\r\nclass` for xcomposite. */
export function windowMatchName(match?: string | null): string {
  if (!match) return "";
  const parts = match.split("\r\n");
  return parts.length >= 3 ? parts[1] : (parts[0] ?? "");
}

export function exeBasename(exe: string): string {
  return exe.split(/[\\/]/).pop() ?? exe;
}

/** Map a detected candidate to the one-time registration payload. */
export function registerInputFromCandidate(g: ResolvedCandidate): RegisterAppInput {
  const base = exeBasename(g.exe);
  const target = g.is_wine ? base : g.window_match ? windowMatchName(g.window_match) : base;
  const strategy = g.is_wine ? "wine_target" : g.window_match ? "window_title" : "exact_exe";
  return {
    display_name: g.title,
    target_exe: target,
    match_strategy: strategy,
    game_key: g.game_key,
    source_kind: g.source_kind === "X11" ? "x11" : "portal",
    window_match: g.window_match ?? null,
    auto_buffer: true,
  };
}
