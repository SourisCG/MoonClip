import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { TrackMixer } from "./TrackMixer";
import { Checkbox } from "../ui/Field";
import { Select } from "../ui/Select";
import type { EngineStatus } from "../../hooks/useEngine";

interface AudioDevice {
  id: string;
  description: string;
  kind: string;
}

const DEFAULT_IDS = ["default_output", "default_input"];

/** Capture audio: device selectors + the OBS-style mixer (Settings section). */
export function AudioSection({ status }: { status: EngineStatus }) {
  const { t } = useTranslation();
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [mic, setMic] = useState("default_input");
  const [desktop, setDesktop] = useState("default_output");
  const [error, setError] = useState<string | null>(null);
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

  const mics = devices.filter((d) => d.kind === "mic");
  const desktops = devices.filter((d) => d.kind === "desktop");

  const options = (list: AudioDevice[], current: string) => {
    const entries = list.map((d) => ({ value: d.id, label: label(d) }));
    if (!entries.some((e) => e.value === current)) {
      entries.unshift({ value: current, label: current });
    }
    return entries;
  };

  return (
    <div className="space-y-3">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <div>
          <span className="mb-1.5 block text-xs font-medium text-ink-muted">{t("audio.mic_src")}</span>
          <Select
            value={mic}
            options={options(mics, mic)}
            onChange={(value) => void changeDevice("mic_device", value)}
          />
        </div>
        <div>
          <span className="mb-1.5 block text-xs font-medium text-ink-muted">
            {t("audio.desktop_src")}
          </span>
          <Select
            value={desktop}
            options={options(desktops, desktop)}
            onChange={(value) => void changeDevice("desktop_device", value)}
          />
        </div>
      </div>

      <TrackMixer running={status.running} />

      <label className="flex items-start gap-2 rounded-control border border-line bg-black/30 px-3 py-2">
        <Checkbox
          checked={singleTrack}
          onChange={(e) => void toggleSingleTrack(e.target.checked)}
        />
        <span>
          <span className="block text-sm text-ink-soft">{t("audio.single_track")}</span>
          <span className="block text-[11px] text-ink-faint">{t("audio.single_track_hint")}</span>
        </span>
      </label>
      {singleTrack ? (
        <p className="text-[11px] text-ink-faint">{t("audio.single_track_note")}</p>
      ) : (
        <p className="text-[11px] text-ink-faint">{t("audio.save_tracks_note")}</p>
      )}

      <p className="font-mono text-xs text-ink-muted">
        {status.running
          ? t("audio.linked", { n: status.tracks_linked })
          : t("audio.stopped_hint")}
        {status.audio_error && <span className="text-brand-bright"> · {status.audio_error}</span>}
        {error && <span className="text-brand-bright"> · {error}</span>}
      </p>
    </div>
  );
}
