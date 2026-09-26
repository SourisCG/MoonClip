import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { CustomApp, RegisterAppInput } from "../types";

export function useCustomApps() {
  const [apps, setApps] = useState<CustomApp[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setApps(await invoke<CustomApp[]>("list_custom_apps"));
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const registerApp = useCallback(
    async (input: RegisterAppInput) => {
      const app = await invoke<CustomApp>("register_app", { input });
      await refresh();
      return app;
    },
    [refresh],
  );

  const updateApp = useCallback(
    async (
      id: string,
      displayName: string,
      clipDurationSeconds: number | null,
      autoBuffer: boolean,
    ) => {
      await invoke("update_custom_app", {
        id,
        displayName,
        clipDurationSeconds,
        autoBuffer,
      });
      await refresh();
    },
    [refresh],
  );

  const deleteApp = useCallback(
    async (id: string) => {
      await invoke("delete_app", { id });
      await refresh();
    },
    [refresh],
  );

  return { apps, loading, error, refresh, registerApp, updateApp, deleteApp };
}
