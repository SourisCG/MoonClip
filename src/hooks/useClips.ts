import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ClipMetadata } from "../types";

export function useClips() {
  const [clips, setClips] = useState<ClipMetadata[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setClips(await invoke<ClipMetadata[]>("list_clips"));
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const toggleFavorite = useCallback(
    async (id: string) => {
      await invoke<boolean>("toggle_favorite", { id });
      await refresh();
    },
    [refresh],
  );

  const deleteClip = useCallback(
    async (id: string) => {
      await invoke("delete_clip", { id });
      await refresh();
    },
    [refresh],
  );

  const purgeMissing = useCallback(async (): Promise<number> => {
    const n = await invoke<number>("purge_missing_clips");
    await refresh();
    return n;
  }, [refresh]);

  const renameClip = useCallback(
    async (id: string, name: string) => {
      await invoke<ClipMetadata>("rename_clip", { id, name });
      await refresh();
    },
    [refresh],
  );

  return {
    clips,
    loading,
    error,
    refresh,
    toggleFavorite,
    deleteClip,
    purgeMissing,
    renameClip,
  };
}
