import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen, KeyRound } from "lucide-react";
import { useSettings } from "../../hooks/useSettings";
import { useLocale } from "../../hooks/useLocale";
import { AudioSection } from "./AudioSection";
import { VideoSection } from "./VideoSection";
import type { EngineStatus } from "../../hooks/useEngine";

const SECRET_TEST_ALIAS = "phase2_selftest";

export function SettingsModal({
  engineStatus,
  onHotkeyChange,
}: {
  engineStatus: EngineStatus;
  onHotkeyChange?: (hotkey: string) => void;
}) {
  const { t } = useTranslation();
  const { locale, setLocale } = useLocale();
  const { settings, loading, error, setSetting } = useSettings();
  const [saving, setSaving] = useState<string | null>(null);
  const [secretStatus, setSecretStatus] = useState<string | null>(null);
  const [hotkey, setHotkey] = useState("F9");
  const [capturing, setCapturing] = useState(false);
  const [hotkeyError, setHotkeyError] = useState<string | null>(null);

  useEffect(() => {
    invoke<string>("get_hotkey")
      .then(setHotkey)
      .catch((e) => setHotkeyError(String(e)));
  }, []);

  const applyHotkey = async (value: string) => {
    setHotkeyError(null);
    try {
      const canonical = await invoke<string>("set_hotkey", { hotkey: value });
      setHotkey(canonical);
      onHotkeyChange?.(canonical);
      return true;
    } catch (e) {
      setHotkeyError(String(e));
      return false;
    }
  };

  /**
   * Recorder: Esc cancels, lone modifiers are ignored and bare non-function
   * keys are rejected here (the backend enforces the same rule as defense).
   */
  const onHotkeyKeyDown = (e: React.KeyboardEvent<HTMLButtonElement>) => {
    if (!capturing) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") {
      setCapturing(false);
      setHotkeyError(null);
      return;
    }
    if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;
    const mods: string[] = [];
    if (e.ctrlKey) mods.push("control");
    if (e.altKey) mods.push("alt");
    if (e.shiftKey) mods.push("shift");
    if (e.metaKey) mods.push("super");
    const code = e.code;
    const singleAllowed = /^F([1-9]|1[0-2])$/.test(code) || ["PrintScreen", "Pause"].includes(code);
    if (mods.length === 0 && !singleAllowed) {
      setHotkeyError(t("settings.hotkey_needs_mod"));
      return;
    }
    void applyHotkey([...mods, code].join("+")).then((ok) => {
      if (ok) setCapturing(false);
    });
  };

  const save = async (key: string, value: string) => {
    setSaving(key);
    setSecretStatus(null);
    try {
      await setSetting(key, value);
    } catch (e) {
      setSecretStatus(String(e));
    } finally {
      setSaving(null);
    }
  };

  const browseDir = async () => {
    setSaving("clips_directory");
    setSecretStatus(null);
    try {
      const dir = await open({
        directory: true,
        multiple: false,
        title: t("settings.browse"),
      });
      if (typeof dir === "string") await setSetting("clips_directory", dir);
    } catch (e) {
      setSecretStatus(String(e));
    } finally {
      setSaving(null);
    }
  };

  const secretRoundTrip = async () => {
    setSecretStatus(t("settings.secret.testing"));
    try {
      const probe = `ok-${Date.now()}`;
      await invoke("secret_store", { alias: SECRET_TEST_ALIAS, value: probe });
      const back = await invoke<string>("secret_get", { alias: SECRET_TEST_ALIAS });
      if (back !== probe) throw new Error("mismatch");
      await invoke("secret_delete", { alias: SECRET_TEST_ALIAS });
      setSecretStatus(t("settings.secret.ok"));
    } catch (e) {
      setSecretStatus(String(e));
    }
  };

  if (loading) return <p className="text-sm text-slate-400">{t("common.loading")}</p>;
  if (error) return <p className="text-sm text-red-400">{error}</p>;

  const row = "flex flex-col items-start gap-2 rounded-xl border border-white/5 bg-black/30 px-3 py-3 sm:flex-row sm:items-center sm:justify-between sm:gap-4 sm:px-4";
  const label = "text-sm text-slate-300";
  const input =
    "w-full rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-sm text-slate-100 outline-none focus:border-cyan-500/50 sm:w-48";

  return (
    <div className="max-w-2xl space-y-3">
      <div className={row}>
        <span className={label}>{t("settings.clips_dir")}</span>
        <span className="flex w-full min-w-0 items-center gap-2 sm:w-auto">
          <code className="min-w-0 flex-1 truncate font-mono text-xs text-cyan-300 sm:max-w-64 sm:flex-none">
            {settings.clips_directory || "—"}
          </code>
          <button
            onClick={() => void browseDir()}
            className="rounded-lg border border-white/10 bg-white/5 p-1.5 text-slate-200 transition hover:border-cyan-500/40 hover:text-cyan-200"
            title={t("settings.browse")}
          >
            <FolderOpen size={15} />
          </button>
        </span>
      </div>

      <div className={row}>
        <span className={label}>{t("settings.buffer")}</span>
        <input
          type="number"
          min={5}
          max={300}
          className={input}
          defaultValue={settings.buffer_seconds}
          key={`buf-${settings.buffer_seconds}`}
          onBlur={(e) => void save("buffer_seconds", e.target.value)}
        />
      </div>

      <div className={row}>
        <span className={label}>{t("settings.max_gb")}</span>
        <input
          type="number"
          min={1}
          max={500}
          className={input}
          defaultValue={settings.max_storage_gb}
          key={`gb-${settings.max_storage_gb}`}
          onBlur={(e) => void save("max_storage_gb", e.target.value)}
        />
      </div>

      <div className={row}>
        <span className={label}>{t("lang.label")}</span>
        <select
          className={input}
          value={locale.startsWith("en") ? "en" : "es"}
          onChange={(e) => void setLocale(e.target.value)}
        >
          <option value="es">{t("lang.es")}</option>
          <option value="en">{t("lang.en")}</option>
        </select>
      </div>

      <div className={row}>
        <span className={label}>{t("settings.hotkey")}</span>
        <div className="flex w-full items-center gap-2 sm:w-auto">
          <button
            onClick={() => {
              setHotkeyError(null);
              setCapturing(true);
            }}
            onKeyDown={onHotkeyKeyDown}
            onBlur={() => setCapturing(false)}
            className={`${input} text-left font-mono ${capturing ? "border-cyan-500/50 text-cyan-200" : ""}`}
            title={t("settings.hotkey_edit")}
          >
            {capturing ? t("settings.hotkey_press") : hotkey}
          </button>
          {!capturing && hotkey !== "F9" && (
            <button
              onClick={() => void applyHotkey("F9")}
              className="shrink-0 rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-xs text-slate-300 transition hover:border-cyan-500/40 hover:text-cyan-200"
            >
              {t("settings.hotkey_reset")}
            </button>
          )}
        </div>
      </div>
      {hotkeyError && (
        <p className="break-all font-mono text-xs text-red-400">{hotkeyError}</p>
      )}

      <div className={row}>
        <span className={label}>
          <span className="inline-flex items-center gap-2">
            <KeyRound size={14} /> {t("settings.secret.title")}
          </span>
        </span>
        <button
          onClick={() => void secretRoundTrip()}
          className="rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-slate-200 transition hover:border-cyan-500/40 hover:text-cyan-200"
        >
          {t("settings.secret.test")}
        </button>
      </div>
      {(secretStatus || saving) && (
        <p className="font-mono text-xs text-slate-400">
          {saving ? `${saving}…` : secretStatus}
        </p>
      )}

      <h3 className="pt-2 text-sm font-semibold text-slate-200">{t("audio.title")}</h3>
      <AudioSection status={engineStatus} />

      <h3 className="pt-2 text-sm font-semibold text-slate-200">{t("video.title")}</h3>
      <VideoSection />

      <p className="pt-2 font-mono text-[11px] text-slate-600">
        build {buildId()}
      </p>
    </div>
  );
}

declare const __MOONCLIP_BUILD__: string | undefined;

function buildId(): string {
  try {
    return typeof __MOONCLIP_BUILD__ !== "undefined" ? __MOONCLIP_BUILD__ : "dev";
  } catch {
    return "dev";
  }
}
