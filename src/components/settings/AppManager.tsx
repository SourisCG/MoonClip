import { useState } from "react";
import { useTranslation } from "react-i18next";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Plus, Trash2 } from "lucide-react";
import { useCustomApps } from "../../hooks/useCustomApps";

const STRATEGIES = ["exact_exe", "cmdline_contains", "window_title", "wine_target"];

/** Phase 2 proof UI. Full process picker arrives in Phase 4. */
export function AppManager() {
  const { t } = useTranslation();
  const { apps, loading, error, registerApp, deleteApp } = useCustomApps();
  const [name, setName] = useState("");
  const [exe, setExe] = useState("");
  const [strategy, setStrategy] = useState(STRATEGIES[0]);
  const [formError, setFormError] = useState<string | null>(null);

  const submit = async () => {
    setFormError(null);
    try {
      await registerApp({
        display_name: name.trim(),
        target_exe: exe.trim(),
        match_strategy: strategy,
      });
      setName("");
      setExe("");
    } catch (e) {
      setFormError(String(e));
    }
  };

  const input =
    "rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-sm text-slate-100 outline-none focus:border-cyan-500/50";

  if (loading) return <p className="text-sm text-slate-400">{t("common.loading")}</p>;
  if (error) return <p className="text-sm text-red-400">{error}</p>;

  return (
    <div className="max-w-2xl space-y-4">
      <div className="flex flex-wrap items-end gap-2 rounded-xl border border-white/5 bg-black/30 p-3">
        <input
          className={`${input} w-full sm:flex-1`}
          placeholder={t("games.name_ph")}
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <input
          className={`${input} w-full sm:flex-1`}
          placeholder={t("games.exe_ph")}
          value={exe}
          onChange={(e) => setExe(e.target.value)}
        />
        <select className={`${input} w-full sm:w-auto`} value={strategy} onChange={(e) => setStrategy(e.target.value)}>
          {STRATEGIES.map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </select>
        <button
          onClick={() => void submit()}
          className="inline-flex w-full items-center justify-center gap-1.5 rounded-lg border border-cyan-500/30 bg-cyan-500/10 px-3 py-1.5 text-sm text-cyan-200 transition hover:bg-cyan-500/20 sm:w-auto"
        >
          <Plus size={14} /> {t("games.add")}
        </button>
      </div>
      {formError && <p className="font-mono text-xs text-red-400">{formError}</p>}
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
                {a.icon_path ? (
                  <img
                    src={convertFileSrc(a.icon_path)}
                    alt=""
                    className="h-8 w-8 shrink-0 rounded-lg object-cover"
                  />
                ) : (
                  <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-white/10 text-xs font-semibold text-slate-300">
                    {a.display_name.trim().charAt(0).toUpperCase() || "?"}
                  </span>
                )}
                <div className="min-w-0 text-sm">
                  <p className="truncate font-medium text-slate-200">{a.display_name}</p>
                  <p className="break-all font-mono text-xs text-slate-500">
                    {a.target_exe || a.game_key} · {a.match_strategy}
                    {a.clip_duration_seconds ? ` · ${a.clip_duration_seconds}s` : ""}
                  </p>
                </div>
              </div>
              <button
                onClick={() => void deleteApp(a.id)}
                className="rounded-lg p-1.5 text-slate-500 transition hover:bg-red-500/20 hover:text-red-300"
                title={t("common.delete")}
              >
                <Trash2 size={15} />
              </button>
            </li>
          ))}
        </ul>
      )}
      <p className="text-xs text-slate-600">{t("games.picker_note")}</p>
    </div>
  );
}
