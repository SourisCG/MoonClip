import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { Volume2, VolumeX } from "lucide-react";
import { Meter } from "../ui/Meter";
import { clampPercent, percentToDb } from "../../lib/audio";

interface Gains {
  game: number;
  mic: number;
  mute_game: boolean;
  mute_mic: boolean;
}

interface Peaks {
  game: number;
  game_peak: number;
  mic: number;
  mic_peak: number;
}

type Track = "game" | "mic";

/** How often a gain is pushed while dragging (OBS-like live apply). */
const COMMIT_THROTTLE_MS = 140;
/** Meter poll while the buffer runs. */
const METER_POLL_MS = 100;

/**
 * OBS-style capture mixer: horizontal 0–100 faders (100 = maximum), live
 * level meters with green/amber/red zones and a dB readout, per-track mute.
 * Gains apply live through obs-websocket while the buffer runs; a failed
 * apply is shown, never a silent restart.
 */
export function TrackMixer({ running }: { running: boolean }) {
  const { t } = useTranslation();
  const [gains, setGains] = useState<Gains>({ game: 100, mic: 100, mute_game: false, mute_mic: false });
  const [levels, setLevels] = useState<Peaks | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [applying, setApplying] = useState<Track | null>(null);
  const pending = useRef<Record<Track, number | null>>({ game: null, mic: null });
  const timer = useRef<number | null>(null);

  const refresh = () => {
    invoke<Gains>("audio_levels")
      .then(setGains)
      .catch((e) => setError(String(e)));
  };

  useEffect(refresh, []);

  // Level meters while capturing.
  useEffect(() => {
    if (!running) {
      setLevels(null);
      return;
    }
    let alive = true;
    const tick = () => {
      invoke<Peaks | null>("audio_peaks")
        .then((p) => {
          if (alive) setLevels(p);
        })
        .catch(() => {});
    };
    tick();
    const id = window.setInterval(tick, METER_POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, [running]);

  const push = async (track: Track, pct: number) => {
    setApplying(track);
    setError(null);
    try {
      const next = await invoke<Gains>("set_track_gain", { track, percent: pct });
      setGains(next);
    } catch (e) {
      setError(String(e));
      refresh();
    } finally {
      setApplying(null);
    }
  };

  const flush = () => {
    timer.current = null;
    (["game", "mic"] as Track[]).forEach((track) => {
      const value = pending.current[track];
      pending.current[track] = null;
      if (value !== null) void push(track, value);
    });
  };

  const schedule = (track: Track, pct: number) => {
    const value = clampPercent(pct);
    setGains((g) => (track === "game" ? { ...g, game: value } : { ...g, mic: value }));
    pending.current[track] = value;
    if (timer.current === null) timer.current = window.setTimeout(flush, COMMIT_THROTTLE_MS);
  };

  const commitMute = async (track: Track, muted: boolean) => {
    setGains((g) => (track === "game" ? { ...g, mute_game: muted } : { ...g, mute_mic: muted }));
    setError(null);
    try {
      const next = await invoke<Gains>("set_track_mute", { track, muted });
      setGains(next);
    } catch (e) {
      setError(String(e));
      refresh();
    }
  };

  const row = (track: Track, value: number, muted: boolean) => {
    const level = levels ? (track === "game" ? levels.game : levels.mic) : 0;
    const peak = levels ? (track === "game" ? levels.game_peak : levels.mic_peak) : 0;
    return (
      <div className="rounded-control border border-line bg-black/60 px-3 py-2.5">
        <div className="mb-1.5 flex items-center gap-2">
          <button
            onClick={() => void commitMute(track, !muted)}
            className={
              "rounded-control p-1 transition-colors " +
              (muted ? "text-brand-bright" : "text-ink-muted hover:text-ink")
            }
            title={muted ? t("audio.unmute") : t("audio.mute")}
            aria-label={muted ? t("audio.unmute") : t("audio.mute")}
          >
            {muted ? <VolumeX size={14} /> : <Volume2 size={14} />}
          </button>
          <span className="text-xs font-medium text-ink-soft">
            {track === "game" ? t("rec.game") : t("rec.mic")}
          </span>
          <span className="ml-auto font-mono text-[10px] text-ink-faint">
            {applying === track ? "…" : percentToDb(value)} dB
          </span>
        </div>
        <Meter level={muted ? 0 : level} peak={muted ? 0 : peak} showScale className="mb-2" />
        <div className="flex items-center gap-2">
          <input
            type="range"
            min={0}
            max={100}
            step={1}
            value={value}
            disabled={muted}
            aria-label={track === "game" ? t("rec.game") : t("rec.mic")}
            onChange={(e) => schedule(track, Number(e.target.value))}
            onPointerUp={() => {
              if (timer.current !== null) {
                window.clearTimeout(timer.current);
                flush();
              }
            }}
            onBlur={() => {
              if (timer.current !== null) {
                window.clearTimeout(timer.current);
                flush();
              }
            }}
            className="fader w-full"
            style={{ ["--fader-pct" as string]: `${value}%` }}
          />
          <span className="w-10 shrink-0 text-right font-mono text-[11px] text-ink-soft">
            {value}%
          </span>
        </div>
      </div>
    );
  };

  const noSignal =
    running && levels !== null && levels.game <= 0.0005 && levels.mic <= 0.0005;

  return (
    <div className="space-y-2">
      {row("game", gains.game, gains.mute_game)}
      {row("mic", gains.mic, gains.mute_mic)}
      <p className="px-1 text-[11px] text-ink-faint">{t("audio.gain_live")}</p>
      {noSignal && <p className="px-1 text-[11px] text-warn-bright">{t("audio.no_signal")}</p>}
      {error && <p className="break-all px-1 font-mono text-[11px] text-brand-bright">{error}</p>}
    </div>
  );
}
