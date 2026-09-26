import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { RegisteredInput } from "../types/registered";

/** Registered capture inputs (one OBS source per app). */
export function useRegisteredInputs() {
  const [inputs, setInputs] = useState<RegisteredInput[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setInputs(await invoke<RegisteredInput[]>("list_registered_inputs"));
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const add = useCallback(
    async (displayName: string, targetExe: string, matchStrategy: string) => {
      const input = await invoke<RegisteredInput>("add_registered_input", {
        displayName,
        targetExe,
        matchStrategy,
      });
      await refresh();
      return input;
    },
    [refresh],
  );

  const remove = useCallback(
    async (id: string) => {
      await invoke("delete_registered_input", { id });
      await refresh();
    },
    [refresh],
  );

  return { inputs, loading, error, refresh, add, remove };
}
