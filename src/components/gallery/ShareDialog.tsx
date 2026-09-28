import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { CloudUpload, Copy, ExternalLink, HardDriveDownload, MessageSquare, Music2, RefreshCw, Trash2, X, SquarePlay } from "lucide-react";
import type { ClipMetadata } from "../../types";
import { Modal } from "../Modal";

interface UploadResult {
  file_id: string;
  name: string;
  web_link: string | null;
}

interface YouTubeResult {
  video_id: string;
  url: string;
  privacy: string;
}

interface DiscordResult {
  message_id: string;
  url: string | null;
  channel: string;
}

interface TikTokCreator {
  username: string;
  nickname: string;
  privacy_level_options: string[];
  comment_disabled: boolean;
  duet_disabled: boolean;
  stitch_disabled: boolean;
  max_video_post_duration_sec: number;
}

interface TikTokResult {
  publish_id: string;
  status: string;
  privacy: string;
}

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

interface Progress {
  clipId: string;
  provider?: string;
  phase?: string;
  sent: number;
  total: number;
}

function bareName(fileName: string) {
  const i = fileName.lastIndexOf("/");
  return i >= 0 ? fileName.slice(i + 1) : fileName;
}

/** Default YouTube title: the clip name without its extension. */
function defaultTitle(fileName: string) {
  const bare = bareName(fileName);
  const dot = bare.lastIndexOf(".");
  return (dot > 0 ? bare.slice(0, dot) : bare).slice(0, 100);
}

/** Google Drive panel for one clip: upload it (opt-in local deletion) or,
 *  when it is already in Drive, manage the link / local copy / replacement.
 *  Uploads are never duplicated. */
