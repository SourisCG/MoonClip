import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { TrackMixer } from "./TrackMixer";
import type { EngineStatus } from "../../hooks/useEngine";

interface AudioDevice {
  id: string;
  description: string;
  kind: string;
}

interface AudioPeaks {
  game: number;
  mic: number;
}

const DEFAULT_IDS = ["default_output", "default_input"];

/** Capture audio: device selectors + live gains + link status (Settings section). */
export function AudioSection({ status }: { status: EngineStatus }) {
  const { t } = useTranslation();
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [mic, setMic] = useState("default_input");
  const [desktop, setDesktop] = useState("default_output");
  const [error, setError] = useState<string | null>(null);
  const [peaks, setPeaks] = useState<AudioPeaks | null>(null);
  const [singleTrack, setSingleTrack] = useState(false);

  useEffect(() => {
    invoke<AudioDevice[]>("list_audio_devices").then(setDevices).catch((e) => setError(String(e)));
    invoke<Record<string, string>>("get_settings")
      .then((s) => {
        if (s.mic_device) setMic(s.mic_device);
        if (s.desktop_device) setDesktop(s.desktop_device);
        setSingleTrack(s.audio_single_track === "1" || s.audio_single_track === "true");
      })
      .catch(console.error);
  }, []);

  // Live signal meters while the buffer runs (Windows; null on Linux).
  useEffect(() => {
    if (!status.running) {
      setPeaks(null);
      return;
    }
    let cancelled = false;
    const tick = () => {
      invoke<AudioPeaks | null>("audio_peaks")
        .then((p) => {
          if (!cancelled) setPeaks(p);
        })
        .catch(() => {});
    };
    tick();
    const id = window.setInterval(tick, 500);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [status.running]);

  const changeDevice = async (key: "mic_device" | "desktop_device", value: string) => {
    setError(null);
    try {
      await invoke("set_setting", { key, value });
      if (key === "mic_device") setMic(value);
      else setDesktop(value);
    } catch (e) {
      setError(String(e));
    }
  };

  const toggleSingleTrack = async (on: boolean) => {
    setError(null);
    const previous = singleTrack;
    setSingleTrack(on);
    try {
      await invoke("set_setting", { key: "audio_single_track", value: on ? "1" : "0" });
    } catch (e) {
      setSingleTrack(previous);
      setError(String(e));
    }
  };

  const label = (d: AudioDevice) =>
    DEFAULT_IDS.includes(d.id)
      ? t("audio.device_default", { name: d.description || "—" })
      : d.description;

  const meter = (track: "game" | "mic") => {
    if (!peaks) return null;
    const level = track === "game" ? peaks.game : peaks.mic;
    const pct = Math.min(100, Math.round(Math.sqrt(Math.min(level, 1)) * 100));
    const db = level > 0.0005 ? `${(20 * Math.log10(level)).toFixed(1)} dB` : "-inf";
    return (
      <div className="rounded-lg border border-white/5 bg-black/20 px-3 py-2">
        <div className="mb-1 flex items-center justify-between text-xs text-slate-400">
          <span>{t(track === "game" ? "rec.game" : "rec.mic")}</span>
          <span className="font-mono text-[11px]">{db}</span>
        </div>
        <div className="h-1.5 w-full overflow-hidden rounded bg-white/5">
          <div
            className={`h-full rounded transition-[width] duration-200 ${
              level > 0.999 ? "bg-red-400" : "bg-cyan-400/80"
            }`}
            style={{ width: `${pct}%` }}
          />
        </div>
      </div>
    );
  };

  const mics = devices.filter((d) => d.kind === "mic");
  const desktops = devices.filter((d) => d.kind === "desktop");
  const select =
    "w-full rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-sm text-slate-100 outline-none focus:border-cyan-500/50";

  return (
    <div className="space-y-3">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <label className="block">
          <span className="mb-1 block text-sm text-slate-300">{t("audio.mic_src")}</span>
          <select className={select} value={mic} onChange={(e) => void changeDevice("mic_device", e.target.value)}>
            {!mics.some((d) => d.id === mic) && <option value={mic}>{mic}</option>}
            {mics.map((d) => (
              <option key={d.id} value={d.id}>
                {label(d)}
              </option>
            ))}
          </select>
        </label>
        <label className="block">
          <span className="mb-1 block text-sm text-slate-300">{t("audio.desktop_src")}</span>
          <select
            className={select}
            value={desktop}
            onChange={(e) => void changeDevice("desktop_device", e.target.value)}
          >
            {!desktops.some((d) => d.id === desktop) && <option value={desktop}>{desktop}</option>}
            {desktops.map((d) => (
              <option key={d.id} value={d.id}>
                {label(d)}
              </option>
            ))}
          </select>
        </label>
      </div>

      {peaks && (
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          {meter("game")}
          {meter("mic")}
        </div>
      )}
      {peaks && status.running && peaks.game <= 0.0005 && (
        <p className="text-xs text-amber-400">{t("audio.no_signal")}</p>
      )}

      <label className="flex items-start gap-2 rounded-lg border border-white/5 bg-black/20 px-3 py-2">
        <input
          type="checkbox"
          checked={singleTrack}
          onChange={(e) => void toggleSingleTrack(e.target.checked)}
          className="mt-0.5 h-3.5 w-3.5 accent-cyan-400"
        />
        <span>
          <span className="block text-sm text-slate-300">{t("audio.single_track")}</span>
          <span className="block text-[11px] text-slate-500">{t("audio.single_track_hint")}</span>
        </span>
      </label>
      {singleTrack ? (
        <p className="text-[11px] text-slate-600">{t("audio.single_track_note")}</p>
      ) : (
        peaks && <p className="text-[11px] text-slate-600">{t("audio.save_tracks_note")}</p>
      )}

      <TrackMixer />

      <p className="font-mono text-xs text-slate-400">
        {status.running
          ? t("audio.linked", { n: status.tracks_linked })
          : t("audio.stopped_hint")}
        {status.audio_error && <span className="text-red-400"> · {status.audio_error}</span>}
        {error && <span className="text-red-400"> · {error}</span>}
      </p>
    </div>
  );
}
