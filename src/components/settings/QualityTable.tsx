import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import type { AppSettings, HeightOpt, SystemMemory, VideoOptions } from "../../types";

/** Moon preset names stay in English in every language (SPEC §17.1). */
const MOONS: Record<number, { name: string; tag?: string }> = {
  720: { name: "New Moon" },
  1080: { name: "First Quarter", tag: "quality.tag_recommended" },
  1440: { name: "Waning Gibbous", tag: "quality.tag_high" },
  2160: { name: "Full Moon", tag: "quality.tag_ultra" },
};
const PLAIN: Record<number, string> = { 360: "Low", 480: "Medium" };

const DURATION_PRESETS = [15, 30, 60, 120, 300, 600];
const CUSTOM_FPS = [24, 30, 60, 120, 144];

const mbFor = (kbps: number, seconds: number) => Math.round((kbps * seconds) / 8000);

export function QualityTable({
  opts,
  onReload,
}: {
  opts: VideoOptions;
  onReload: () => void;
}) {
  const { t } = useTranslation();
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [freeMb, setFreeMb] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [customSeconds, setCustomSeconds] = useState("");
  const [riskAck, setRiskAck] = useState(false);
  const [customKbps, setCustomKbps] = useState(20000);
  const [customFps, setCustomFps] = useState(60);
  const [hovered, setHovered] = useState<number | null>(null);

  const loadSettings = () =>
    invoke<AppSettings>("get_settings")
      .then((s) => {
        setSettings(s);
        const kb = Number(s.custom_bitrate_kbps);
        if (Number.isFinite(kb) && kb > 0) setCustomKbps(Math.min(100000, Math.max(3000, kb)));
        const f = Number(s.custom_fps);
        if (CUSTOM_FPS.includes(f)) setCustomFps(f);
      })
      .catch((e) => setError(String(e)));

  useEffect(() => {
    void loadSettings();
    invoke<SystemMemory>("system_memory")
      .then((m) => setFreeMb(m.free_mb))
      .catch(() => setFreeMb(null));
  }, []);

  const codecIdx = Math.max(
    0,
    opts.codecs.findIndex((c) => c.id === opts.current_codec),
  );
  const av1Offered = opts.codecs.some((c) => c.id === "av1");
  const experimentalGpu = opts.vendor !== "nvidia";

  const bufferRow: HeightOpt = useMemo(() => {
    const byBuffer = opts.heights.find((h) => h.height === opts.buffer_height);
    if (byBuffer) return byBuffer;
    return opts.heights.find((h) => h.height === opts.current_height) ?? opts.heights[0];
  }, [opts]);

  const bitrateOf = (row: HeightOpt) =>
    row.bitrates[Math.min(codecIdx, row.bitrates.length - 1)] ?? 20000;

  const durationSeconds = Number(settings?.buffer_seconds ?? "30") || 30;
  const bufferBitrate = bitrateOf(bufferRow);
  const ramMb = mbFor(bufferBitrate, durationSeconds);
  const overTwoMin = durationSeconds > 120;
  const ramLow = freeMb != null && ramMb > Math.max(0, freeMb - 1024);

  const applyVideo = async (codec: string, height: number, fps: number) => {
    setBusy(true);
    setError(null);
    try {
      await invoke("set_video_quality", { codec, height, fps });
      onReload();
      await loadSettings();
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
      onReload();
      await loadSettings();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const applyDuration = async (seconds: number) => {
    if (!Number.isFinite(seconds) || seconds < 5) {
      setError(t("quality.custom_seconds"));
      return;
    }
    if (seconds > 120 && !riskAck) {
      setError(t("quality.over_2min", { ram: mbFor(bufferBitrate, seconds), save: mbFor(bufferBitrate, seconds) }));
      return;
    }
    await applySettings([{ key: "buffer_seconds", value: String(Math.round(seconds)) }]);
  };

  const badge = (row: HeightOpt, bitrate: number): string | null => {
    if (row.height === 1080 && opts.current_codec === "h264" && opts.current_fps === 60) {
      return t("quality.badge_recommended");
    }
    if (bitrate >= 40000) return t("quality.badge_ultra");
    if (experimentalGpu) return t("quality.badge_experimental");
    return null;
  };

  const pill = (active: boolean) =>
    `rounded-md px-2 py-0.5 font-mono text-[11px] transition ${
      active
        ? "bg-cyan-500/20 text-cyan-200"
        : "bg-white/5 text-slate-400 hover:bg-white/10 hover:text-slate-200"
    }`;

  if (!settings) return <p className="text-sm text-slate-400">{t("common.loading")}</p>;

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        <h4 className="text-sm font-semibold text-slate-200">{t("quality.title")}</h4>
        <span className="rounded-md bg-white/5 px-2 py-0.5 font-mono text-[11px] text-slate-400">
          {opts.vendor}
        </span>
        {freeMb != null && (
          <span className="rounded-md bg-white/5 px-2 py-0.5 font-mono text-[11px] text-slate-400">
            {t("quality.ram_free", { free: freeMb })}
          </span>
        )}
        {busy && <span className="font-mono text-[11px] text-cyan-300">…</span>}
      </div>

      <div className="space-y-1.5">
        {opts.heights.map((row) => {
          const bitrate = bitrateOf(row);
          const selected = row.height === opts.current_height;
          const moon = MOONS[row.height];
          const rowBadge = badge(row, bitrate);
          return (
            <div
              key={row.height}
              onMouseEnter={() => setHovered(row.height)}
              onMouseLeave={() => setHovered(null)}
              className={`rounded-xl border px-3 py-2 transition ${
                selected
                  ? "border-cyan-500/30 bg-cyan-500/5"
                  : "border-white/5 bg-black/20 hover:border-white/10"
              }`}
            >
              <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
                <button
                  onClick={() => void applyVideo(opts.current_codec, row.height, opts.current_fps)}
                  disabled={busy}
                  className="min-w-40 flex-1 text-left disabled:opacity-60"
                >
                  <span className="text-sm font-medium text-slate-100">
                    {PLAIN[row.height] ?? moon?.name} — {row.height}p
                  </span>
                  {moon?.tag && (
                    <span className="ml-2 text-[11px] text-slate-500">{t(moon.tag)}</span>
                  )}
                </button>
                <div className="flex items-center gap-1">
                  {[30, 60].map((f) => (
                    <button
                      key={f}
                      onClick={() => void applyVideo(opts.current_codec, row.height, f)}
                      disabled={busy}
                      className={pill(opts.current_fps === f && selected)}
                    >
                      {f}
                    </button>
                  ))}
                </div>
                <div className="flex items-center gap-1">
                  {opts.codecs
                    .filter((c) => c.id !== "x264")
                    .map((c) => (
                      <button
                        key={c.id}
                        onClick={() => void applyVideo(c.id, row.height, opts.current_fps)}
                        disabled={busy || (c.id === "av1" && !av1Offered)}
                        title={c.id === "av1" ? t("quality.badge_experimental") : undefined}
                        className={`${pill(opts.current_codec === c.id && selected)} disabled:opacity-40`}
                      >
                        {c.id.toUpperCase()}
                      </button>
                    ))}
                  {opts.codecs.some((c) => c.id === "x264") && (
                    <button
                      onClick={() => void applyVideo("x264", row.height, opts.current_fps)}
                      disabled={busy}
                      className={pill(opts.current_codec === "x264" && selected)}
                    >
                      x264
                    </button>
                  )}
                </div>
                <span className="rounded-md bg-white/5 px-2 py-0.5 font-mono text-[11px] text-slate-300">
                  {(bitrate / 1000).toFixed(bitrate % 1000 === 0 ? 0 : 1)}M
                </span>
                <span className="font-mono text-[11px] text-slate-500">
                  {t("quality.ram60")} ~{row.ring_mb_60s[Math.min(codecIdx, row.ring_mb_60s.length - 1)]} MB
                </span>
                {rowBadge && (
                  <span
                    className={`rounded-md px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide ${
                      rowBadge === t("quality.badge_recommended")
                        ? "bg-cyan-500/15 text-cyan-300"
                        : rowBadge === t("quality.badge_ultra")
                          ? "bg-fuchsia-500/15 text-fuchsia-300"
                          : "bg-amber-500/15 text-amber-300"
                    }`}
                  >
                    {rowBadge}
                  </span>
                )}
              </div>
              {hovered === row.height && (
                <p className="mt-1 font-mono text-[11px] text-slate-500">
                  {t("quality.preview", {
                    mb30: mbFor(bitrate, 30),
                    mb60: mbFor(bitrate, 60),
                    mb120: mbFor(bitrate, 120),
                  })}
                </p>
              )}
            </div>
          );
        })}
      </div>

      <div className="space-y-2 rounded-xl border border-white/5 bg-black/20 px-3 py-3">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm text-slate-300">{t("quality.duration")}</span>
          {DURATION_PRESETS.map((s) => (
            <button
              key={s}
              onClick={() => void applyDuration(s)}
              disabled={busy}
              className={pill(durationSeconds === s)}
            >
              {s >= 60 && s % 60 === 0 ? t("quality.minutes", { n: s / 60 }) : t("quality.seconds", { n: s })}
            </button>
          ))}
          <input
            type="number"
            min={5}
            step={5}
            placeholder={t("quality.custom")}
            value={customSeconds}
            onChange={(e) => setCustomSeconds(e.target.value)}
            className="w-24 rounded-lg border border-white/10 bg-white/5 px-2 py-1 text-xs text-slate-100 outline-none focus:border-cyan-500/50"
          />
          <button
            onClick={() => void applyDuration(Number(customSeconds))}
            disabled={busy || customSeconds.trim() === ""}
            className="rounded-lg border border-cyan-500/30 bg-cyan-500/10 px-2.5 py-1 text-xs font-semibold text-cyan-200 transition hover:bg-cyan-500/20 disabled:opacity-50"
          >
            {t("quality.apply")}
          </button>
          {overTwoMin && (
            <span className="rounded-md bg-amber-500/15 px-2 py-0.5 text-[10px] font-semibold uppercase text-amber-300">
              {t("quality.experimental_badge")}
            </span>
          )}
        </div>
        <p className={`font-mono text-[11px] ${ramLow ? "text-red-400" : "text-slate-500"}`}>
          {t("quality.ram_estimate", { ram: ramMb, save: ramMb })}
        </p>
        {overTwoMin && (
          <label className="flex items-center gap-2 text-xs text-amber-300">
            <input
              type="checkbox"
              checked={riskAck}
              onChange={(e) => setRiskAck(e.target.checked)}
              className="h-3.5 w-3.5 accent-amber-400"
            />
            {t("quality.risk_ack")}
          </label>
        )}
        {ramLow && <p className="text-xs text-red-400">{t("quality.ram_low")}</p>}
      </div>

      <div className="space-y-2 rounded-xl border border-white/5 bg-black/20 px-3 py-3">
        <label className="flex items-center gap-2 text-sm text-slate-300">
          <input
            type="checkbox"
            checked={settings.video_mode === "custom"}
            onChange={(e) =>
              void applySettings([
                { key: "video_mode", value: e.target.checked ? "custom" : "ladder" },
              ])
            }
            className="h-3.5 w-3.5 accent-cyan-400"
          />
          {t("quality.custom_mode")}
        </label>
        {settings.video_mode === "custom" && (
          <div className="space-y-2">
            <label className="block text-xs text-slate-400">
              {t("quality.custom_bitrate")}: {(customKbps / 1000).toFixed(0)}M
              <input
                type="range"
                min={3000}
                max={100000}
                step={1000}
                value={customKbps}
                onChange={(e) => setCustomKbps(Number(e.target.value))}
                className="mt-1 w-full accent-cyan-400"
              />
            </label>
            <label className="block text-xs text-slate-400">
              {t("quality.custom_fps")}
              <select
                value={String(customFps)}
                onChange={(e) => setCustomFps(Number(e.target.value))}
                className="ml-2 rounded-lg border border-white/10 bg-white/5 px-2 py-1 text-xs text-slate-100 outline-none"
              >
                {CUSTOM_FPS.map((f) => (
                  <option key={f} value={String(f)}>
                    {f}
                  </option>
                ))}
              </select>
            </label>
            {![30, 60].includes(customFps) && (
              <p className="text-[11px] text-amber-400">
                {t("quality.fps_clamped", { fps: customFps <= 45 ? 30 : 60 })}
              </p>
            )}
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
              {t("quality.apply")}
            </button>
          </div>
        )}
      </div>

      <div className="flex flex-wrap items-center gap-2 rounded-xl border border-white/5 bg-black/20 px-3 py-3">
        <span className="text-sm text-slate-300">{t("quality.container")}</span>
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
        {opts.transcoding && (
          <span className="text-[11px] text-cyan-300/80">{t("video.transcoding_note")}</span>
        )}
      </div>

      <div className="space-y-2 rounded-xl border border-white/5 bg-black/20 px-3 py-3">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm text-slate-300">{t("quality.capture_cap")}</span>
          {[0, 60, 90, 120].map((v) => (
            <button
              key={v}
              onClick={() =>
                void applySettings([{ key: "capture_max_fps", value: String(v) }])
              }
              disabled={busy}
              className={pill(Number(settings.capture_max_fps || "0") === v)}
            >
              {v === 0 ? t("quality.capture_auto") : v}
            </button>
          ))}
        </div>
        <p className="text-[11px] text-slate-600">{t("quality.capture_cap_hint")}</p>
        <label className="flex items-center gap-2 text-sm text-slate-300">
          <input
            type="checkbox"
            checked={settings.faststart === "1"}
            onChange={(e) =>
              void applySettings([
                { key: "faststart", value: e.target.checked ? "1" : "0" },
              ])
            }
            className="h-3.5 w-3.5 accent-cyan-400"
          />
          {t("quality.faststart")}
        </label>
        <p className="text-[11px] text-slate-600">{t("quality.faststart_hint")}</p>
      </div>

      {error && <p className="break-all font-mono text-xs text-red-400">{error}</p>}
      <p className="text-[11px] text-slate-600">{t("quality.gpu_scale_note")}</p>
      <p className="text-[11px] text-slate-600">{t("quality.codec_savings")}</p>
      <p className="text-[11px] text-slate-600">{t("quality.restart_notice")}</p>
    </div>
  );
}
