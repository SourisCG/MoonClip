import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { HardDrive, SquarePlay } from "lucide-react";
import type { ReactNode } from "react";

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

function ProviderRow({
  icon,
  label,
  status,
  connecting,
  disabled,
  onConnect,
  onDisconnect,
}: {
  icon: ReactNode;
  label: string;
  status: ProviderStatus | undefined;
  connecting: boolean;
  disabled: boolean;
  onConnect: () => void;
  onDisconnect: () => void;
}) {
  const { t } = useTranslation();
  const row =
    "flex flex-col items-start gap-2 rounded-xl border border-white/5 bg-black/30 px-3 py-3 sm:flex-row sm:items-center sm:justify-between sm:gap-4 sm:px-4";
  const btn =
    "rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-slate-200 transition hover:border-cyan-500/40 hover:text-cyan-200 disabled:opacity-50";
  return (
    <div className={row}>
      <span className="inline-flex items-center gap-2 text-sm text-slate-300">
        {icon} {label}
      </span>
      <div className="flex min-w-0 items-center gap-2">
        {status?.connected ? (
          <>
            <span
              className="max-w-[16rem] truncate text-xs text-emerald-300/90"
              title={status.account}
            >
              {status.account || t("accounts.connected")}
            </span>
            <button className={btn} onClick={onDisconnect} disabled={disabled}>
              {t("accounts.disconnect")}
            </button>
          </>
        ) : status?.configured ? (
          <button className={btn} onClick={onConnect} disabled={disabled}>
            {connecting ? t("accounts.connecting") : t("accounts.connect")}
          </button>
        ) : (
          <span className="text-xs text-slate-500">{t("accounts.not_configured")}</span>
        )}
      </div>
    </div>
  );
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

  const run = async (key: string, command: string) => {
    setBusy(key);
    setError(null);
    try {
      setStatus(await invoke<SocialStatus>(command));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="space-y-2">
      <ProviderRow
        icon={<HardDrive size={14} />}
        label={t("accounts.drive")}
        status={status?.google_drive}
        connecting={busy === "google_drive"}
        disabled={busy !== null}
        onConnect={() => void run("google_drive", "connect_google_drive")}
        onDisconnect={() => void run("google_drive", "disconnect_google_drive")}
      />
      <ProviderRow
        icon={<SquarePlay size={14} />}
        label={t("accounts.youtube")}
        status={status?.google_youtube}
        connecting={busy === "google_youtube"}
        disabled={busy !== null}
        onConnect={() => void run("google_youtube", "connect_google_youtube")}
        onDisconnect={() => void run("google_youtube", "disconnect_google_youtube")}
      />
      {error && <p className="break-all font-mono text-xs text-red-400">{error}</p>}
    </div>
  );
}
