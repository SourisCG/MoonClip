import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { Volume2, VolumeX } from "lucide-react";

interface Gains {
  game: number;
  mic: number;
  mute_game: boolean;
  mute_mic: boolean;
}

type Track = "game" | "mic";

/**
 * Capture mixer: per-track gain (0-200 %) + mute. Gains apply LIVE through
 * obs-cmd while the buffer runs (no restart); the backend falls back to a
 * single restart only when live apply fails. Errors are shown, never
 * swallowed: a slider that moves must change the volume or say why not.
 */
export function TrackMixer() {
  const { t } = useTranslation();
  const [gains, setGains] = useState<Gains>({ game: 100, mic: 100, mute_game: false, mute_mic: false });
  const [error, setError] = useState<string | null>(null);
  const [applying, setApplying] = useState<Track | null>(null);

  useEffect(() => {
    invoke<Gains>("audio_levels")
      .then(setGains)
      .catch((e) => setError(String(e)));
  }, []);

  const refresh = () => {
    invoke<Gains>("audio_levels")
      .then(setGains)
      .catch((e) => setError(String(e)));
  };

  const commitGain = async (track: Track, pct: number) => {
    setApplying(track);
    setError(null);
    try {
      const g = await invoke<Gains>("set_track_gain", { track, percent: pct });
      setGains(g);
    } catch (e) {
      setError(String(e));
      refresh();
    } finally {
      setApplying(null);
    }
  };

  const commitMute = async (track: Track, muted: boolean) => {
    setGains((g) => (track === "game" ? { ...g, mute_game: muted } : { ...g, mute_mic: muted }));
    setError(null);
    try {
      const g = await invoke<Gains>("set_track_mute", { track, muted });
      setGains(g);
    } catch (e) {
      setError(String(e));
      refresh();
    }
  };

  const row = (track: Track, value: number, muted: boolean) => (
    <div className="flex items-center gap-2">
      <button
        onClick={() => void commitMute(track, !muted)}
        className={`rounded-md p-1 transition ${muted ? "text-red-400" : "text-slate-400 hover:text-slate-200"}`}
        title={track === "game" ? t("rec.game") : t("rec.mic")}
      >
        {muted ? <VolumeX size={13} /> : <Volume2 size={13} />}
      </button>
      <span className="w-8 text-[11px] text-slate-400">
        {track === "game" ? t("rec.game") : t("rec.mic")}
      </span>
      <input
        type="range"
        min={0}
        max={200}
        value={value}
        disabled={muted || applying === track}
        onChange={(e) => {
          const v = Number(e.target.value);
          setGains((g) => (track === "game" ? { ...g, game: v } : { ...g, mic: v }));
        }}
        onPointerUp={(e) => void commitGain(track, Number((e.target as HTMLInputElement).value))}
        onKeyUp={(e) => void commitGain(track, Number((e.target as HTMLInputElement).value))}
        onBlur={(e) => void commitGain(track, Number((e.target as HTMLInputElement).value))}
        className="h-1 flex-1 cursor-pointer appearance-none rounded-full bg-white/10 accent-cyan-400 disabled:opacity-40"
      />
      <span className="w-9 text-right font-mono text-[11px] text-slate-300">
        {applying === track ? "…" : `${value}%`}
      </span>
    </div>
  );

  return (
    <div className="space-y-1.5 rounded-xl border border-white/5 bg-black/30 p-2.5">
      {row("game", gains.game, gains.mute_game)}
      {row("mic", gains.mic, gains.mute_mic)}
      <p className="px-1 pt-0.5 text-[10px] text-slate-600">{t("audio.gain_live")}</p>
      {error && <p className="break-all px-1 font-mono text-[11px] text-red-400">{error}</p>}
    </div>
  );
}
