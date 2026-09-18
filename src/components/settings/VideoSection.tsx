import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import type { AppSettings, HeightOpt, SystemMemory, VideoOptions } from "../../types";

/**
 * Medal-style recording settings on top of the embedded OBS engine.
 * Moon preset names stay in English in every language (SPEC §17.1); only
 * tags/descriptions are translated. All values go through the restart-once
 * IPC paths (`set_video_quality` / `set_settings`).
 */
const MOONS: Record<number, { name: string; tag?: string }> = {
  720: { name: "New Moon" },
  1080: { name: "First Quarter", tag: "video.tag_recommended" },
  1440: { name: "Waning Gibbous", tag: "video.tag_high" },
  2160: { name: "Full Moon", tag: "video.tag_ultra" },
};
const PLAIN: Record<number, string> = { 360: "Low", 480: "Medium" };
const CODEC_LABEL: Record<string, string> = {
  h264: "H.264",
  hevc: "H.265",
  av1: "AV1",
  x264: "x264",
};
const DURATION_PRESETS = [15, 30, 60, 120, 300, 600];

const mbFor = (kbps: number, seconds: number) => Math.round((kbps * seconds) / 8000);

export function VideoSection() {
  const { t } = useTranslation();
  const [opts, setOpts] = useState<VideoOptions | null>(null);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [freeMb, setFreeMb] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [riskAck, setRiskAck] = useState(false);
  const [customSeconds, setCustomSeconds] = useState("");
  const [customKbps, setCustomKbps] = useState(20000);
  const [customFps, setCustomFps] = useState(60);

  const load = async () => {
    try {
      const [o, s] = await Promise.all([
        invoke<VideoOptions>("video_options"),
        invoke<AppSettings>("get_settings"),
      ]);
      setOpts(o);
      setSettings(s);
      const kb = Number(s.custom_bitrate_kbps);
      if (Number.isFinite(kb) && kb > 0) setCustomKbps(Math.min(100000, Math.max(3000, kb)));
      const f = Number(s.custom_fps);
      if ([24, 30, 60, 120, 144].includes(f)) setCustomFps(f);
    } catch (e) {
      setError(String(e));
    }
  };

  useEffect(() => {
    void load();
    invoke<SystemMemory>("system_memory")
      .then((m) => setFreeMb(m.free_mb))
      .catch(() => setFreeMb(null));
  }, []);

  const applyVideo = async (codec: string, height: number, fps: number) => {
    setBusy(true);
    setError(null);
    try {
      await invoke("set_video_quality", { codec, height, fps });
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const applySettings = async (pairs: { key: string; value: string }[]) => {
    setBusy(true);
    setError(null);
    try {
      await invoke("set_settings", { values: pairs });
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const codecIdx = Math.max(
    0,
    opts?.codecs.findIndex((c) => c.id === opts.current_codec) ?? 0,
  );
  const customMode = settings?.video_mode === "custom";
  const fps = customMode ? customFps : (opts?.current_fps ?? 60);
  const bufferSeconds = Number(settings?.buffer_seconds ?? "30") || 30;

  const currentBitrate = useMemo(() => {
    if (!opts) return 20000;
    if (customMode) return customKbps;
    const row = opts.heights.find((h) => h.height === opts.current_height);
    if (!row) return 20000;
    return row.bitrates[Math.min(codecIdx, row.bitrates.length - 1)] ?? 20000;
  }, [opts, customMode, customKbps, codecIdx]);

  const ramMb = mbFor(currentBitrate, bufferSeconds);
  const overTwoMin = bufferSeconds > 120;
  const ramLow = freeMb != null && ramMb > Math.max(0, freeMb - 1024);

  const pickHeight = (h: number) => {
    if (!opts) return;
    if (customMode) {
      void applySettings([{ key: "out_height", value: String(h) }]);
    } else {
      void applyVideo(opts.current_codec, h, opts.current_fps);
    }
  };

  const badge = (row: HeightOpt, bitrate: number): string | null => {
    if (row.height === 1080 && opts?.current_codec === "h264" && fps === 60) {
      return t("video.badge_recommended");
    }
    if (bitrate >= 40000) return t("video.badge_ultra");
    if (opts && opts.vendor !== "nvidia") return t("video.badge_experimental");
    return null;
  };

  const pill = (active: boolean) =>
    `rounded-md px-2 py-0.5 font-mono text-[11px] transition ${
      active
        ? "bg-cyan-500/20 text-cyan-200"
        : "bg-white/5 text-slate-400 hover:bg-white/10 hover:text-slate-200"
    }`;

  if (error && !opts) return <p className="font-mono text-xs text-red-400">{error}</p>;
  if (!opts || !settings) return <p className="text-sm text-slate-400">{t("common.loading")}</p>;

  const selectedRow = opts.heights.find((h) => h.height === opts.current_height);
  const recommended = selectedRow?.recommended[Math.min(codecIdx, selectedRow.recommended.length - 1)];
  const overSource = opts.max_source_height > 0 && opts.current_height > opts.max_source_height;
  const select =
    "w-full rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-sm text-slate-100 outline-none focus:border-cyan-500/50";

  return (
    <div className="space-y-4">
      {/* Monitor */}
      <label className="block">
        <span className="mb-1 block text-sm text-slate-300">{t("video.monitor")}</span>
        <select
          className={select}
          value={opts.current_monitor}
          onChange={(e) =>
            void applySettings([{ key: "monitor", value: e.target.value }])
          }
        >
          <option value="">{t("video.monitor_auto")}</option>
          {opts.monitors.map((m) => (
            <option key={m.name} value={m.name}>
              {m.label}
            </option>
          ))}
        </select>
        {opts.monitors.length === 0 && (
          <span className="mt-1 block text-[11px] text-slate-500">{t("video.portal_note")}</span>
        )}
      </label>

      {/* Preset cards */}
      <div className="grid grid-cols-1 gap-1.5 sm:grid-cols-2">
        {opts.heights.map((row) => {
          const bitrate = row.bitrates[Math.min(codecIdx, row.bitrates.length - 1)] ?? 20000;
          const selected = row.height === opts.current_height;
          const moon = MOONS[row.height];
          const rowBadge = badge(row, bitrate);
          const name = PLAIN[row.height] ?? moon?.name;
          return (
            <button
              key={row.height}
              onClick={() => pickHeight(row.height)}
              disabled={busy}
              className={`rounded-xl border px-3 py-2 text-left transition disabled:opacity-60 ${
                selected
                  ? "border-cyan-500/40 bg-cyan-500/10"
                  : "border-white/5 bg-black/20 hover:border-white/15"
              }`}
            >
              <div className="flex flex-wrap items-center gap-x-2">
                <span className="text-sm font-medium text-slate-100">
                  {name} — {row.height}p
                </span>
                {moon?.tag && (
                  <span className="text-[10px] uppercase tracking-wide text-slate-500">
                    {t(moon.tag)}
                  </span>
                )}
                {rowBadge && (
                  <span
                    className={`ml-auto rounded-md px-1.5 py-0.5 text-[10px] font-semibold uppercase ${
                      rowBadge === t("video.badge_recommended")
                        ? "bg-cyan-500/15 text-cyan-300"
                        : rowBadge === t("video.badge_ultra")
                          ? "bg-fuchsia-500/15 text-fuchsia-300"
                          : "bg-amber-500/15 text-amber-300"
                    }`}
                  >
                    {rowBadge}
                  </span>
                )}
              </div>
              <div className="mt-0.5 flex items-center gap-2 font-mono text-[11px] text-slate-400">
                <span>{(bitrate / 1000).toFixed(bitrate % 1000 === 0 ? 0 : 1)} Mbps</span>
                <span className="text-slate-600">·</span>
                <span>
                  {t("video.ram_60")} ~{row.ring_mb_60s[Math.min(codecIdx, row.ring_mb_60s.length - 1)]} MB
                </span>
                {row.height > opts.max_source_height && opts.max_source_height > 0 && (
                  <span className="text-amber-400">↑</span>
                )}
              </div>
            </button>
          );
        })}
      </div>

      {/* FPS + codec + encoder */}
      <div className="space-y-2 rounded-xl border border-white/5 bg-black/20 px-3 py-3">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm text-slate-300">{t("video.fps")}</span>
          {opts.fps_options.map((f) => (
            <button
              key={f}
              onClick={() => void applyVideo(opts.current_codec, opts.current_height, f)}
              disabled={busy}
              className={pill(fps === f)}
            >
              {f}
            </button>
          ))}
          {fps > 60 && <span className="text-[11px] text-amber-400">{t("video.fps_warn")}</span>}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm text-slate-300">{t("video.codec")}</span>
          {opts.codecs
            .filter((c) => c.id !== "x264")
            .map((c) => (
              <button
                key={c.id}
                onClick={() => void applyVideo(c.id, opts.current_height, opts.current_fps)}
                disabled={busy}
                className={pill(opts.current_codec === c.id)}
                title={t(`video.codec_${c.id}_note`)}
              >
                {CODEC_LABEL[c.id] ?? c.id.toUpperCase()}
              </button>
            ))}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm text-slate-300">{t("video.encoder")}</span>
          {(["gpu", "cpu"] as const).map((e) => (
            <button
              key={e}
              onClick={() => void applySettings([{ key: "video_encoder", value: e }])}
              disabled={busy}
              className={pill(opts.encoder === e)}
            >
              {e === "gpu" ? t("video.encoder_gpu") : t("video.encoder_cpu")}
            </button>
          ))}
          {opts.encoder === "cpu" && (
            <span className="text-[11px] text-amber-400">{t("video.encoder_cpu_warn")}</span>
          )}
        </div>
        {recommended && (
          <p className="font-mono text-[11px] text-slate-500">
            {t("video.medal_range", {
              min: (recommended[0] / 1000).toFixed(0),
              max: (recommended[1] / 1000).toFixed(0),
              codec: CODEC_LABEL[opts.current_codec] ?? opts.current_codec,
            })}
          </p>
        )}
      </div>

      {/* Custom mode */}
      <div className="space-y-2 rounded-xl border border-white/5 bg-black/20 px-3 py-3">
        <label className="flex items-center gap-2 text-sm text-slate-300">
          <input
            type="checkbox"
            checked={customMode}
            onChange={(e) =>
              void applySettings([
                { key: "video_mode", value: e.target.checked ? "custom" : "ladder" },
              ])
            }
            className="h-3.5 w-3.5 accent-cyan-400"
          />
          {t("video.custom_mode")}
        </label>
        {customMode && (
          <div className="space-y-2">
            <label className="block text-xs text-slate-400">
              {t("video.custom_bitrate")}: <span className="font-mono">{(customKbps / 1000).toFixed(0)} Mbps</span>
              <input
                type="range"
                min={3000}
                max={100000}
                step={1000}
                value={customKbps}
                onChange={(e) => setCustomKbps(Number(e.target.value))}
                className="mt-1 w-full accent-cyan-400"
              />
              <span className="flex justify-between font-mono text-[10px] text-slate-600">
                <span>3 Mbps</span>
                <span>100 Mbps</span>
              </span>
            </label>
            <label className="block text-xs text-slate-400">
              {t("video.custom_fps")}
              <select
                value={String(customFps)}
                onChange={(e) => setCustomFps(Number(e.target.value))}
                className="ml-2 rounded-lg border border-white/10 bg-white/5 px-2 py-1 text-xs text-slate-100 outline-none"
              >
                {opts.fps_options.map((f) => (
                  <option key={f} value={String(f)}>
                    {f}
                  </option>
                ))}
              </select>
            </label>
            <button
              onClick={() =>
                void applySettings([
                  { key: "custom_bitrate_kbps", value: String(customKbps) },
                  { key: "custom_fps", value: String(customFps) },
                ])
              }
              disabled={busy}
              className="rounded-lg border border-cyan-500/30 bg-cyan-500/10 px-2.5 py-1 text-xs font-semibold text-cyan-200 transition hover:bg-cyan-500/20 disabled:opacity-50"
            >
              {t("video.apply")}
            </button>
          </div>
        )}
      </div>

      {/* Duration + container */}
      <div className="space-y-2 rounded-xl border border-white/5 bg-black/20 px-3 py-3">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm text-slate-300">{t("video.duration")}</span>
          {DURATION_PRESETS.map((s) => (
            <button
              key={s}
              onClick={() => void applySettings([{ key: "buffer_seconds", value: String(s) }])}
              disabled={busy}
              className={pill(bufferSeconds === s)}
            >
              {s >= 60 && s % 60 === 0 ? t("video.minutes", { n: s / 60 }) : t("video.seconds", { n: s })}
            </button>
          ))}
          <input
            type="number"
            min={5}
            step={5}
            placeholder={t("video.custom")}
            value={customSeconds}
            onChange={(e) => setCustomSeconds(e.target.value)}
            className="w-20 rounded-lg border border-white/10 bg-white/5 px-2 py-1 text-xs text-slate-100 outline-none focus:border-cyan-500/50"
          />
          <button
            onClick={() =>
              customSeconds.trim() !== "" &&
              void applySettings([{ key: "buffer_seconds", value: customSeconds }])
            }
            disabled={busy || customSeconds.trim() === ""}
            className="rounded-lg border border-cyan-500/30 bg-cyan-500/10 px-2.5 py-1 text-xs font-semibold text-cyan-200 transition hover:bg-cyan-500/20 disabled:opacity-50"
          >
            {t("video.apply")}
          </button>
          {overTwoMin && (
            <span className="rounded-md bg-amber-500/15 px-2 py-0.5 text-[10px] font-semibold uppercase text-amber-300">
              {t("video.badge_experimental")}
            </span>
          )}
        </div>
        <p className={`font-mono text-[11px] ${ramLow ? "text-red-400" : "text-slate-500"}`}>
          {t("video.ram_estimate", { ram: ramMb })}
          {freeMb != null ? ` · ${t("video.ram_free", { free: freeMb })}` : ""}
        </p>
        {overTwoMin && (
          <label className="flex items-center gap-2 text-xs text-amber-300">
            <input
              type="checkbox"
              checked={riskAck}
              onChange={(e) => setRiskAck(e.target.checked)}
              className="h-3.5 w-3.5 accent-amber-400"
            />
            {t("video.risk_ack")}
          </label>
        )}
        {ramLow && <p className="text-xs text-red-400">{t("video.ram_low")}</p>}
        <div className="flex flex-wrap items-center gap-2 pt-1">
          <span className="text-sm text-slate-300">{t("video.container")}</span>
          {["mp4", "mkv"].map((c) => (
            <button
              key={c}
              onClick={() => void applySettings([{ key: "container", value: c }])}
              disabled={busy}
              className={pill((settings.container || "mp4") === c)}
            >
              {c.toUpperCase()}
            </button>
          ))}
        </div>
      </div>

      {/* Perf hints */}
      {overSource && <p className="text-xs text-amber-400">{t("video.upscale_warn")}</p>}
      <p className="text-[11px] text-slate-600">{t("video.perf_hint")}</p>
      <p className="text-[11px] text-slate-600">{t("video.gpu_scale_note")}</p>
      <p className="text-[11px] text-slate-600">{t("video.restart_notice")}</p>
      <p className="text-[11px] text-slate-600">{t("video.codec_savings")}</p>

      {error && <p className="break-all font-mono text-xs text-red-400">{error}</p>}
      {busy && <p className="font-mono text-[11px] text-cyan-300">…</p>}
    </div>
  );
}
