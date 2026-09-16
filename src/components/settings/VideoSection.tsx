import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";

interface CodecOpt {
  id: string;
}

interface HeightOpt {
  height: number;
  label: string;
  bitrates: number[];
  ring_mb_60s: number[];
}

interface MonitorOpt {
  name: string;
  label: string;
}

interface VideoOptions {
  codecs: CodecOpt[];
  heights: HeightOpt[];
  monitors: MonitorOpt[];
  current_codec: string;
  current_height: number;
  current_fps: number;
  current_monitor: string;
  buffer_height: number;
  transcoding: boolean;
  max_source_height: number;
  vendor: string;
}

interface GsrInfo {
  path: string;
  source: string;
  caps_ok: boolean;
  present: boolean;
}

/** Recording video: codec + output resolution (Medal ladder, GSR-backed). */
export function VideoSection() {
  const { t } = useTranslation();
  const [opts, setOpts] = useState<VideoOptions | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [gsr, setGsr] = useState<GsrInfo | null>(null);
  const [capsBusy, setCapsBusy] = useState(false);
  const [capsMsg, setCapsMsg] = useState<string | null>(null);

  const load = () => {
    invoke<VideoOptions>("video_options").then(setOpts).catch((e) => setError(String(e)));
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

  const change = async (key: "video_codec" | "out_height" | "fps" | "monitor", value: string) => {
    setError(null);
    try {
      await invoke("set_setting", { key, value });
      load();
    } catch (e) {
      setError(String(e));
    }
  };

  if (error) return <p className="font-mono text-xs text-red-400">{error}</p>;
  if (!opts) return <p className="text-sm text-slate-400">{t("common.loading")}</p>;

  const codecIdx = Math.max(
    0,
    opts.codecs.findIndex((c) => c.id === opts.current_codec),
  );
  const heightRow =
    opts.heights.find((h) => h.height === opts.current_height) ?? opts.heights[2];
  const codecLabel = (id: string): string => {
    const key = `video.codec_${id}_label`;
    const s = t(key);
    return s === key ? id : s;
  };
  const codecNote = (id: string): string => {
    const key = `video.codec_${id}_note`;
    const s = t(key);
    return s === key ? "" : s;
  };
  const currentNote = codecNote(opts.current_codec);
  // RAM hint follows the BUFFER resolution (source when transcoding).
  const bufRow =
    opts.heights.find((h) => h.height === opts.buffer_height) ?? heightRow;
  const bufBitrate = bufRow.bitrates[Math.min(codecIdx, bufRow.bitrates.length - 1)];
  const bufRam = bufRow.ring_mb_60s[Math.min(codecIdx, bufRow.ring_mb_60s.length - 1)];
  const upscale =
    opts.max_source_height > 0 &&
    opts.current_height > opts.max_source_height;

  const select =
    "w-full rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-sm text-slate-100 outline-none focus:border-cyan-500/50";

  return (
    <div className="space-y-3">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <label className="block">
          <span className="mb-1 block text-sm text-slate-300">{t("video.codec")}</span>
          <select
            className={select}
            value={opts.current_codec}
            onChange={(e) => void change("video_codec", e.target.value)}
          >
            {opts.codecs.map((c) => (
              <option key={c.id} value={c.id}>
                {codecLabel(c.id)}
              </option>
            ))}
          </select>
        </label>
        <label className="block">
          <span className="mb-1 block text-sm text-slate-300">{t("video.resolution")}</span>
          <select
            className={select}
            value={String(opts.current_height)}
            onChange={(e) => void change("out_height", e.target.value)}
          >
            <option value="0">{t("video.source")}</option>
            {opts.heights.map((h) => (
              <option key={h.height} value={String(h.height)}>
                {h.label}
              </option>
            ))}
          </select>
        </label>
        <label className="block">
          <span className="mb-1 block text-sm text-slate-300">{t("video.monitor")}</span>
          <select
            className={select}
            value={opts.current_monitor}
            onChange={(e) => void change("monitor", e.target.value)}
          >
            <option value="">{t("video.monitor_auto")}</option>
            {opts.monitors.map((m) => (
              <option key={m.name} value={m.name}>
                {m.label}
              </option>
            ))}
          </select>
        </label>
        <label className="block">
          <span className="mb-1 block text-sm text-slate-300">{t("video.fps")}</span>
          <select
            className={select}
            value={String(opts.current_fps)}
            onChange={(e) => void change("fps", e.target.value)}
          >
            <option value="60">60 fps</option>
            <option value="30">30 fps</option>
          </select>
        </label>
      </div>
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
      <p className="text-xs text-slate-400">{currentNote}</p>
      {upscale && (
        <p className="text-xs text-amber-400">{t("video.upscale_warn")}</p>
      )}
      {opts.transcoding && (
        <p className="text-xs text-cyan-300/80">{t("video.transcoding_note")}</p>
      )}
      <p className="text-xs text-slate-600">{t("video.disk_note")}</p>
      <p className="font-mono text-xs text-slate-400">
        {t("video.estimate", { bitrate: bufBitrate, ram: bufRam })}
      </p>
      <p className="text-xs text-slate-600">{t("video.hq_note")}</p>
      <p className="text-xs text-slate-600">{t("video.judder_note")}</p>
    </div>
  );
}