export function ShareDialog({
  clip,
  onClose,
  onUploaded,
}: {
  clip: ClipMetadata;
  onClose: () => void;
  onUploaded?: () => void;
}) {
  const { t } = useTranslation();
  const [makePublic, setMakePublic] = useState(false);
  const [deleteLocal, setDeleteLocal] = useState(false);
  const [confirmReplace, setConfirmReplace] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [result, setResult] = useState<UploadResult | null>(() =>
    clip.drive_file_id
      ? { file_id: clip.drive_file_id, name: bareName(clip.file_name), web_link: clip.drive_web_url ?? null }
      : null,
  );
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  // YouTube: upload-only (title + fixed #MoonClip #moonclip tags).
  const [social, setSocial] = useState<SocialStatus | null>(null);
  const [ytTitle, setYtTitle] = useState(() => defaultTitle(clip.file_name));
  const [ytPrivacy, setYtPrivacy] = useState("private");
  const [ytConfirmed, setYtConfirmed] = useState(false);
  const [ytBusy, setYtBusy] = useState<string | null>(null);
  const [ytProgress, setYtProgress] = useState<Progress | null>(null);
  const [ytResult, setYtResult] = useState<YouTubeResult | null>(null);
  const [ytError, setYtError] = useState<string | null>(null);
  const [ytCopied, setYtCopied] = useState(false);

  // Discord: connect-your-account webhook; over-limit clips compress to 720p.
  const [dcTitle, setDcTitle] = useState(() => defaultTitle(clip.file_name));
  const [dcCompress, setDcCompress] = useState(true);
  const [dcBusy, setDcBusy] = useState<string | null>(null);
  const [dcProgress, setDcProgress] = useState<Progress | null>(null);
  const [dcResult, setDcResult] = useState<DiscordResult | null>(null);
  const [dcError, setDcError] = useState<string | null>(null);
  const [dcCopied, setDcCopied] = useState(false);
  const [discordMaxMb, setDiscordMaxMb] = useState("10");

  // TikTok: Direct Post; privacy options come from creator_info and there is
  // no default value (TikTok's UX requirement).
  const [ttTitle, setTtTitle] = useState(
    () => `${defaultTitle(clip.file_name)} #MoonClip #moonclip`.slice(0, 2200),
  );
  const [ttCreator, setTtCreator] = useState<TikTokCreator | null>(null);
  const [ttPrivacy, setTtPrivacy] = useState("");
  const [ttBusy, setTtBusy] = useState<string | null>(null);
  const [ttProgress, setTtProgress] = useState<Progress | null>(null);
  const [ttResult, setTtResult] = useState<TikTokResult | null>(null);
  const [ttError, setTtError] = useState<string | null>(null);

  const uploaded = !!clip.drive_file_id;
  const cloud = clip.cloud;

  useEffect(() => {
    invoke<Record<string, string>>("get_settings")
      .then((s) => {
        setDeleteLocal(s.share_delete_local === "1");
        setDiscordMaxMb(s.discord_max_mb || "10");
      })
      .catch(() => {});
    invoke<SocialStatus>("social_status").then(setSocial).catch(() => {});
  }, []);

  useEffect(() => {
    const unlisten = listen<Progress>("moonclip://upload-progress", (event) => {
      if (event.payload.clipId === clip.id) setProgress(event.payload);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [clip.id]);

  useEffect(() => {
    const unlisten = listen<Progress>("moonclip://publish-progress", (event) => {
      if (event.payload.clipId === clip.id && event.payload.provider === "youtube") {
        setYtProgress(event.payload);
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [clip.id]);

  useEffect(() => {
    const unlisten = listen<Progress>("moonclip://publish-progress", (event) => {
      if (event.payload.clipId === clip.id && event.payload.provider === "discord") {
        setDcProgress(event.payload);
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [clip.id]);

  useEffect(() => {
    const unlisten = listen<Progress>("moonclip://publish-progress", (event) => {
      if (event.payload.clipId === clip.id && event.payload.provider === "tiktok") {
        setTtProgress(event.payload);
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [clip.id]);

  useEffect(() => {
    if (!social?.tiktok.connected) return;
    let cancelled = false;
    invoke<TikTokCreator>("tiktok_creator_info")
      .then((info) => {
        if (!cancelled) setTtCreator(info);
      })
      .catch((e) => {
        if (!cancelled) setTtError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [social?.tiktok.connected]);

  const toggleDeleteLocal = (value: boolean) => {
    setDeleteLocal(value);
    void invoke("set_setting", {
      key: "share_delete_local",
      value: value ? "1" : "0",
    }).catch(() => {});
  };

  const run = async (op: string, opts: { makePublic: boolean; deleteLocal: boolean; replace: boolean }) => {
    setBusy(op);
    setError(null);
    setProgress(null);
    try {
      const res = await invoke<UploadResult>("drive_upload_clip", {
        clipId: clip.id,
        makePublic: opts.makePublic,
        deleteLocal: opts.deleteLocal,
        replace: opts.replace,
      });
      setResult(res);
      onUploaded?.();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
      setProgress(null);
      setConfirmReplace(false);
    }
  };

  const copyLink = async () => {
    if (!result?.web_link) return;
    try {
      await writeText(result.web_link);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch (e) {
      setError(String(e));
    }
  };

  const openInDrive = async () => {
    const url =
      result?.web_link ?? `https://drive.google.com/file/d/${result?.file_id ?? ""}/view`;
    try {
      await openUrl(url);
    } catch (e) {
      setError(String(e));
    }
  };

  const connectYouTube = async () => {
    setYtBusy("connect");
    setYtError(null);
    try {
      setSocial(await invoke<SocialStatus>("connect_google_youtube"));
    } catch (e) {
      setYtError(String(e));
    } finally {
      setYtBusy(null);
    }
  };

  const uploadYouTube = async () => {
    setYtBusy("upload");
    setYtError(null);
    setYtProgress(null);
    setYtResult(null);
    try {
      setYtResult(
        await invoke<YouTubeResult>("youtube_share_clip", {
          clipId: clip.id,
          title: ytTitle,
          privacy: ytPrivacy,
        }),
      );
    } catch (e) {
      setYtError(String(e));
    } finally {
      setYtBusy(null);
      setYtProgress(null);
    }
  };

  const copyYouTube = async () => {
    if (!ytResult?.url) return;
    try {
      await writeText(ytResult.url);
      setYtCopied(true);
      setTimeout(() => setYtCopied(false), 2000);
    } catch (e) {
      setYtError(String(e));
    }
  };

  const connectDiscord = async () => {
    setDcBusy("connect");
    setDcError(null);
    try {
      setSocial(await invoke<SocialStatus>("connect_discord"));
    } catch (e) {
      setDcError(String(e));
    } finally {
      setDcBusy(null);
    }
  };

  const shareDiscord = async () => {
    setDcBusy("upload");
    setDcError(null);
    setDcProgress(null);
    setDcResult(null);
    try {
      setDcResult(
        await invoke<DiscordResult>("discord_share_clip", {
          clipId: clip.id,
          title: dcTitle,
          compress: dcCompress,
        }),
      );
    } catch (e) {
      setDcError(String(e));
    } finally {
      setDcBusy(null);
      setDcProgress(null);
    }
  };

  const copyDiscord = async () => {
    if (!dcResult?.url) return;
    try {
      await writeText(dcResult.url);
      setDcCopied(true);
      setTimeout(() => setDcCopied(false), 2000);
    } catch (e) {
      setDcError(String(e));
    }
  };

  const connectTikTok = async () => {
    setTtBusy("connect");
    setTtError(null);
    try {
      setSocial(await invoke<SocialStatus>("connect_tiktok"));
    } catch (e) {
      setTtError(String(e));
    } finally {
      setTtBusy(null);
    }
  };

  const publishTikTok = async () => {
    setTtBusy("publish");
    setTtError(null);
    setTtProgress(null);
    setTtResult(null);
    try {
      setTtResult(
        await invoke<TikTokResult>("tiktok_share_clip", {
          clipId: clip.id,
          title: ttTitle,
          privacyLevel: ttPrivacy,
        }),
      );
    } catch (e) {
      setTtError(String(e));
    } finally {
      setTtBusy(null);
      setTtProgress(null);
    }
  };

  const pct =
    progress && progress.total > 0
      ? Math.min(100, Math.round((progress.sent / progress.total) * 100))
      : null;
  const ytPct =
    ytProgress && ytProgress.total > 0
      ? Math.min(100, Math.round((ytProgress.sent / ytProgress.total) * 100))
      : null;
  const dcPct =
    dcProgress && dcProgress.total > 0
      ? Math.min(100, Math.round((dcProgress.sent / dcProgress.total) * 100))
      : null;
  const ttPct =
    ttProgress && ttProgress.total > 0
      ? Math.min(100, Math.round((ttProgress.sent / ttProgress.total) * 100))
      : null;
  const discordOverLimit =
    clip.file_size_bytes > (Number(discordMaxMb) || 10) * 1024 * 1024;
  const btn =
    "inline-flex items-center justify-center gap-1.5 rounded-control border border-line bg-raised/60 px-3 py-1.5 text-xs text-ink transition hover:border-warn/50 hover:text-ink disabled:opacity-50";

  return (
    <Modal>
      <div className="fixed inset-0 z-50 flex items-center justify-center bg-base/80 p-4">
        <div className="max-h-[calc(100vh-2rem)] w-full max-w-md overflow-y-auto rounded-card border border-line bg-surface p-4 shadow-panel">
        <div className="mb-3 flex items-center">
          <h3 className="text-sm font-semibold text-ink">{t("share.title")}</h3>
          <button
            onClick={onClose}
            className="ml-auto rounded-control p-1.5 text-ink-faint transition hover:bg-raised hover:text-ink"
          >
            <X size={15} />
          </button>
        </div>

        <p className="mb-3 truncate font-mono text-xs text-ink-faint" title={clip.file_name}>
          {clip.file_name}
        </p>

        {uploaded ? (
          <div className="space-y-2">
            <p className="flex items-center gap-1.5 text-xs text-ok-bright">
              <CloudUpload size={13} />
              {cloud ? t("share.in_drive_only") : t("share.uploaded")}
            </p>
            <div className="flex flex-wrap gap-2">
              {result?.web_link ? (
                <button onClick={() => void copyLink()} className={btn}>
                  <Copy size={13} />
                  {copied ? t("share.copied") : t("share.copy_link")}
                </button>
              ) : (
                <button
                  onClick={() => void run("public", { makePublic: true, deleteLocal: false, replace: false })}
                  disabled={busy !== null}
                  className={btn}
                >
                  <ExternalLink size={13} />
                  {busy === "public" ? t("share.working") : t("share.make_public")}
                </button>
              )}
              <button onClick={() => void openInDrive()} className={btn}>
                <ExternalLink size={13} /> {t("share.open_drive")}
              </button>
              {!cloud && (
                <button
                  onClick={() => void run("delete-local", { makePublic: false, deleteLocal: true, replace: false })}
                  disabled={busy !== null}
                  className={btn}
                >
                  <HardDriveDownload size={13} />
                  {busy === "delete-local" ? t("share.working") : t("share.delete_local_now")}
                </button>
              )}
              {!cloud &&
                (confirmReplace ? (
                  <button
                    onClick={() => void run("replace", { makePublic: !!result?.web_link, deleteLocal: false, replace: true })}
                    disabled={busy !== null}
                    className="inline-flex items-center gap-1.5 rounded-control bg-brand/20 px-3 py-1.5 text-xs font-medium text-brand-bright transition hover:bg-brand/30 disabled:opacity-50"
                  >
                    <Trash2 size={13} /> {t("share.replace_confirm")}
                  </button>
                ) : (
                  <button
                    onClick={() => {
                      setConfirmReplace(true);
                      setTimeout(() => setConfirmReplace(false), 4000);
                    }}
                    disabled={busy !== null}
                    className={btn}
                  >
                    <RefreshCw size={13} /> {t("share.replace")}
                  </button>
                ))}
            </div>
            {!result?.web_link && (
              <p className="text-[11px] text-ink-faint">{t("share.private_note")}</p>
            )}
          </div>
        ) : (
          <>
            <label className="mb-3 flex cursor-pointer items-start gap-2 rounded-control border border-line bg-base/50 px-3 py-2 text-xs text-ink-soft">
              <input
                type="checkbox"
                checked={makePublic}
                onChange={(e) => setMakePublic(e.target.checked)}
                className="mt-0.5"
              />
              <span>
                {t("share.public")}
                <span className="mt-0.5 block text-ink-faint">
                  {makePublic ? t("share.public_note") : t("share.private_note")}
                </span>
              </span>
            </label>
            <label className="mb-3 flex cursor-pointer items-start gap-2 rounded-control border border-line bg-base/50 px-3 py-2 text-xs text-ink-soft">
              <input
                type="checkbox"
                checked={deleteLocal}
                onChange={(e) => toggleDeleteLocal(e.target.checked)}
                className="mt-0.5"
              />
              <span>
                {t("share.delete_local")}
                <span className="mt-0.5 block text-ink-faint">{t("share.delete_local_note")}</span>
              </span>
            </label>
            <button
              onClick={() => void run("upload", { makePublic, deleteLocal, replace: false })}
              disabled={busy !== null}
              className="flex w-full items-center justify-center gap-2 rounded-control border border-warn/40 bg-link/10 px-3 py-2 text-sm text-ink transition hover:bg-link/20 disabled:opacity-50"
            >
              <CloudUpload size={15} />
              {busy === "upload" ? t("share.uploading") : t("share.drive_upload")}
            </button>
          </>
        )}

        {busy === "upload" && (
          <div className="mt-3">
            <div className="h-1.5 w-full overflow-hidden rounded-full bg-raised">
              <div
                className="h-full rounded-full bg-warn transition-all"
                style={{ width: `${pct ?? 5}%` }}
              />
            </div>
            <p className="mt-1 text-right font-mono text-[10px] text-ink-faint">
              {pct !== null ? `${pct}%` : "…"}
            </p>
          </div>
        )}

        {/* YouTube: upload-only, title + fixed #MoonClip #moonclip tags. */}
        <div className="mt-4 border-t border-line pt-3">
          <h4 className="mb-2 flex items-center gap-1.5 text-xs font-semibold text-ink">
            <SquarePlay size={14} /> {t("youtube.section")}
          </h4>
          {social && !social.google_youtube.configured ? (
            <p className="text-[11px] text-ink-faint">{t("accounts.not_configured")}</p>
          ) : social && !social.google_youtube.connected ? (
            <button
              onClick={() => void connectYouTube()}
              disabled={ytBusy !== null}
              className={btn}
            >
              <SquarePlay size={13} />
              {ytBusy === "connect" ? t("accounts.connecting") : t("youtube.connect")}
            </button>
          ) : ytResult ? (
            <div className="space-y-2">
              <p className="flex items-center gap-1.5 text-xs text-ok-bright">
                <CloudUpload size={13} /> {t("youtube.uploaded")}
              </p>
              <div className="flex flex-wrap gap-2">
                <button onClick={() => void copyYouTube()} className={btn}>
                  <Copy size={13} />
                  {ytCopied ? t("share.copied") : t("share.copy_link")}
                </button>
                <button
                  onClick={() => void openUrl(ytResult.url).catch(() => {})}
                  className={btn}
                >
                  <ExternalLink size={13} /> {t("youtube.open")}
                </button>
              </div>
              <p className="text-[11px] text-ink-faint">{t("youtube.audit_note")}</p>
            </div>
          ) : social ? (
            <div className="space-y-2">
              <input
                value={ytTitle}
                onChange={(e) => setYtTitle(e.target.value)}
                maxLength={100}
                placeholder={t("youtube.title_ph")}
                className="w-full rounded-control border border-line bg-raised/60 px-2.5 py-1.5 text-xs text-ink outline-none focus:border-warn/60"
              />
              <div className="flex flex-wrap items-center gap-2">
                <select
                  value={ytPrivacy}
                  onChange={(e) => setYtPrivacy(e.target.value)}
                  className="rounded-control border border-line bg-raised/60 px-2.5 py-1.5 text-xs text-ink outline-none focus:border-warn/60"
                >
                  <option value="private">{t("youtube.private")}</option>
                  <option value="unlisted">{t("youtube.unlisted")}</option>
                  <option value="public">{t("youtube.public")}</option>
                </select>
                <span className="font-mono text-[10px] text-ink-faint">
                  #MoonClip #moonclip
                </span>
              </div>
              <label className="flex cursor-pointer items-start gap-2 text-[11px] text-ink-muted">
                <input
                  type="checkbox"
                  checked={ytConfirmed}
                  onChange={(e) => setYtConfirmed(e.target.checked)}
                  className="mt-0.5"
                />
                {t("youtube.confirm")}
              </label>
              <button
                onClick={() => void uploadYouTube()}
                disabled={ytBusy !== null || !ytConfirmed || !ytTitle.trim()}
                className="flex w-full items-center justify-center gap-2 rounded-control border border-brand/40 bg-brand/10 px-3 py-2 text-sm text-red-100 transition hover:bg-brand/20 disabled:opacity-50"
              >
                <SquarePlay size={15} />
                {ytBusy === "upload" ? t("share.uploading") : t("youtube.upload")}
              </button>
              {ytBusy === "upload" && (
                <div>
                  <div className="h-1.5 w-full overflow-hidden rounded-full bg-raised">
                    <div
                      className="h-full rounded-full bg-brand-bright transition-all"
                      style={{ width: `${ytPct ?? 5}%` }}
                    />
                  </div>
                  <p className="mt-1 text-right font-mono text-[10px] text-ink-faint">
                    {ytPct !== null ? `${ytPct}%` : "…"}
                  </p>
                </div>
              )}
              <p className="text-[11px] text-ink-faint">{t("youtube.audit_note")}</p>
            </div>
          ) : null}
          {ytError && <p className="mt-2 break-all font-mono text-xs text-brand-bright">{ytError}</p>}
        </div>

        {/* Discord: connect your account (pick a channel); over-limit clips
            compress to a 720p copy on the fly. */}
        <div className="mt-4 border-t border-line pt-3">
          <h4 className="mb-2 flex items-center gap-1.5 text-xs font-semibold text-ink">
            <MessageSquare size={14} /> {t("discord.section")}
          </h4>
          {social && !social.discord.connected ? (
            <button
              onClick={() => void connectDiscord()}
              disabled={dcBusy !== null}
              className={btn}
            >
              <MessageSquare size={13} />
              {dcBusy === "connect" ? t("accounts.connecting") : t("discord.connect")}
            </button>
          ) : dcResult ? (
            <div className="space-y-2">
              <p className="flex min-w-0 items-center gap-1.5 text-xs text-ok-bright">
                <CloudUpload size={13} /> {t("discord.sent")}
                {dcResult.channel && (
                  <span className="truncate text-ink-faint" title={dcResult.channel}>
                    · {dcResult.channel}
                  </span>
                )}
              </p>
              {dcResult.url && (
                <div className="flex flex-wrap gap-2">
                  <button onClick={() => void copyDiscord()} className={btn}>
                    <Copy size={13} />
                    {dcCopied ? t("share.copied") : t("share.copy_link")}
                  </button>
                  <button
                    onClick={() => void openUrl(dcResult.url ?? "").catch(() => {})}
                    className={btn}
                  >
                    <ExternalLink size={13} /> {t("discord.open")}
                  </button>
                </div>
              )}
            </div>
          ) : social ? (
            <div className="space-y-2">
              <input
                value={dcTitle}
                onChange={(e) => setDcTitle(e.target.value)}
                maxLength={100}
                placeholder={t("discord.title_ph")}
                className="w-full rounded-control border border-line bg-raised/60 px-2.5 py-1.5 text-xs text-ink outline-none focus:border-indigo-500/50"
              />
              {discordOverLimit && (
                <label className="flex cursor-pointer items-start gap-2 text-[11px] text-ink-muted">
                  <input
                    type="checkbox"
                    checked={dcCompress}
                    onChange={(e) => setDcCompress(e.target.checked)}
                    className="mt-0.5"
                  />
                  {t("discord.compress")}
                </label>
              )}
              {discordOverLimit && !dcCompress && (
                <p className="text-[11px] text-warn-bright/80">{t("discord.too_big")}</p>
              )}
              <button
                onClick={() => void shareDiscord()}
                disabled={dcBusy !== null || !dcTitle.trim() || (discordOverLimit && !dcCompress)}
                className="flex w-full items-center justify-center gap-2 rounded-control border border-edit/40 bg-edit/10 px-3 py-2 text-sm text-ink transition hover:bg-edit/20 disabled:opacity-50"
              >
                <MessageSquare size={15} />
                {dcBusy === "upload" ? t("share.uploading") : t("discord.send")}
              </button>
              {dcBusy === "upload" && (
                <div>
                  <div className="h-1.5 w-full overflow-hidden rounded-full bg-raised">
                    <div
                      className="h-full rounded-full bg-indigo-400 transition-all"
                      style={{ width: `${dcPct ?? 5}%` }}
                    />
                  </div>
                  <p className="mt-1 text-right font-mono text-[10px] text-ink-faint">
                    {dcProgress?.phase === "compress" ? `${t("discord.compressing")} · ` : ""}
                    {dcPct !== null ? `${dcPct}%` : "…"}
                  </p>
                </div>
              )}
              {discordOverLimit && dcCompress && (
                <p className="text-[11px] text-ink-faint">{t("discord.compress_note")}</p>
              )}
            </div>
          ) : null}
          {dcError && <p className="mt-2 break-all font-mono text-xs text-brand-bright">{dcError}</p>}
        </div>

        {/* TikTok: Direct Post; privacy options come from creator_info and
            there is deliberately no default value (TikTok UX rule). */}
        <div className="mt-4 border-t border-line pt-3">
          <h4 className="mb-2 flex items-center gap-1.5 text-xs font-semibold text-ink">
            <Music2 size={14} /> {t("tiktok.section")}
          </h4>
          {social && !social.tiktok.connected ? (
            <button
              onClick={() => void connectTikTok()}
              disabled={ttBusy !== null}
              className={btn}
            >
              <Music2 size={13} />
              {ttBusy === "connect" ? t("accounts.connecting") : t("tiktok.connect")}
            </button>
          ) : ttResult ? (
            <div className="space-y-1">
              <p className="flex items-center gap-1.5 text-xs text-ok-bright">
                <CloudUpload size={13} /> {t("tiktok.published")}
              </p>
              <p className="text-[11px] text-ink-faint">{t("tiktok.note")}</p>
            </div>
          ) : social ? (
            <div className="space-y-2">
              {ttCreator && (
                <p className="text-[11px] text-ink-muted">
                  {t("tiktok.creator")}{" "}
                  <span className="text-ink">
                    {ttCreator.nickname || `@${ttCreator.username}`}
                  </span>
                </p>
              )}
              <input
                value={ttTitle}
                onChange={(e) => setTtTitle(e.target.value)}
                maxLength={2200}
                placeholder={t("tiktok.title_ph")}
                className="w-full rounded-control border border-line bg-raised/60 px-2.5 py-1.5 text-xs text-ink outline-none focus:border-slate-400/50"
              />
              <select
                value={ttPrivacy}
                onChange={(e) => setTtPrivacy(e.target.value)}
                className="w-full rounded-control border border-line bg-raised/60 px-2.5 py-1.5 text-xs text-ink outline-none focus:border-slate-400/50"
              >
                <option value="">{t("tiktok.privacy_ph")}</option>
                {(ttCreator?.privacy_level_options ?? []).map((option) => (
                  <option key={option} value={option}>
                    {t(`tiktok.privacy.${option}`, { defaultValue: option })}
                  </option>
                ))}
              </select>
              <button
                onClick={() => void publishTikTok()}
                disabled={ttBusy !== null || !ttTitle.trim() || !ttPrivacy || !ttCreator}
                className="flex w-full items-center justify-center gap-2 rounded-control border border-slate-300/20 bg-slate-100/10 px-3 py-2 text-sm text-ink transition hover:bg-slate-100/20 disabled:opacity-50"
              >
                <Music2 size={15} />
                {ttBusy === "publish" ? t("share.uploading") : t("tiktok.publish")}
              </button>
              {ttBusy === "publish" && (
                <div>
                  <div className="h-1.5 w-full overflow-hidden rounded-full bg-raised">
                    <div
                      className="h-full rounded-full bg-slate-200 transition-all"
                      style={{ width: `${ttPct ?? 5}%` }}
                    />
                  </div>
                  <p className="mt-1 text-right font-mono text-[10px] text-ink-faint">
                    {ttPct !== null ? `${ttPct}%` : "…"}
                  </p>
                </div>
              )}
              <p className="text-[11px] text-ink-faint">{t("tiktok.note")}</p>
            </div>
          ) : null}
          {ttError && <p className="mt-2 break-all font-mono text-xs text-brand-bright">{ttError}</p>}
        </div>

        {error && <p className="mt-3 break-all font-mono text-xs text-brand-bright">{error}</p>}
      </div>
    </div>
    </Modal>
  );
}
