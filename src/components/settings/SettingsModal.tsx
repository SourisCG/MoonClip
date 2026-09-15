import { useState } from "react";
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

export function SettingsModal({ engineStatus }: { engineStatus: EngineStatus }) {
  const { t } = useTranslation();
  const { locale, setLocale } = useLocale();
  const { settings, loading, error, setSetting } = useSettings();
  const [saving, setSaving] = useState<string | null>(null);
  const [secretStatus, setSecretStatus] = useState<string | null>(null);

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

  const row = "flex items-center justify-between gap-4 rounded-xl border border-white/5 bg-black/30 px-4 py-3";
  const label = "text-sm text-slate-300";
  const input =
    "w-48 rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-sm text-slate-100 outline-none focus:border-cyan-500/50";

  return (
    <div className="max-w-2xl space-y-3">
      <div className={row}>
        <span className={label}>{t("settings.clips_dir")}</span>
        <span className="flex items-center gap-2">
          <code className="max-w-64 truncate font-mono text-xs text-cyan-300">
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
