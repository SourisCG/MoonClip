import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import type {
  AppSettings,
  CustomVideo,
  EncoderSchema,
  HardwareTestResult,
  OptionSpecJson,
  SystemMemory,
  VideoOptions,
} from "../../types";
import { NumberField } from "./NumberField";

/**
 * Medal-style recording settings on top of the embedded OBS engine.
 * Moon preset names stay in English in every language (SPEC §17.1); only
 * tags/descriptions are translated.
 *
 * Simple view (video_mode=ladder): monitor + Moon preset cards + duration +
 * container. Custom view (video_mode=custom): every OBS video-output option
 * (encoder + full per-family schema + Video tab), rendered dynamically from
 * the backend registry. All values go through the restart-once IPC paths.
 */
const MOONS: Record<number, { name: string; tag?: string }> = {
  720: { name: "New Moon" },
  1080: { name: "First Quarter", tag: "video.tag_recommended" },
  1440: { name: "Waning Gibbous", tag: "video.tag_high" },
  2160: { name: "Full Moon", tag: "video.tag_ultra" },
};
const PLAIN: Record<number, string> = { 360: "Low", 480: "Medium" };
const DURATION_PRESETS = [15, 30, 60, 120, 300, 600];
const TEST_SECONDS = 10;

const mbFor = (kbps: number, seconds: number) => Math.round((kbps * seconds) / 8000);
const even = (v: number) => Math.max(2, Math.min(8192, Math.round(v))) & ~1;

type DraftVals = Record<string, string | number | boolean>;

const specDefault = (s: OptionSpecJson): string | number | boolean | undefined => {
  if (s.default === null || s.default === undefined) return undefined;
  return s.default;
};

const visible = (s: OptionSpecJson, vals: DraftVals, schema: OptionSpecJson[]): boolean => {
  const rule = s.visible_when;
  if (!rule) return true;
  const ctrl = schema.find((o) => o.key === rule.key);
  const cur = vals[rule.key] ?? (ctrl ? specDefault(ctrl) : undefined);
  if (cur === undefined || !rule.values.includes(String(cur))) return false;
  if (rule.and_key) {
    const actrl = schema.find((o) => o.key === rule.and_key);
    const acur = vals[rule.and_key] ?? (actrl ? specDefault(actrl) : undefined);
    if (acur === undefined || !rule.and_values.includes(String(acur))) return false;
  }
  return true;
};

const defaultVideo = (height: number): CustomVideo => ({
  out_width: even((height * 16) / 9),
  out_height: height,
  scale_type: "bicubic",
  fps_type: "common",
  fps_common: 60,
  fps_int: 60,
  fps_num: 60000,
  fps_den: 1001,
  color_format: "NV12",
  color_space: "709",
  color_range: "Partial",
});

/** Ladder CBR for a codec+height (Medal table): the bitrate Custom starts from. */
const ladderBitrateFor = (o: VideoOptions | null, codecId: string, height: number): number => {
  if (!o) return 20000;
  const row = o.heights.find((h) => h.height === height);
  const idx = Math.max(0, o.codecs.findIndex((c) => c.id === codecId));
  if (!row) return 20000;
  return row.bitrates[Math.min(idx, row.bitrates.length - 1)] ?? 20000;
};

/** Schema resolved for one encoder id (codec already fixed). */
const schemaFor = (o: VideoOptions | null, encId: string): EncoderSchema | undefined =>
  o?.encoder_schema.find((s) => s.encoder === encId);

