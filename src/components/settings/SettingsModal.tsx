import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen } from "lucide-react";
import { useSettings } from "../../hooks/useSettings";
import { useLocale } from "../../hooks/useLocale";
import { AccountsSection } from "./AccountsSection";
import { AudioSection } from "./AudioSection";
import { DecodeNotice } from "./DecodeNotice";
import { NumberField } from "./NumberField";
import { ObsEngineSection } from "./ObsEngineSection";
import { VideoSection } from "./VideoSection";
import type { EngineStatus } from "../../hooks/useEngine";

export function SettingsModal({
  engineStatus,
  onHotkeyChange,
  onOpenWizard,
}: {
  engineStatus: EngineStatus;
  onHotkeyChange?: (hotkey: string) => void;
  onOpenWizard?: () => void;
}) {
  const { t } = useTranslation();
  const { locale, setLocale } = useLocale();
  const { settings, loading, error, setSetting } = useSettings();
  const [saving, setSaving] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
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
    setStatus(null);
    try {
      await setSetting(key, value);
    } catch (e) {
      setStatus(String(e));
    } finally {
      setSaving(null);
    }
  };

  const browseDir = async () => {
    setSaving("clips_directory");
    setStatus(null);
    try {
      const dir = await open({
        directory: true,
        multiple: false,
        title: t("settings.browse"),
      });
      if (typeof dir === "string") await setSetting("clips_directory", dir);
    } catch (e) {
      setStatus(String(e));
    } finally {
      setSaving(null);
    }
  };

  if (loading) return <p className="text-sm text-ink-muted">{t("common.loading")}</p>;
  if (error) return <p className="text-sm text-blood-bright">{error}</p>;

  const row = "flex flex-col items-start gap-2 rounded-xl border border-line bg-void/50 px-3 py-3 sm:flex-row sm:items-center sm:justify-between sm:gap-4 sm:px-4";
  const label = "text-sm text-ink-soft";
  const input =
    "w-full rounded-control border border-line bg-raised/60 px-2.5 py-1.5 text-sm text-ink outline-none focus:border-gold/60 sm:w-48";

  return (
    <div className="max-w-2xl space-y-3">
      <div className={row}>
        <span className={label}>{t("settings.clips_dir")}</span>
        <span className="flex w-full min-w-0 items-center gap-2 sm:w-auto">
          <code className="min-w-0 flex-1 truncate font-mono text-xs text-sky-bright sm:max-w-64 sm:flex-none">
            {settings.clips_directory || "—"}
          </code>
          <button
            onClick={() => void browseDir()}
            className="rounded-control border border-line bg-raised/60 p-1.5 text-ink transition hover:border-gold/50 hover:text-ink"
            title={t("settings.browse")}
          >
            <FolderOpen size={15} />
          </button>
        </span>
      </div>

      <div className={row}>
        <span className={label}>{t("settings.max_gb")}</span>
        <NumberField
          value={settings.max_storage_gb ? Number(settings.max_storage_gb) : undefined}
          min={1}
          max={500}
          className={input}
          onCommit={(v) => void save("max_storage_gb", String(v))}
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
            className={`${input} text-left font-mono ${capturing ? "border-gold/60 text-ink" : ""}`}
            title={t("settings.hotkey_edit")}
          >
            {capturing ? t("settings.hotkey_press") : hotkey}
          </button>
          {!capturing && hotkey !== "F9" && (
            <button
              onClick={() => void applyHotkey("F9")}
              className="shrink-0 rounded-control border border-line bg-raised/60 px-2.5 py-1.5 text-xs text-ink-soft transition hover:border-gold/50 hover:text-ink"
            >
              {t("settings.hotkey_reset")}
            </button>
          )}
        </div>
      </div>
      {hotkeyError && (
        <p className="break-all font-mono text-xs text-blood-bright">{hotkeyError}</p>
      )}

      {(status || saving) && (
        <p className="font-mono text-xs text-ink-muted">{saving ? `${saving}…` : status}</p>
      )}

      <h3 className="pt-2 text-sm font-semibold text-ink">{t("accounts.title")}</h3>
      <AccountsSection />

      <h3 className="pt-2 text-sm font-semibold text-ink">{t("audio.title")}</h3>
      <AudioSection status={engineStatus} />

      <div className="flex items-center justify-between gap-2 pt-2">
        <h3 className="text-sm font-semibold text-ink">{t("video.title")}</h3>
        {onOpenWizard && (
          <button
            onClick={onOpenWizard}
            className="rounded-control border border-line bg-raised/60 px-2.5 py-1 text-xs text-ink-soft transition hover:border-gold/50 hover:text-ink"
          >
            {t("wizard.reopen")}
          </button>
        )}
      </div>
      <VideoSection />
      <DecodeNotice />

      <ObsEngineSection />

      <p className="pt-2 font-mono text-[11px] text-ink-faint">
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
