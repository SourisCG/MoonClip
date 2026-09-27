import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { HardDrive } from "lucide-react";

interface ProviderStatus {
  configured: boolean;
  connected: boolean;
  account: string;
}

interface SocialStatus {
  google_drive: ProviderStatus;
  google_youtube: ProviderStatus;
  tiktok: ProviderStatus;
}

/** Settings → Accounts: connect/disconnect the social providers. Tokens live
 *  in the OS keyring; this component only drives the OAuth flow. */
export function AccountsSection() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<SocialStatus | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setStatus(await invoke<SocialStatus>("social_status"));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const connect = async () => {
    setBusy("google_drive");
    setError(null);
    try {
      setStatus(await invoke<SocialStatus>("connect_google_drive"));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const disconnect = async () => {
    setBusy("google_drive");
    setError(null);
    try {
      setStatus(await invoke<SocialStatus>("disconnect_google_drive"));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const drive = status?.google_drive;
  const row =
    "flex flex-col items-start gap-2 rounded-xl border border-white/5 bg-black/30 px-3 py-3 sm:flex-row sm:items-center sm:justify-between sm:gap-4 sm:px-4";
  const btn =
    "rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-slate-200 transition hover:border-cyan-500/40 hover:text-cyan-200 disabled:opacity-50";

  return (
    <div className="space-y-2">
      <div className={row}>
        <span className="inline-flex items-center gap-2 text-sm text-slate-300">
          <HardDrive size={14} /> {t("accounts.drive")}
        </span>
        <div className="flex min-w-0 items-center gap-2">
          {drive?.connected ? (
            <>
              <span
                className="max-w-[16rem] truncate text-xs text-emerald-300/90"
                title={drive.account}
              >
                {drive.account || t("accounts.connected")}
              </span>
              <button className={btn} onClick={() => void disconnect()} disabled={busy !== null}>
                {t("accounts.disconnect")}
              </button>
            </>
          ) : drive?.configured ? (
            <button className={btn} onClick={() => void connect()} disabled={busy !== null}>
              {busy === "google_drive" ? t("accounts.connecting") : t("accounts.connect")}
            </button>
          ) : (
            <span className="text-xs text-slate-500">{t("accounts.not_configured")}</span>
          )}
        </div>
      </div>
      {error && <p className="break-all font-mono text-xs text-red-400">{error}</p>}
    </div>
  );
}
