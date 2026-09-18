import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { QualityTable } from "./QualityTable";
import type { GsrInfo, VideoOptions } from "../../types";

/** Recording video: monitor + the Medal-style quality table. */
export function VideoSection() {
  const { t } = useTranslation();
  const [opts, setOpts] = useState<VideoOptions | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [gsr, setGsr] = useState<GsrInfo | null>(null);
  const [capsBusy, setCapsBusy] = useState(false);
  const [capsMsg, setCapsMsg] = useState<string | null>(null);

  const load = () => {
    invoke<VideoOptions>("video_options")
      .then(setOpts)
      .catch((e) => setError(String(e)));
  };
  useEffect(load, []);

  const loadGsr = () => {
    invoke<GsrInfo>("gsr_info").then(setGsr).catch((e) => setCapsMsg(String(e)));
  };
  useEffect(loadGsr, []);

  const fixCaps = async () => {
    setCapsBusy(true);
    setCapsMsg(null);
    try {
      await invoke("fix_gsr_caps");
      loadGsr();
    } catch (e) {
      setCapsMsg(String(e));
    } finally {
      setCapsBusy(false);
    }
  };

  const changeMonitor = async (value: string) => {
    setError(null);
    try {
      await invoke("set_setting", { key: "monitor", value });
      load();
    } catch (e) {
      setError(String(e));
    }
  };

  if (error) return <p className="font-mono text-xs text-red-400">{error}</p>;
  if (!opts) return <p className="text-sm text-slate-400">{t("common.loading")}</p>;

  const upscale = opts.max_source_height > 0 && opts.current_height > opts.max_source_height;
  const select =
    "w-full rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-sm text-slate-100 outline-none focus:border-cyan-500/50";

  return (
    <div className="space-y-3">
      <label className="block">
        <span className="mb-1 block text-sm text-slate-300">{t("video.monitor")}</span>
        <select
          className={select}
          value={opts.current_monitor}
          onChange={(e) => void changeMonitor(e.target.value)}
        >
          <option value="">{t("video.monitor_auto")}</option>
          {opts.monitors.map((m) => (
            <option key={m.name} value={m.name}>
              {m.label}
            </option>
          ))}
        </select>
      </label>

      {gsr?.present && !gsr.caps_ok && (
        <div className="flex flex-wrap items-center gap-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2">
          <p className="flex-1 text-xs text-amber-300">{t("video.caps_missing")}</p>
          <button
            onClick={() => void fixCaps()}
            disabled={capsBusy}
            className="rounded-lg border border-amber-500/40 bg-amber-500/10 px-2.5 py-1.5 text-xs font-semibold text-amber-200 transition hover:bg-amber-500/20 disabled:opacity-50"
          >
            {capsBusy ? t("video.caps_fixing") : t("video.caps_fix")}
          </button>
        </div>
      )}
      {capsMsg && <p className="break-all font-mono text-xs text-red-400">{capsMsg}</p>}
      {upscale && <p className="text-xs text-amber-400">{t("video.upscale_warn")}</p>}

      <QualityTable opts={opts} onReload={load} />
    </div>
  );
}