export function VideoSection() {
  const { t } = useTranslation();
  const [opts, setOpts] = useState<VideoOptions | null>(null);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [freeMb, setFreeMb] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [riskAck, setRiskAck] = useState(false);
  const [customSeconds, setCustomSeconds] = useState("");

  // Custom drafts (only used when video_mode=custom).
  const [encId, setEncId] = useState("");
  const [vals, setVals] = useState<DraftVals>({});
  const [autoKeys, setAutoKeys] = useState<Record<string, boolean>>({});
  const [video, setVideo] = useState<CustomVideo>(() => defaultVideo(1080));
  const [testing, setTesting] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  const [testResult, setTestResult] = useState<HardwareTestResult | null>(null);
  const timer = useRef<number | null>(null);

  const load = async () => {
    try {
      const [o, s] = await Promise.all([
        invoke<VideoOptions>("video_options"),
        invoke<AppSettings>("get_settings"),
      ]);
      setOpts(o);
      setSettings(s);
      // Seed the Custom drafts from the validated persisted selection.
      if (s.video_mode === "custom") {
        const first = o.encoders.find((e) => e.available)?.id ?? o.encoders[0]?.id ?? "";
        const id = o.custom?.encoder && o.encoders.some((e) => e.id === o.custom?.encoder)
          ? (o.custom.encoder as string)
          : first;
        const schema = schemaFor(o, id);
        const persisted = o.custom?.encoder === id ? (o.custom?.settings ?? {}) : {};
        const draft: DraftVals = {};
        const auto: Record<string, boolean> = {};
        for (const spec of schema?.options ?? []) {
          if (spec.key in persisted) {
            draft[spec.key] = persisted[spec.key] as string | number | boolean;
            auto[spec.key] = false;
          } else {
            auto[spec.key] = true;
          }
        }
        // Fresh Custom (nothing persisted): bitrate starts at the ladder
        // value so the first Apply records at the expected bitrate instead
        // of the OBS 10 Mbps default.
        if (o.custom == null) {
          const codec = o.encoders.find((e) => e.id === id)?.codec ?? "h264";
          const h = o.current_height || o.max_source_height || 1080;
          draft.bitrate = ladderBitrateFor(o, codec, h);
          auto.bitrate = false;
        }
        setEncId(id);
        setVals(draft);
        setAutoKeys(auto);
        setVideo(o.custom?.video && o.custom.encoder === id ? o.custom.video : defaultVideo(o.current_height || o.max_source_height || 1080));
      }
    } catch (e) {
      setError(String(e));
    }
  };

  useEffect(() => {
    void load();
    invoke<SystemMemory>("system_memory")
      .then((m) => setFreeMb(m.free_mb))
      .catch(() => setFreeMb(null));
    return () => {
      if (timer.current != null) window.clearInterval(timer.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** Wayland: forget the portal choice so the picker opens again (restarts a
   * running buffer so the dialog appears immediately). */
  const changePortalScreen = async () => {
    setError(null);
    try {
      await invoke("clear_portal_token");
      await load();
    } catch (e) {
      setError(String(e));
    }
  };

  const customMode = settings?.video_mode === "custom";
  const bufferSeconds = Number(settings?.buffer_seconds ?? "30") || 30;

  // Effective encoder: explicit pick, else first available (e.g. Custom was
  // just enabled and drafts were never seeded).
  const firstAvail = opts?.encoders.find((e) => e.available)?.id ?? opts?.encoders[0]?.id ?? "";
  const activeId = encId || firstAvail;

  const schema: EncoderSchema | undefined = schemaFor(opts, activeId);
  const colorFormats =
    opts?.encoder_colors.find((c) => c.id === activeId)?.formats ?? ["NV12"];

  /**
   * Drop draft values that are not applicable right now: unknown keys,
   * codec-unsupported options, hidden options, out-of-range/enum values
   * and 10-bit profiles without P010. Keeps drafts and backend validation
   * in sync (user decision: invalid values are discarded, not kept).
   */
  const sanitizeVals = (
    v: DraftVals,
    a: Record<string, boolean>,
    colorFmt: string,
  ): { vals: DraftVals; auto: Record<string, boolean> } => {
    if (!schema) return { vals: v, auto: a };
    const vals: DraftVals = { ...v };
    const auto: Record<string, boolean> = {};
    for (const spec of schema.options) {
      const val = vals[spec.key];
      let keep = !spec.supported ? false : visible(spec, vals, schema.options);
      if (keep && a[spec.key] === false && val !== undefined) {
        if (spec.kind === "enum") {
          keep = spec.values.includes(String(val));
        } else if (spec.kind === "int") {
          keep =
            typeof val === "number" &&
            Number.isFinite(val) &&
            (spec.int_values.length > 0
              ? spec.int_values.includes(val)
              : val >= spec.min && val <= spec.max);
        } else if (spec.kind === "bool") {
          keep = typeof val === "boolean";
        } else {
          keep = typeof val === "string";
        }
      }
      if (keep && colorFmt !== "P010") {
        if (typeof val === "string" && spec.p010_values.includes(val)) keep = false;
        if (typeof val === "number" && spec.p010_ints.includes(val)) keep = false;
      }
      if (!keep || a[spec.key] !== false || val === undefined) {
        delete vals[spec.key];
        auto[spec.key] = true;
      } else {
        auto[spec.key] = false;
      }
    }
    return { vals, auto };
  };

  // Explicit (non-Auto) settings actually sent to the backend.
  const explicitVals = useMemo(() => {
    const out: DraftVals = {};
    for (const [k, v] of Object.entries(vals)) {
      if (!autoKeys[k]) out[k] = v;
    }
    return out;
  }, [vals, autoKeys]);

  const effectiveBitrate = useMemo(() => {
    if (customMode && typeof explicitVals.bitrate === "number") return explicitVals.bitrate;
    if (!opts) return 20000;
    const row = opts.heights.find((h) => h.height === opts.current_height);
    const idx = Math.max(0, opts.codecs.findIndex((c) => c.id === opts.current_codec));
    if (!row) return 20000;
    return row.bitrates[Math.min(idx, row.bitrates.length - 1)] ?? 20000;
  }, [opts, customMode, explicitVals]);

  const ramMb = mbFor(effectiveBitrate, bufferSeconds);
  const overTwoMin = bufferSeconds > 120;
  const ramLow = freeMb != null && ramMb > Math.max(0, freeMb - 1024);

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

  const pickEncoder = (id: string) => {
    setEncId(id);
    // Fresh encoder: all Auto + ladder bitrate seeded for its codec.
    const fam = schemaFor(opts, id);
    const next: Record<string, boolean> = {};
    for (const spec of fam?.options ?? []) next[spec.key] = true;
    const draft: DraftVals = {};
    const codec = opts?.encoders.find((e) => e.id === id)?.codec ?? "h264";
    if (fam?.options.some((s) => s.key === "bitrate")) {
      draft.bitrate = ladderBitrateFor(opts, codec, video.out_height || 1080);
      next.bitrate = false;
    }
    setVals(draft);
    setAutoKeys(next);
    setTestResult(null);
  };

  const seedAuto = () => {
    const fam = schemaFor(opts, activeId);
    setAutoKeys((prev) => {
      if (Object.keys(prev).length > 0) return prev;
      const next: Record<string, boolean> = {};
      for (const spec of fam?.options ?? []) next[spec.key] = true;
      return next;
    });
  };

  const setOpt = (key: string, v: string | number | boolean) => {
    const clean = sanitizeVals({ ...vals, [key]: v }, { ...autoKeys, [key]: false }, video.color_format);
    setVals(clean.vals);
    setAutoKeys(clean.auto);
  };

  const setAuto = (key: string, auto: boolean) => {
    if (auto) {
      const clean = sanitizeVals(vals, { ...autoKeys, [key]: true }, video.color_format);
      setVals(clean.vals);
      setAutoKeys(clean.auto);
      return;
    }
    // Un-Auto seeds a valid starting value (codec-scoped default, else a
    // safe first value) so the option is actually settable.
    const d = schema?.options.find((o) => o.key === key);
    let def = d ? specDefault(d) : undefined;
    if (def === undefined && d) {
      if (d.kind === "int") {
        def = d.int_values.length > 0 ? d.int_values[0] : d.min;
      } else if (d.kind === "text") {
        def = "";
      }
    }
    const nv = def !== undefined ? { ...vals, [key]: def } : { ...vals };
    const clean = sanitizeVals(nv, { ...autoKeys, [key]: false }, video.color_format);
    setVals(clean.vals);
    setAutoKeys(clean.auto);
  };

  const setVideoField = <K extends keyof CustomVideo>(key: K, v: CustomVideo[K]) => {
    const nv = { ...video, [key]: v };
    setVideo(nv);
    // A color change can invalidate encoder values (10-bit profiles).
    const clean = sanitizeVals(vals, autoKeys, nv.color_format);
    setVals(clean.vals);
    setAutoKeys(clean.auto);
  };

  const resetAuto = () => {
    // Full reset: first auto-detected encoder, all Auto, Video tab back to
    // ladder defaults, bitrate seeded from the ladder.
    const first = opts?.encoders.find((e) => e.available)?.id ?? opts?.encoders[0]?.id ?? "";
    const fam = schemaFor(opts, first);
    const next: Record<string, boolean> = {};
    for (const spec of fam?.options ?? []) next[spec.key] = true;
    const draft: DraftVals = {};
    const codec = opts?.encoders.find((e) => e.id === first)?.codec ?? "h264";
    const h = opts?.current_height || opts?.max_source_height || 1080;
    if (fam?.options.some((s) => s.key === "bitrate")) {
      draft.bitrate = ladderBitrateFor(opts, codec, h);
      next.bitrate = false;
    }
    setEncId(first);
    setVals(draft);
    setAutoKeys(next);
    setVideo(defaultVideo(h));
    setTestResult(null);
  };

  const applyCustom = () =>
    void applySettings([
      { key: "video_mode", value: "custom" },
      { key: "custom_encoder_json", value: JSON.stringify({ encoder: activeId, settings: explicitVals }) },
      { key: "custom_video_json", value: JSON.stringify(video) },
    ]);

  const runTest = async () => {
    setTesting(true);
    setTestResult(null);
    setError(null);
    setElapsed(0);
    timer.current = window.setInterval(() => setElapsed((s) => s + 1), 1000);
    try {
      const r = await invoke<HardwareTestResult>("test_hardware", {
        seconds: TEST_SECONDS,
        encoderId: activeId,
        encoderSettings: explicitVals,
        customVideo: video,
      });
      setTestResult(r);
    } catch (e) {
      setError(String(e));
    } finally {
      if (timer.current != null) window.clearInterval(timer.current);
      setTesting(false);
    }
  };

  const pickHeight = (h: number) => {
    if (!opts) return;
    if (customMode) {
      const w = even((h * 16) / 9);
      setVideo((v) => ({ ...v, out_width: w, out_height: h }));
    } else {
      void applyVideo(opts.current_codec, h, opts.current_fps);
    }
  };

  if (error && !opts) return <p className="font-mono text-xs text-red-400">{error}</p>;
  if (!opts || !settings) return <p className="text-sm text-slate-400">{t("common.loading")}</p>;

  const pill = (active: boolean) =>
    `rounded-md px-2 py-0.5 font-mono text-[11px] transition ${
      active
        ? "bg-cyan-500/20 text-cyan-200"
        : "bg-white/5 text-slate-400 hover:bg-white/10 hover:text-slate-200"
    }`;
  const select =
    "w-full rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-sm text-slate-100 outline-none focus:border-cyan-500/50";
  const num =
    "w-full rounded-lg border border-white/10 bg-white/5 px-2 py-1 font-mono text-xs text-slate-100 outline-none focus:border-cyan-500/50";

  const codecIdx = Math.max(0, opts.codecs.findIndex((c) => c.id === opts.current_codec));
  const overSource = opts.max_source_height > 0 && opts.current_height > opts.max_source_height;

  return (
    <div className="space-y-4">
      {/* Monitor */}
      <label className="block">
        <span className="mb-1 block text-sm text-slate-300">{t("video.monitor")}</span>
        <select
          className={select}
          value={opts.current_monitor}
          onChange={(e) => void applySettings([{ key: "monitor", value: e.target.value }])}
        >
          <option value="">{t("video.monitor_auto")}</option>
          {opts.monitors.map((m) => (
            <option key={m.name} value={m.name}>
              {m.label}
            </option>
          ))}
        </select>
      </label>

      {/* Screen (Linux/Wayland portal): selection lives in the system picker,
          stored as a one-time restore token. */}
      {opts.monitors.length === 0 && (
        <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl border border-white/5 bg-black/30 px-3 py-2.5">
          <div className="min-w-0">
            <p className="text-sm text-slate-300">{t("video.portal_screen")}</p>
            <p className="text-[11px] text-slate-500">
              {opts.portal_ready ? t("video.portal_ready") : t("video.portal_note")}
            </p>
          </div>
          <button
            type="button"
            onClick={() => void changePortalScreen()}
            className="rounded-lg border border-cyan-500/30 bg-cyan-500/10 px-3 py-1.5 text-xs font-semibold text-cyan-200 transition hover:bg-cyan-500/20"
          >
            {t("video.portal_change")}
          </button>
        </div>
      )}

      {/* Preset cards (both modes; in Custom they set the output resolution) */}
      <div className="grid grid-cols-1 gap-1.5 sm:grid-cols-2">
        {opts.heights.map((row) => {
          const bitrate = row.bitrates[Math.min(codecIdx, row.bitrates.length - 1)] ?? 20000;
          const selected = customMode ? row.height === video.out_height : row.height === opts.current_height;
          const moon = MOONS[row.height];
          const name = PLAIN[row.height] ?? moon?.name;
          const recommended =
            row.height === 1080 && !customMode ? t("video.badge_recommended") : null;
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
                {moon?.tag && !customMode && (
                  <span className="text-[10px] uppercase tracking-wide text-slate-500">
                    {t(moon.tag)}
                  </span>
                )}
                {recommended && (
                  <span className="ml-auto rounded-md bg-cyan-500/15 px-1.5 py-0.5 text-[10px] font-semibold uppercase text-cyan-300">
                    {recommended}
                  </span>
                )}
              </div>
              {!customMode && (
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
              )}
            </button>
          );
        })}
      </div>

      {/* Custom toggle */}
      <div className="space-y-2 rounded-xl border border-white/5 bg-black/20 px-3 py-3">
        <label className="flex items-center gap-2 text-sm text-slate-300">
          <input
            type="checkbox"
            checked={customMode}
            onChange={(e) => {
              if (e.target.checked) seedAuto();
              void applySettings([
                { key: "video_mode", value: e.target.checked ? "custom" : "ladder" },
              ]);
            }}
            className="h-3.5 w-3.5 accent-cyan-400"
          />
          {t("video.custom_mode")}
        </label>

        {customMode && (
          <div className="space-y-3 pt-1">
            <p className="text-[11px] text-slate-500">{t("video.custom_title")}</p>

            {/* Encoder picker */}
            <label className="block">
              <span className="mb-1 block text-xs text-slate-400">{t("video.custom_encoder")}</span>
              <select className={select} value={activeId} onChange={(e) => pickEncoder(e.target.value)}>
                {opts.encoders.map((e) => (
                  <option key={e.id} value={e.id} disabled={!e.available}>
                    {e.id} ({e.codec}){e.available ? "" : ` — ${t("video.custom_unavailable")}`}
                  </option>
                ))}
              </select>
            </label>
            {schema && !schema.validated && (
              <p className="text-[11px] text-amber-400">{t("video.custom_unvalidated")}</p>
            )}

            {/* Per-encoder options (unsupported = greyed out, never sent) */}
            {schema?.options
              .filter((s) => visible(s, vals, schema.options))
              .map((s) => {
                const off = !s.supported;
                const p010off =
                  video.color_format !== "P010" &&
                  (s.p010_values.length > 0 || s.p010_ints.length > 0);
                return (
                <div key={s.key} className={`flex items-center gap-2 ${off ? "opacity-40" : ""}`}>
                  <label className="flex shrink-0 cursor-pointer items-center gap-1.5" title="Auto = engine default">
                    <input
                      type="checkbox"
                      checked={!!autoKeys[s.key] || off}
                      disabled={off}
                      onChange={(e) => setAuto(s.key, e.target.checked)}
                      className="h-3 w-3 accent-slate-500"
                    />
                    <span className="text-[10px] uppercase tracking-wide text-slate-500">
                      {t("video.custom_auto")}
                    </span>
                  </label>
                  <div className={`flex-1 ${autoKeys[s.key] || off ? "opacity-50" : ""}`}>
                    <OptionControl
                      spec={s}
                      value={vals[s.key] ?? specDefault(s)}
                      disabled={!!autoKeys[s.key] || busy || off}
                      p010off={p010off}
                      t={t}
                      onChange={(v) => setOpt(s.key, v)}
                    />
                    {off && (
                      <p className="mt-0.5 text-[10px] text-slate-600">
                        {t("video.custom_incompatible", {
                          codec: schema?.codec ?? "?",
                        })}
                      </p>
                    )}
                  </div>
                </div>
                );
              })}

            {/* Video tab */}
            <div className="space-y-2 rounded-lg border border-white/5 bg-black/30 p-2.5">
              <p className="text-xs font-semibold text-slate-300">{t("video.custom_group_video")}</p>
              <div className="grid grid-cols-2 gap-2">
                <label className="block text-[11px] text-slate-400">
                  {t("video.custom_out_w")}
                  <NumberField
                    value={video.out_width}
                    min={2}
                    max={8192}
                    step={2}
                    even
                    className={num}
                    onCommit={(v) => setVideoField("out_width", v)}
                  />
                </label>
                <label className="block text-[11px] text-slate-400">
                  {t("video.custom_out_h")}
                  <NumberField
                    value={video.out_height}
                    min={2}
                    max={8192}
                    step={2}
                    even
                    className={num}
                    onCommit={(v) => setVideoField("out_height", v)}
                  />
                </label>
              </div>
              <div className="grid grid-cols-2 gap-2">
                <label className="block text-[11px] text-slate-400">
                  {t("video.custom_scale")}
                  <select
                    value={video.scale_type}
                    onChange={(e) => setVideoField("scale_type", e.target.value)}
                    className={select}
                  >
                    {opts.scale_filters.map((f) => (
                      <option key={f} value={f}>
                        {f}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="block text-[11px] text-slate-400">
                  {t("video.custom_fps_type")}
                  <select
                    value={video.fps_type}
                    onChange={(e) => setVideoField("fps_type", e.target.value)}
                    className={select}
                  >
                    {opts.fps_types.map((f) => (
                      <option key={f} value={f}>
                        {f}
                      </option>
                    ))}
                  </select>
                </label>
              </div>
              {video.fps_type === "common" && (
                <label className="block text-[11px] text-slate-400">
                  {t("video.custom_fps_value")}
                  <select
                    value={String(video.fps_common)}
                    onChange={(e) => setVideoField("fps_common", Number(e.target.value))}
                    className={select}
                  >
                    {opts.fps_common_values.map((f) => (
                      <option key={f} value={String(f)}>
                        {f}
                      </option>
                    ))}
                  </select>
                </label>
              )}
              {video.fps_type === "integer" && (
                <label className="block text-[11px] text-slate-400">
                  {t("video.custom_fps_value")}
                  <NumberField
                    value={video.fps_int}
                    min={1}
                    max={360}
                    className={num}
                    onCommit={(v) => setVideoField("fps_int", v)}
                  />
                </label>
              )}
              {video.fps_type === "fractional" && (
                <div className="grid grid-cols-2 gap-2">
                  <label className="block text-[11px] text-slate-400">
                    {t("video.custom_fps_num")}
                    <NumberField
                      value={video.fps_num}
                      min={1}
                      max={1000000}
                      className={num}
                      onCommit={(v) => setVideoField("fps_num", v)}
                    />
                  </label>
                  <label className="block text-[11px] text-slate-400">
                    {t("video.custom_fps_den")}
                    <NumberField
                      value={video.fps_den}
                      min={1}
                      max={1000000}
                      className={num}
                      onCommit={(v) => setVideoField("fps_den", v)}
                    />
                  </label>
                </div>
              )}
              <div className="grid grid-cols-3 gap-2">
                <label className="block text-[11px] text-slate-400">
                  {t("video.custom_color_format")}
                  <select
                    value={video.color_format}
                    onChange={(e) => setVideoField("color_format", e.target.value)}
                    className={select}
                  >
                    {colorFormats.map((f) => (
                      <option key={f} value={f}>
                        {f}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="block text-[11px] text-slate-400">
                  {t("video.custom_color_space")}
                  <select
                    value={video.color_space}
                    onChange={(e) => setVideoField("color_space", e.target.value)}
                    className={select}
                  >
                    {opts.color_spaces.map((f) => (
                      <option key={f} value={f}>
                        {f}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="block text-[11px] text-slate-400">
                  {t("video.custom_color_range")}
                  <select
                    value={video.color_range}
                    onChange={(e) => setVideoField("color_range", e.target.value)}
                    className={select}
                  >
                    {opts.color_ranges.map((f) => (
                      <option key={f} value={f}>
                        {f}
                      </option>
                    ))}
                  </select>
                </label>
              </div>
            </div>

            {/* Apply / reset / test */}
            <div className="flex flex-wrap items-center gap-2">
              <button
                onClick={applyCustom}
                disabled={busy || !activeId}
                className="rounded-lg border border-cyan-500/30 bg-cyan-500/10 px-2.5 py-1 text-xs font-semibold text-cyan-200 transition hover:bg-cyan-500/20 disabled:opacity-50"
              >
                {t("video.apply")}
              </button>
              <button
                onClick={resetAuto}
                disabled={busy}
                className="rounded-lg border border-white/10 bg-white/5 px-2.5 py-1 text-xs text-slate-300 transition hover:border-white/20 disabled:opacity-50"
              >
                {t("video.custom_reset_auto")}
              </button>
              <button
                onClick={() => void runTest()}
                disabled={testing || busy || !activeId}
                className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-2.5 py-1 text-xs font-semibold text-emerald-200 transition hover:bg-emerald-500/20 disabled:opacity-50"
              >
                {testing ? t("video.custom_testing", { s: elapsed }) : t("video.custom_test")}
              </button>
            </div>
            {testResult && (
              <p className={`font-mono text-[11px] ${testResult.ok ? "text-emerald-300" : "text-amber-300"}`}>
                {testResult.ok
                  ? t("video.custom_test_ok", {
                      codec: testResult.probe?.codec_name ?? testResult.codec,
                      width: testResult.probe?.width ?? "?",
                      height: testResult.probe?.height ?? testResult.height,
                      fps: testResult.probe ? testResult.probe.fps.toFixed(2) : testResult.fps,
                      size: Math.round(testResult.size_bytes / 1024 / 1024),
                    })
                  : t("video.custom_test_fail", { err: testResult.error ?? "?" })}
              </p>
            )}
            {(() => {
              const summary = appliedSummary(t, opts.custom);
              return summary != null ? (
                <p className="font-mono text-[11px] text-slate-500">{summary}</p>
              ) : null;
            })()}
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
          <NumberField
            value={customSeconds.trim() === "" ? undefined : Number(customSeconds)}
            min={5}
            max={3600}
            step={5}
            ariaLabel={t("video.custom")}
            placeholder={t("video.custom")}
            className="w-20 rounded-lg border border-white/10 bg-white/5 px-2 py-1 text-xs text-slate-100 outline-none focus:border-cyan-500/50"
            onCommit={(v) => setCustomSeconds(String(v))}
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
      {overSource && !customMode && <p className="text-xs text-amber-400">{t("video.upscale_warn")}</p>}
      <p className="text-[11px] text-slate-600">{t("video.restart_notice")}</p>

      {error && <p className="break-all font-mono text-xs text-red-400">{error}</p>}
      {busy && <p className="font-mono text-[11px] text-cyan-300">…</p>}
    </div>
  );
}

function appliedSummary(
  t: (k: string, o?: Record<string, string | number>) => string,
  custom: { encoder: string; settings: Record<string, string | number | boolean> } | null,
): string | null {
  if (!custom) return null;
  const keys = Object.keys(custom.settings);
  if (keys.length === 0) return t("video.custom_applied_auto");
  const list = keys.map((k) => `${k}=${String(custom.settings[k])}`).join(", ");
  return t("video.custom_applied", { encoder: custom.encoder, list });
}

function OptionControl({
  spec,
  value,
  disabled,
  p010off,
  onChange,
  t,
}: {
  spec: OptionSpecJson;
  value: string | number | boolean | undefined;
  disabled: boolean;
  /** True when the color format is not P010: 10-bit values are disabled. */
  p010off: boolean;
  onChange: (v: string | number | boolean) => void;
  t: (k: string) => string;
}) {
  const label = <span className="mb-0.5 block text-xs text-slate-300">{t(spec.i18n)}</span>;
  const ctl =
    "w-full rounded-lg border border-white/10 bg-white/5 px-2 py-1 text-xs text-slate-100 outline-none focus:border-cyan-500/50 disabled:opacity-60";
  // Values requiring P010 while another format is active: not selectable.
  const enumOff = (v: string) => p010off && spec.p010_values.includes(v);
  const intOff = (v: number) => p010off && spec.p010_ints.includes(v);
  if (spec.kind === "bool") {
    return (
      <label className="flex items-center gap-2 text-xs text-slate-300">
        <input
          type="checkbox"
          checked={value === true}
          disabled={disabled}
          onChange={(e) => onChange(e.target.checked)}
          className="h-3.5 w-3.5 accent-cyan-400"
        />
        {t(spec.i18n)}
      </label>
    );
  }
  if (spec.kind === "enum") {
    return (
      <label className="block">
        {label}
        <select
          value={value === undefined ? "" : String(value)}
          disabled={disabled}
          onChange={(e) => onChange(e.target.value)}
          className={ctl}
        >
          {value === undefined && <option value="">—</option>}
          {spec.values.map((v) => (
            <option key={v} value={v} disabled={enumOff(v)}>
              {v}
            </option>
          ))}
        </select>
      </label>
    );
  }
  if (spec.kind === "int") {
    const list = spec.int_values.length > 0;
    return (
      <label className="block">
        {label}
        {list ? (
          <select
            value={value === undefined ? "" : String(value)}
            disabled={disabled}
            onChange={(e) => onChange(Number(e.target.value))}
            className={ctl}
          >
            {value === undefined && <option value="">—</option>}
            {spec.int_values.map((v) => (
              <option key={v} value={String(v)} disabled={intOff(v)}>
                {v}
              </option>
            ))}
          </select>
        ) : (
          <NumberField
            value={typeof value === "number" ? value : undefined}
            min={spec.min}
            max={spec.max}
            step={spec.step || 1}
            disabled={disabled}
            className={`${ctl} font-mono`}
            onCommit={(v) => onChange(v)}
          />
        )}
      </label>
    );
  }
  return (
    <label className="block">
      {label}
      <input
        type="text"
        value={typeof value === "string" ? value : ""}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
        placeholder="key=value key2=value2"
        className={`${ctl} font-mono`}
      />
    </label>
  );
}
