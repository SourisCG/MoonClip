import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { MoonClipLogo } from "../logo/MoonClipLogo";
import type { HardwareTestResult, VideoOptions } from "../../types";

/** Moon names stay in English in every language (SPEC §17.1). */
const MOONS = [
  { name: "New Moon", height: 720, tag: null },
  { name: "First Quarter", height: 1080, tag: "video.tag_recommended" },
  { name: "Waning Gibbous", height: 1440, tag: "video.tag_high" },
  { name: "Full Moon", height: 2160, tag: "video.tag_ultra" },
];

const TEST_SECONDS = 10;

export function SetupWizard({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const [opts, setOpts] = useState<VideoOptions | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [testing, setTesting] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  const [result, setResult] = useState<HardwareTestResult | null>(null);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    invoke<VideoOptions>("video_options")
      .then(setOpts)
      .catch((e) => setError(String(e)));
    return () => {
      if (timer.current != null) window.clearInterval(timer.current);
    };
  }, []);

  const suggested = opts?.vendor === "nvidia" ? 1080 : 720;
  const codec = "h264";

  const ramFor = (height: number): number => {
    if (!opts) return 0;
    const row = opts.heights.find((h) => h.height === height);
    if (!row) return 0;
    const idx = Math.max(0, opts.codecs.findIndex((c) => c.id === codec));
    const kbps = row.bitrates[Math.min(idx, row.bitrates.length - 1)] ?? 20000;
    return Math.round((kbps * 30) / 8000);
  };

  const apply = async (height: number) => {
    setBusy(true);
    setError(null);
    try {
      await invoke("set_video_quality", { codec, height, fps: 60 });
      await invoke("set_settings", {
        values: [
          { key: "buffer_seconds", value: "30" },
          { key: "setup_done", value: "1" },
        ],
      });
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const skip = async () => {
    try {
      await invoke("set_setting", { key: "setup_done", value: "1" });
    } catch {
      // best effort: the wizard can show again next run
    }
    onClose();
  };

  const runTest = async (height: number, fps: number) => {
    setTesting(true);
    setResult(null);
    setError(null);
    setElapsed(0);
    timer.current = window.setInterval(() => setElapsed((s) => s + 1), 1000);
    try {
      const r = await invoke<HardwareTestResult>("test_hardware", {
        height,
        fps,
        seconds: TEST_SECONDS,
      });
      setResult(r);
    } catch (e) {
      setError(String(e));
    } finally {
      if (timer.current != null) window.clearInterval(timer.current);
      setTesting(false);
    }
  };

  const applyResult = async () => {
    if (!result) return;
    await apply(result.height);
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4 backdrop-blur-sm">
      <div className="max-h-[90vh] w-full max-w-2xl overflow-y-auto rounded-2xl border border-white/10 bg-moonclip-panel/95 p-5 shadow-2xl">
        <div className="flex items-center gap-3">
          <MoonClipLogo size={34} />
          <div>
            <h3 className="text-lg font-bold text-slate-100">{t("wizard.title")}</h3>
            <p className="text-xs text-slate-400">{t("wizard.subtitle")}</p>
          </div>
        </div>

        <div className="mt-4 grid grid-cols-1 gap-2 sm:grid-cols-2">
          {MOONS.map((m) => (
            <button
              key={m.height}
              onClick={() => void apply(m.height)}
              disabled={busy || testing || !opts}
              className={`rounded-xl border px-3 py-3 text-left transition disabled:opacity-50 ${
                m.height === suggested
                  ? "border-cyan-500/40 bg-cyan-500/10 hover:bg-cyan-500/15"
                  : "border-white/10 bg-black/20 hover:border-white/20"
              }`}
            >
              <div className="flex items-center justify-between gap-2">
                <span className="text-sm font-semibold text-slate-100">
                  {m.name} — {m.height}p
                </span>
                {m.height === suggested && (
                  <span className="rounded-md bg-cyan-500/20 px-2 py-0.5 text-[10px] font-semibold uppercase text-cyan-200">
                    {t("wizard.suggested")}
                  </span>
                )}
              </div>
              {m.tag && <div className="mt-0.5 text-[11px] text-slate-500">{t(m.tag)}</div>}
              <div className="mt-1 font-mono text-[11px] text-slate-400">
                {t("wizard.ram_note", { ram: ramFor(m.height) })}
              </div>
              {opts && m.height > opts.max_source_height && opts.max_source_height > 0 && (
                <div className="mt-1 text-[11px] text-amber-400">{t("video.upscale_warn")}</div>
              )}
            </button>
          ))}
        </div>

        {/* Optional hardware test */}
        <div className="mt-3 space-y-2 rounded-xl border border-white/5 bg-black/20 px-3 py-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div>
              <p className="text-sm text-slate-300">{t("wizard.test_title")}</p>
              <p className="text-[11px] text-slate-500">
                {t("wizard.test_hint", { seconds: TEST_SECONDS })}
              </p>
            </div>
            <button
              onClick={() => void runTest(suggested, 60)}
              disabled={testing || busy || !opts}
              className="rounded-lg border border-cyan-500/30 bg-cyan-500/10 px-3 py-1.5 text-xs font-semibold text-cyan-200 transition hover:bg-cyan-500/20 disabled:opacity-50"
            >
              {testing ? t("wizard.testing", { s: elapsed }) : t("wizard.test")}
            </button>
          </div>

          {result && (
            <div className="space-y-1">
              <p
                className={`font-mono text-[11px] ${
                  result.ok ? "text-emerald-300" : "text-amber-300"
                }`}
              >
                {result.ok
                  ? t("wizard.test_ok", {
                      height: result.height,
                      fps: result.fps,
                      size: Math.round(result.size_bytes / 1024 / 1024),
                    })
                  : t("wizard.test_fail", {
                      height: result.height,
                      err: result.error ?? "?",
                    })}
              </p>
              {result.ok && (
                <div className="flex gap-2">
                  <button
                    onClick={() => void applyResult()}
                    disabled={busy}
                    className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-2.5 py-1 text-xs font-semibold text-emerald-200 transition hover:bg-emerald-500/20 disabled:opacity-50"
                  >
                    {t("wizard.test_use", { height: result.height })}
                  </button>
                </div>
              )}
              {!result.ok && result.fallback_height != null && result.fallback_fps != null && (
                <button
                  onClick={() =>
                    void runTest(result.fallback_height as number, result.fallback_fps as number)
                  }
                  disabled={testing}
                  className="rounded-lg border border-amber-500/30 bg-amber-500/10 px-2.5 py-1 text-xs font-semibold text-amber-200 transition hover:bg-amber-500/20 disabled:opacity-50"
                >
                  {t("wizard.test_retry", {
                    height: result.fallback_height,
                    fps: result.fallback_fps,
                  })}
                </button>
              )}
            </div>
          )}
        </div>

        <p className="mt-3 text-[11px] text-slate-500">
          {t("wizard.custom_note")}
        </p>

        {error && <p className="mt-2 break-all font-mono text-xs text-red-400">{error}</p>}

        <div className="mt-4 flex items-center justify-end gap-2">
          <button
            onClick={() => void skip()}
            disabled={testing}
            className="rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-slate-300 transition hover:border-white/20 disabled:opacity-50"
          >
            {t("wizard.skip")}
          </button>
          <button
            onClick={() => void apply(suggested)}
            disabled={busy || testing || !opts}
            className="rounded-lg border border-cyan-500/40 bg-cyan-500/10 px-3 py-1.5 text-xs font-semibold text-cyan-200 transition hover:bg-cyan-500/20 disabled:opacity-50"
          >
            {t("wizard.start")}
          </button>
        </div>
      </div>
    </div>
  );
}
