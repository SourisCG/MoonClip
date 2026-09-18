import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { MoonClipLogo } from "../logo/MoonClipLogo";
import type { VideoOptions } from "../../types";

/** Moon names stay in English in every language (SPEC §17.1). */
const MOONS = [
  { name: "New Moon", height: 720, tag: null },
  { name: "First Quarter", height: 1080, tag: "quality.tag_recommended" },
  { name: "Waning Gibbous", height: 1440, tag: "quality.tag_high" },
  { name: "Full Moon", height: 2160, tag: "quality.tag_ultra" },
];

export function SetupWizard({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const [opts, setOpts] = useState<VideoOptions | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<VideoOptions>("video_options")
      .then(setOpts)
      .catch((e) => setError(String(e)));
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
              disabled={busy || !opts}
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

        <p className="mt-3 text-[11px] text-slate-500">{t("quality.custom")}: Settings → {t("quality.title")}</p>

        {error && <p className="mt-2 break-all font-mono text-xs text-red-400">{error}</p>}

        <div className="mt-4 flex items-center justify-end gap-2">
          <button
            onClick={() => void skip()}
            className="rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-slate-300 transition hover:border-white/20"
          >
            {t("wizard.skip")}
          </button>
          <button
            onClick={() => void apply(suggested)}
            disabled={busy || !opts}
            className="rounded-lg border border-cyan-500/40 bg-cyan-500/10 px-3 py-1.5 text-xs font-semibold text-cyan-200 transition hover:bg-cyan-500/20 disabled:opacity-50"
          >
            {t("wizard.start")}
          </button>
        </div>
      </div>
    </div>
  );
}
