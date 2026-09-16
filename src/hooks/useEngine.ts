import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ClipMetadata } from "../types";

export interface EngineStatus {
  running: boolean;
  backend: string;
  tracks_linked: number;
  audio_error: string | null;
  engine_error: string | null;
}

export function useEngine(onClipSaved: () => void) {
  const [status, setStatus] = useState<EngineStatus>({
    running: false,
    backend: "",
    tracks_linked: 0,
    audio_error: null,
    engine_error: null,
  });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setStatus(await invoke<EngineStatus>("engine_status"));
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    // Live link status while recording (streams appear async after spawn).
    const poll = window.setInterval(() => {
      invoke<EngineStatus>("engine_status").then(setStatus).catch(() => {});
    }, 2000);
    let unlisten: (() => void) | undefined;
    let unlistenStopped: (() => void) | undefined;
    (async () => {
      try {
        unlisten = await listen("moonclip://clip-saved", () => onClipSaved());
        unlistenStopped = await listen("moonclip://engine-stopped", () => {
          void refresh();
        });
      } catch (err) {
        console.error(err);
      }
    })();
    return () => {
      window.clearInterval(poll);
      unlisten?.();
      unlistenStopped?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refresh]);

  const start = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      setStatus(await invoke<EngineStatus>("start_buffer"));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  const stop = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      setStatus(await invoke<EngineStatus>("stop_buffer"));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  const saveNow = useCallback(async (): Promise<ClipMetadata | null> => {
    setBusy(true);
    setError(null);
    try {
      const clip = await invoke<ClipMetadata>("save_clip_now");
      onClipSaved();
      return clip;
    } catch (e) {
      setError(String(e));
      return null;
    } finally {
      setBusy(false);
    }
  }, [onClipSaved]);

  return { status, busy, error, start, stop, saveNow, refresh };
}
