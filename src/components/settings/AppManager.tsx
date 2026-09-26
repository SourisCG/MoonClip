import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { Check, Gamepad2, Monitor, Plus, RefreshCw, Trash2, Video } from "lucide-react";
import { useRegisteredInputs } from "../../hooks/useRegisteredInputs";
import type { RegisteredInput } from "../../types/registered";
import type { RunningApp } from "../../types";

const STRATEGIES = ["exact_exe", "cmdline_contains", "window_title", "wine_target"];

/** Capture registry: one OBS window/screen input per app, picker once. */
export function AppManager() {
  const { t } = useTranslation();
  const { inputs, loading, error, refresh, add, remove } = useRegisteredInputs();
  const [name, setName] = useState("");
  const [exe, setExe] = useState("");
  const [strategy, setStrategy] = useState(STRATEGIES[0]);
  const [formError, setFormError] = useState<string | null>(null);
  const [running, setRunning] = useState<RunningApp[]>([]);
  const [query, setQuery] = useState("");
  const [setupName, setSetupName] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refreshRunning = useCallback(async () => {
    try {
      setRunning(await invoke<RunningApp[]>("running_apps"));
    } catch {
      setRunning([]);
    }
  }, []);

  useEffect(() => {
    void refreshRunning();
    const id = window.setInterval(() => void refreshRunning(), 5000);
    return () => window.clearInterval(id);
  }, [refreshRunning]);

  const addRunning = async (r: RunningApp) => {
    setFormError(null);
    try {
      await add(r.name, r.exe, r.exe.toLowerCase().endsWith(".exe") ? "wine_target" : "exact_exe");
    } catch (e) {
      setFormError(String(e));
    }
  };

  const setup = async (input: RegisteredInput) => {
    setFormError(null);
    setBusy(true);
    try {
      await invoke("setup_registered_input", { inputName: input.input_name });
      setSetupName(input.input_name);
    } catch (e) {
      setFormError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const finishSetup = async () => {
    setBusy(true);
    try {
      await invoke("finish_setup");
      setSetupName(null);
      await refresh();
    } catch (e) {
      setFormError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const record = async (input: RegisteredInput) => {
    setFormError(null);
    try {
      if (setupName) await finishSetup();
      await invoke("start_registered_input", { inputName: input.input_name });
    } catch (e) {
      setFormError(String(e));
    }
  };

  const removeInput = async (input: RegisteredInput) => {
    setFormError(null);
    try {
      if (setupName === input.input_name) {
        await invoke("finish_setup");
        setSetupName(null);
      }
      await remove(input.id);
    } catch (e) {
      setFormError(String(e));
    }
  };

  const registered = new Set(inputs.map((a) => a.target_exe.toLowerCase()));

  const submit = async () => {
    setFormError(null);
    try {
      await add(name.trim(), exe.trim(), strategy);
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
      {setupName && (
        <div className="flex flex-wrap items-center gap-2 rounded-xl border border-cyan-500/30 bg-cyan-500/10 px-3 py-2.5 text-sm text-cyan-100">
          <Gamepad2 size={15} className="shrink-0" />
          <span className="min-w-0 flex-1">
            {t(
              inputs.find((i) => i.input_name === setupName)?.input_kind === "screen"
                ? "games.setup_hint_screen"
                : "games.setup_hint_window",
            )}
          </span>
          <button
            onClick={() => void finishSetup()}
            disabled={busy}
            className="inline-flex items-center gap-1.5 rounded-lg border border-cyan-500/40 bg-cyan-500/20 px-3 py-1 text-xs font-medium text-cyan-100 transition hover:bg-cyan-500/30 disabled:opacity-50"
          >
            <Check size={13} /> {t("games.finish_setup")}
          </button>
        </div>
      )}

      <div>
        <div className="mb-2 flex items-center gap-2">
          <h4 className="text-sm font-semibold text-slate-200">{t("games.running")}</h4>
          <button
            onClick={() => void refreshRunning()}
            className="ml-auto inline-flex items-center gap-1 text-xs text-slate-500 transition hover:text-slate-200"
          >
            <RefreshCw size={12} /> {t("games.refresh")}
          </button>
        </div>
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t("games.search_ph")}
          className="mb-2 w-full rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-sm text-slate-100 outline-none focus:border-cyan-500/50"
        />
        {running.length === 0 ? (
          <p className="text-xs text-slate-500">{t("games.empty_running")}</p>
        ) : (
          <ul className="max-h-56 space-y-1 overflow-y-auto pr-1">
            {running
              .filter(
                (r) =>
                  query.trim() === "" ||
                  r.name.toLowerCase().includes(query.toLowerCase()) ||
                  r.exe.toLowerCase().includes(query.toLowerCase()),
              )
              .map((r) => {
                const already = registered.has(r.exe.toLowerCase());
                return (
                  <li
                    key={`${r.exe}:${r.name}`}
                    className="flex items-center gap-3 rounded-lg border border-white/5 bg-black/20 px-3 py-1.5"
                  >
                    <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-white/10 text-[11px] font-semibold text-slate-300">
                      {r.name.trim().charAt(0).toUpperCase() || "?"}
                    </span>
                    <div className="min-w-0 flex-1">
                      <p className="truncate text-sm text-slate-200">{r.name}</p>
                      <p className="truncate font-mono text-[11px] text-slate-500">{r.exe}</p>
                    </div>
                    {already ? (
                      <span className="text-xs text-cyan-300">{t("games.registered")}</span>
                    ) : (
                      <button
                        onClick={() => void addRunning(r)}
                        className="inline-flex items-center gap-1 rounded-lg border border-cyan-500/30 bg-cyan-500/10 px-2.5 py-1 text-xs text-cyan-200 transition hover:bg-cyan-500/20"
                      >
                        <Plus size={13} /> {t("games.register_btn")}
                      </button>
                    )}
                  </li>
                );
              })}
          </ul>
        )}
      </div>

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
        <select
          className={`${input} w-full sm:w-auto`}
          value={strategy}
          onChange={(e) => setStrategy(e.target.value)}
        >
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

      <div>
        <h4 className="mb-2 text-sm font-semibold text-slate-200">{t("games.registered_title")}</h4>
        {inputs.length === 0 ? (
          <p className="text-sm text-slate-500">{t("games.empty")}</p>
        ) : (
          <ul className="space-y-2">
            {inputs.map((a) => (
              <li
                key={a.id}
                className="flex flex-col items-start gap-2 rounded-xl border border-white/5 bg-black/30 px-3 py-2.5 sm:flex-row sm:items-center sm:justify-between sm:gap-3 sm:px-4"
              >
                <div className="w-full min-w-0 text-sm sm:w-auto">
                  <p className="flex items-center gap-1.5 font-medium text-slate-200">
                    {a.input_kind === "screen" ? <Monitor size={14} /> : <Gamepad2 size={14} />}
                    {a.display_name}
                    <span className="rounded bg-white/10 px-1.5 py-0.5 text-[10px] font-normal uppercase tracking-wide text-slate-400">
                      {t(a.input_kind === "screen" ? "games.kind_screen" : "games.kind_window")}
                    </span>
                  </p>
                  {a.input_kind !== "screen" && (
                    <p className="break-all font-mono text-xs text-slate-500">
                      {a.target_exe} · {a.match_strategy}
                    </p>
                  )}
                </div>
                <div className="flex items-center gap-1.5">
                  <button
                    onClick={() => void setup(a)}
                    disabled={busy}
                    className="inline-flex items-center gap-1 rounded-lg border border-white/10 bg-white/5 px-2.5 py-1 text-xs text-slate-200 transition hover:bg-white/10 disabled:opacity-50"
                  >
                    {a.input_kind === "screen" ? <Monitor size={13} /> : <Gamepad2 size={13} />}
                    {t(a.input_kind === "screen" ? "games.pick_screen" : "games.pick_window")}
                  </button>
                  <button
                    onClick={() => void record(a)}
                    className="inline-flex items-center gap-1 rounded-lg border border-cyan-500/30 bg-cyan-500/10 px-2.5 py-1 text-xs text-cyan-200 transition hover:bg-cyan-500/20"
                  >
                    <Video size={13} /> {t("games.record")}
                  </button>
                  <button
                    onClick={() => void removeInput(a)}
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
      </div>
    </div>
  );
}
