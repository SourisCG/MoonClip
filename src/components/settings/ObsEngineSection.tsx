import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { ChevronDown, ChevronRight, ShieldCheck, Wrench } from "lucide-react";
import type { ObsInfo } from "../../types";

/**
 * "Motor OBS (aislado)": status of the embedded OBS + obs-cmd, the private
 * config location and a one-click repair. This section exists to make the
 * isolation visible: the user's own OBS is never read or modified.
 */
export function ObsEngineSection() {
  const { t } = useTranslation();
  const [info, setInfo] = useState<ObsInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [repaired, setRepaired] = useState(false);
  const [showLogs, setShowLogs] = useState(false);

  const load = () => {
    invoke<ObsInfo>("obs_info")
      .then((i) => {
        setInfo(i);
        setError(null);
      })
      .catch((e) => setError(String(e)));
  };
  useEffect(load, []);

  const repair = async () => {
    setBusy(true);
    setError(null);
    setRepaired(false);
    try {
      await invoke("repair_obs_config");
      setRepaired(true);
      // The config is regenerated on the next buffer start.
      load();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const row = "flex flex-wrap items-center justify-between gap-2";
  const label = "text-sm text-slate-300";
  const value = "min-w-0 break-all font-mono text-xs text-slate-400";

  if (error && !info) return <p className="font-mono text-xs text-red-400">{error}</p>;
  if (!info) return <p className="text-sm text-slate-400">{t("common.loading")}</p>;

  const missing = !info.present;

  return (
    <div className="space-y-3 rounded-xl border border-white/5 bg-black/30 px-3 py-3">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h4 className="text-sm font-semibold text-slate-200">{t("obs.title")}</h4>
          <p className="text-[11px] text-slate-500">{t("obs.subtitle")}</p>
        </div>
        <button
          onClick={() => void repair()}
          disabled={busy}
          className="inline-flex shrink-0 items-center gap-1.5 rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-xs text-slate-200 transition hover:border-cyan-500/40 hover:text-cyan-200 disabled:opacity-50"
        >
          <Wrench size={13} />
          {busy ? t("obs.repairing") : t("obs.repair")}
        </button>
      </div>

      {missing ? (
        <div className="space-y-1">
          <p className="text-xs text-amber-400">{t("obs.missing")}</p>
          <p className="text-[11px] text-slate-500">{t("obs.missing_hint", { source: info.source })}</p>
        </div>
      ) : (
        <div className="space-y-1.5">
          <div className={row}>
            <span className={label}>{t("obs.version")}</span>
            <span className={value}>{info.version || "?"}</span>
          </div>
          <div className={row}>
            <span className={label}>{t("obs.profile")}</span>
            <span className={value}>
              {info.profile} · {info.collection}
            </span>
          </div>
          <div className={row}>
            <span className={label}>{t("obs.port")}</span>
            <span className={value}>{info.websocket_port}</span>
          </div>
          <div className={row}>
            <span className={label}>{t("obs.config_dir")}</span>
            <span className={value}>{info.config_dir}</span>
          </div>
        </div>
      )}

      {repaired && <p className="text-xs text-cyan-300">{t("obs.repaired")}</p>}

      <p className="flex items-center gap-1.5 text-[11px] text-emerald-300/90">
        <ShieldCheck size={13} /> {t("obs.anti_cheat")}
      </p>

      <button
        onClick={() => setShowLogs((v) => !v)}
        className="inline-flex items-center gap-1 text-[11px] text-slate-500 transition hover:text-slate-300"
      >
        {showLogs ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        {t("obs.logs")}
      </button>
      {showLogs && (
        <pre className="max-h-40 overflow-auto rounded-lg bg-black/50 p-2 font-mono text-[10px] leading-relaxed text-slate-500">
          {info.log_tail.length > 0 ? info.log_tail.slice(-12).join("\n") : t("obs.no_logs")}
        </pre>
      )}

      {error && <p className="break-all font-mono text-xs text-red-400">{error}</p>}
    </div>
  );
}
