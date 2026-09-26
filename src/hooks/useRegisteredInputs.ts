import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { RegisteredInput } from "../types/registered";

/** Registered games (one OBS window input each; the screen input is hidden). */
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

  /** Opens the system picker and registers the picked window as a game. */
  const register = useCallback(async () => {
    const input = await invoke<RegisteredInput>("register_game");
    await refresh();
    return input;
  }, [refresh]);

  /** Re-opens the picker for an existing game (choose another window). */
  const edit = useCallback(
    async (id: string) => {
      const input = await invoke<RegisteredInput>("edit_game", { id });
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

  return { inputs, loading, error, refresh, register, edit, remove };
}
