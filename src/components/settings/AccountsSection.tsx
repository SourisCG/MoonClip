import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { HardDrive, MessageSquare, Music2, RefreshCw, SquarePlay } from "lucide-react";
import type { ReactNode } from "react";

interface ProviderStatus {
  configured: boolean;
  connected: boolean;
  account: string;
}

interface SocialStatus {
  google_drive: ProviderStatus;
  google_youtube: ProviderStatus;
  discord: ProviderStatus;
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
    "flex flex-col items-start gap-2 rounded-card border border-line bg-base/50 px-3 py-3 sm:flex-row sm:items-center sm:justify-between sm:gap-4 sm:px-4";
  const btn =
    "rounded-control border border-line bg-raised/60 px-3 py-1.5 text-xs text-ink transition hover:border-warn/50 hover:text-ink disabled:opacity-50";
  return (
    <div className={row}>
      <span className="inline-flex items-center gap-2 text-sm text-ink-soft">
        {icon} {label}
      </span>
      <div className="flex min-w-0 items-center gap-2">
        {status?.connected ? (
          <>
            <span
              className="max-w-[16rem] truncate text-xs text-ok-bright"
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
          <span className="text-xs text-ink-faint">{t("accounts.not_configured")}</span>
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
  const [discordMaxMb, setDiscordMaxMb] = useState("10");
  const [restoreMsg, setRestoreMsg] = useState<string | null>(null);

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

  useEffect(() => {
    invoke<Record<string, string>>("get_settings")
      .then((s) => setDiscordMaxMb(s.discord_max_mb || "10"))
      .catch(() => {});
  }, []);

  const changeDiscordLimit = (value: string) => {
    setDiscordMaxMb(value);
    void invoke("set_setting", { key: "discord_max_mb", value }).catch(() => {});
  };

  const restoreDrive = async () => {
    setBusy("drive-restore");
    setError(null);
    setRestoreMsg(null);
    try {
      const res = await invoke<{ restored: number }>("drive_sync_library");
      setRestoreMsg(
        res.restored > 0
          ? t("accounts.restore_done", { n: res.restored })
          : t("accounts.restore_none"),
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

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
      {status?.google_drive.connected && (
        <div className="flex flex-wrap items-center gap-2 rounded-card border border-line bg-base/40 px-3 py-2 sm:px-4">
          <button
            onClick={() => void restoreDrive()}
            disabled={busy !== null}
            className="rounded-control border border-line bg-raised/60 px-3 py-1.5 text-xs text-ink transition hover:border-warn/50 hover:text-ink disabled:opacity-50"
          >
            <span className="inline-flex items-center gap-1.5">
              <RefreshCw size={12} className={busy === "drive-restore" ? "animate-spin" : ""} />
              {busy === "drive-restore" ? t("accounts.restoring") : t("accounts.restore")}
            </span>
          </button>
          <span className="text-[11px] text-ink-faint">
            {restoreMsg ?? t("accounts.restore_hint")}
          </span>
        </div>
      )}
      <ProviderRow
        icon={<SquarePlay size={14} />}
        label={t("accounts.youtube")}
        status={status?.google_youtube}
        connecting={busy === "google_youtube"}
        disabled={busy !== null}
        onConnect={() => void run("google_youtube", "connect_google_youtube")}
        onDisconnect={() => void run("google_youtube", "disconnect_google_youtube")}
      />
      <ProviderRow
        icon={<MessageSquare size={14} />}
        label={t("accounts.discord")}
        status={status?.discord}
        connecting={busy === "discord"}
        disabled={busy !== null}
        onConnect={() => void run("discord", "connect_discord")}
        onDisconnect={() => void run("discord", "disconnect_discord")}
      />
      <ProviderRow
        icon={<Music2 size={14} />}
        label={t("accounts.tiktok")}
        status={status?.tiktok}
        connecting={busy === "tiktok"}
        disabled={busy !== null}
        onConnect={() => void run("tiktok", "connect_tiktok")}
        onDisconnect={() => void run("tiktok", "disconnect_tiktok")}
      />
      {status?.discord.connected && (
        <div className="flex flex-wrap items-center gap-2 rounded-card border border-line bg-base/40 px-3 py-2 sm:px-4">
          <span className="text-xs text-ink-muted">{t("accounts.discord_limit")}</span>
          <select
            value={discordMaxMb}
            onChange={(e) => changeDiscordLimit(e.target.value)}
            className="rounded-control border border-line bg-raised/60 px-2 py-1 text-xs text-ink outline-none focus:border-warn/60"
          >
            {["10", "25", "50", "100"].map((mb) => (
              <option key={mb} value={mb}>
                {mb} MB
              </option>
            ))}
          </select>
          <span className="text-[11px] text-ink-faint">{t("accounts.discord_limit_hint")}</span>
        </div>
      )}
      {error && <p className="break-all font-mono text-xs text-brand-bright">{error}</p>}
    </div>
  );
}
