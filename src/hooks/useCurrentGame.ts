import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ResolvedCandidate } from "../types";

/** Last game picked by the detection worker (event-driven, IPC seed). */
export function useCurrentGame() {
  const [game, setGame] = useState<ResolvedCandidate | null>(null);

  useEffect(() => {
    let alive = true;
    void invoke<ResolvedCandidate | null>("current_game")
      .then((g) => {
        if (alive) setGame(g ?? null);
      })
      .catch(() => {});
    let unlisten: (() => void) | undefined;
    void listen<ResolvedCandidate | null>("moonclip://game-changed", (event) => {
      if (alive) setGame(event.payload ?? null);
    }).then((u) => {
      unlisten = u;
      if (!alive) u();
    });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  return game;
}
