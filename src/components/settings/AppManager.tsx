import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen, Plus, RefreshCw, Trash2 } from "lucide-react";
import { useCustomApps } from "../../hooks/useCustomApps";
import { exeBasename, registerInputFromCandidate } from "../../lib/detect";
import type { ResolvedCandidate } from "../../types";

/** Games: running detector picker + registered rows (edit/delete). */
export function AppManager() {
  const { t } = useTranslation();
  const { apps, loading, error, registerApp, updateApp, deleteApp } = useCustomApps();
  const [running, setRunning] = useState<ResolvedCandidate[]>([]);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [formError, setFormError] = useState<string | null>(null);

  const refreshRunning = useCallback(async () => {
    try {
      setRunning(await invoke<ResolvedCandidate[]>("get_running_applications"));
    } catch {
      setRunning([]);
    }
  }, []);

  useEffect(() => {
    void refreshRunning();
    const id = window.setInterval(() => void refreshRunning(), 5000);
    return () => window.clearInterval(id);
  }, [refreshRunning]);

  const addCandidate = async (g: ResolvedCandidate) => {
    setBusyKey(g.game_key);
    setFormError(null);
    try {
      await registerApp(registerInputFromCandidate(g));
      await refreshRunning();
    } catch (e) {
      setFormError(String(e));
    } finally {
      setBusyKey(null);
    }
  };

  const browse = async () => {
    setFormError(null);
    try {
      const picked = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "Executable", extensions: ["exe", "sh", "bin", "AppImage"] }],
      });
      if (typeof picked !== "string") return;
      const base = exeBasename(picked);
      await registerApp({
        display_name: base.replace(/\.(exe|sh|bin|AppImage)$/i, ""),
        target_exe: base,
        match_strategy: base.toLowerCase().endsWith(".exe") ? "wine_target" : "exact_exe",
      });
    } catch (e) {
      setFormError(String(e));
    }
  };

  const input =
    "rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-sm text-slate-100 outline-none focus:border-cyan-500/50";

  const icon = (iconPath?: string | null, name = "?") =>
    iconPath ? (
      <img
        src={convertFileSrc(iconPath)}
        alt=""
        className="h-8 w-8 shrink-0 rounded-lg object-cover"
      />
    ) : (
      <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-white/10 text-xs font-semibold text-slate-300">
        {name.trim().charAt(0).toUpperCase() || "?"}
      </span>
    );

  if (loading) return <p className="text-sm text-slate-400">{t("common.loading")}</p>;
  if (error) return <p className="text-sm text-red-400">{error}</p>;

  const registeredKeys = new Set(apps.map((a) => a.game_key).filter(Boolean) as string[]);

  return (
    <div className="max-w-3xl space-y-6">
      <section>
        <div className="mb-2 flex items-center gap-2">
          <h3 className="text-sm font-semibold text-slate-200">{t("games.running")}</h3>
          <button
            onClick={() => void refreshRunning()}
            className="ml-auto inline-flex items-center gap-1 text-xs text-slate-500 transition hover:text-slate-200"
          >
            <RefreshCw size={12} /> {t("games.refresh")}
          </button>
        </div>
        {running.length === 0 ? (
          <p className="text-xs text-slate-500">{t("games.empty")}</p>
        ) : (
          <ul className="space-y-1.5">
            {running.map((g) => {
              const already = g.registered || registeredKeys.has(g.game_key);
              return (
                <li
                  key={`${g.game_key}:${g.pid}`}
                  className="flex items-center gap-3 rounded-xl border border-white/5 bg-black/30 px-3 py-2"
                >
                  {icon(g.icon_path, g.title)}
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm text-slate-200">{g.title}</p>
                    <p className="truncate font-mono text-[11px] text-slate-500">
                      {exeBasename(g.exe)} · {g.source}
                      {g.is_wine ? " · wine" : ""} · {g.source_kind === "Window" ? "window" : "portal"}
                    </p>
                  </div>
                  {already ? (
                    <span className="text-xs text-cyan-300">{t("games.registered")}</span>
                  ) : (
                    <button
                      onClick={() => void addCandidate(g)}
                      disabled={busyKey === g.game_key}
                      className="inline-flex items-center gap-1.5 rounded-lg border border-cyan-500/30 bg-cyan-500/10 px-2.5 py-1 text-xs text-cyan-200 transition hover:bg-cyan-500/20 disabled:opacity-50"
                    >
                      <Plus size={13} /> {t("games.register_btn")}
                    </button>
                  )}
                </li>
              );
            })}
          </ul>
        )}
        <button
          onClick={() => void browse()}
          className="mt-2 inline-flex items-center gap-1.5 rounded-lg border border-white/10 px-2.5 py-1.5 text-xs text-slate-300 transition hover:bg-white/5"
        >
          <FolderOpen size={13} /> {t("games.browse")}
        </button>
      </section>

      <section>
        <h3 className="mb-2 text-sm font-semibold text-slate-200">{t("nav.games")}</h3>
        {apps.length === 0 ? (
          <p className="text-sm text-slate-500">{t("games.empty")}</p>
        ) : (
          <ul className="space-y-2">
            {apps.map((a) => (
              <li
                key={a.id}
                className="flex flex-col items-start gap-2 rounded-xl border border-white/5 bg-black/30 px-3 py-2.5 sm:flex-row sm:items-center sm:justify-between sm:gap-3 sm:px-4"
              >
                <div className="flex w-full min-w-0 items-center gap-3 sm:w-auto">
                  {icon(a.icon_path, a.display_name)}
                  <div className="min-w-0 text-sm">
                    <p className="truncate font-medium text-slate-200">{a.display_name}</p>
                    <p className="break-all font-mono text-xs text-slate-500">
                      {a.target_exe || a.game_key} · {a.match_strategy}
                    </p>
                  </div>
                </div>
                <div className="flex w-full items-center gap-3 sm:w-auto">
                  <input
                    type="number"
                    min={5}
                    max={3600}
                    placeholder={t("games.duration_ph")}
                    defaultValue={a.clip_duration_seconds ?? ""}
                    onBlur={(e) => {
                      const raw = e.target.value.trim();
                      const duration = raw === "" ? null : Math.max(5, Math.min(3600, Number(raw)));
                      if (duration !== null && !Number.isFinite(duration)) return;
                      void updateApp(a.id, a.display_name, duration, a.auto_buffer);
                    }}
                    className={`${input} w-20`}
                  />
                  <label className="inline-flex items-center gap-1.5 text-xs text-slate-400">
                    <input
                      type="checkbox"
                      checked={a.auto_buffer}
                      onChange={(e) =>
                        void updateApp(
                          a.id,
                          a.display_name,
                          a.clip_duration_seconds ?? null,
                          e.target.checked,
                        )
                      }
                    />
                    {t("games.auto_buffer")}
                  </label>
                  <button
                    onClick={() => void deleteApp(a.id)}
                    className="rounded-lg p-1.5 text-slate-500 transition hover:bg-red-500/20 hover:text-red-300"
                    title={t("common.delete")}
                  >
                    <Trash2 size={15} />
                  </button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>

      {formError && <p className="font-mono text-xs text-red-400">{formError}</p>}
      <p className="text-xs text-slate-600">{t("games.picker_note")}</p>
    </div>
  );
}
