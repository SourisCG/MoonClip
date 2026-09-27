import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { CloudUpload, Copy, ExternalLink, HardDriveDownload, RefreshCw, Trash2, X, SquarePlay } from "lucide-react";
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

interface Progress {
  clipId: string;
  provider?: string;
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

  const uploaded = !!clip.drive_file_id;
  const cloud = clip.cloud;

  useEffect(() => {
    invoke<Record<string, string>>("get_settings")
      .then((s) => setDeleteLocal(s.share_delete_local === "1"))
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

  const pct =
    progress && progress.total > 0
      ? Math.min(100, Math.round((progress.sent / progress.total) * 100))
      : null;
  const ytPct =
    ytProgress && ytProgress.total > 0
      ? Math.min(100, Math.round((ytProgress.sent / ytProgress.total) * 100))
      : null;
  const btn =
    "inline-flex items-center justify-center gap-1.5 rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-slate-200 transition hover:border-cyan-500/40 hover:text-cyan-200 disabled:opacity-50";

  return (
    <Modal>
      <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4">
        <div className="max-h-[calc(100vh-2rem)] w-full max-w-md overflow-y-auto rounded-2xl border border-white/10 bg-[#0b0f19] p-4 shadow-2xl">
        <div className="mb-3 flex items-center">
          <h3 className="text-sm font-semibold text-slate-100">{t("share.title")}</h3>
          <button
            onClick={onClose}
            className="ml-auto rounded-lg p-1.5 text-slate-500 transition hover:bg-white/10 hover:text-slate-200"
          >
            <X size={15} />
          </button>
        </div>

        <p className="mb-3 truncate font-mono text-xs text-slate-500" title={clip.file_name}>
          {clip.file_name}
        </p>

        {uploaded ? (
          <div className="space-y-2">
            <p className="flex items-center gap-1.5 text-xs text-emerald-300/90">
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
                    className="inline-flex items-center gap-1.5 rounded-lg bg-red-500/20 px-3 py-1.5 text-xs font-medium text-red-200 transition hover:bg-red-500/30 disabled:opacity-50"
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
              <p className="text-[11px] text-slate-500">{t("share.private_note")}</p>
            )}
          </div>
        ) : (
          <>
            <label className="mb-3 flex cursor-pointer items-start gap-2 rounded-lg border border-white/5 bg-black/30 px-3 py-2 text-xs text-slate-300">
              <input
                type="checkbox"
                checked={makePublic}
                onChange={(e) => setMakePublic(e.target.checked)}
                className="mt-0.5"
              />
              <span>
                {t("share.public")}
                <span className="mt-0.5 block text-slate-500">
                  {makePublic ? t("share.public_note") : t("share.private_note")}
                </span>
              </span>
            </label>
            <label className="mb-3 flex cursor-pointer items-start gap-2 rounded-lg border border-white/5 bg-black/30 px-3 py-2 text-xs text-slate-300">
              <input
                type="checkbox"
                checked={deleteLocal}
                onChange={(e) => toggleDeleteLocal(e.target.checked)}
                className="mt-0.5"
              />
              <span>
                {t("share.delete_local")}
                <span className="mt-0.5 block text-slate-500">{t("share.delete_local_note")}</span>
              </span>
            </label>
            <button
              onClick={() => void run("upload", { makePublic, deleteLocal, replace: false })}
              disabled={busy !== null}
              className="flex w-full items-center justify-center gap-2 rounded-lg border border-cyan-400/30 bg-cyan-500/10 px-3 py-2 text-sm text-cyan-100 transition hover:bg-cyan-500/20 disabled:opacity-50"
            >
              <CloudUpload size={15} />
              {busy === "upload" ? t("share.uploading") : t("share.drive_upload")}
            </button>
          </>
        )}

        {busy === "upload" && (
          <div className="mt-3">
            <div className="h-1.5 w-full overflow-hidden rounded-full bg-white/10">
              <div
                className="h-full rounded-full bg-cyan-400 transition-all"
                style={{ width: `${pct ?? 5}%` }}
              />
            </div>
            <p className="mt-1 text-right font-mono text-[10px] text-slate-500">
              {pct !== null ? `${pct}%` : "…"}
            </p>
          </div>
        )}

        {/* YouTube: upload-only, title + fixed #MoonClip #moonclip tags. */}
        <div className="mt-4 border-t border-white/10 pt-3">
          <h4 className="mb-2 flex items-center gap-1.5 text-xs font-semibold text-slate-200">
            <SquarePlay size={14} /> {t("youtube.section")}
          </h4>
          {social && !social.google_youtube.configured ? (
            <p className="text-[11px] text-slate-500">{t("accounts.not_configured")}</p>
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
              <p className="flex items-center gap-1.5 text-xs text-emerald-300/90">
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
              <p className="text-[11px] text-slate-500">{t("youtube.audit_note")}</p>
            </div>
          ) : social ? (
            <div className="space-y-2">
              <input
                value={ytTitle}
                onChange={(e) => setYtTitle(e.target.value)}
                maxLength={100}
                placeholder={t("youtube.title_ph")}
                className="w-full rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-xs text-slate-100 outline-none focus:border-cyan-500/50"
              />
              <div className="flex flex-wrap items-center gap-2">
                <select
                  value={ytPrivacy}
                  onChange={(e) => setYtPrivacy(e.target.value)}
                  className="rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-xs text-slate-100 outline-none focus:border-cyan-500/50"
                >
                  <option value="private">{t("youtube.private")}</option>
                  <option value="unlisted">{t("youtube.unlisted")}</option>
                  <option value="public">{t("youtube.public")}</option>
                </select>
                <span className="font-mono text-[10px] text-slate-500">
                  #MoonClip #moonclip
                </span>
              </div>
              <label className="flex cursor-pointer items-start gap-2 text-[11px] text-slate-400">
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
                className="flex w-full items-center justify-center gap-2 rounded-lg border border-red-400/30 bg-red-500/10 px-3 py-2 text-sm text-red-100 transition hover:bg-red-500/20 disabled:opacity-50"
              >
                <SquarePlay size={15} />
                {ytBusy === "upload" ? t("share.uploading") : t("youtube.upload")}
              </button>
              {ytBusy === "upload" && (
                <div>
                  <div className="h-1.5 w-full overflow-hidden rounded-full bg-white/10">
                    <div
                      className="h-full rounded-full bg-red-400 transition-all"
                      style={{ width: `${ytPct ?? 5}%` }}
                    />
                  </div>
                  <p className="mt-1 text-right font-mono text-[10px] text-slate-500">
                    {ytPct !== null ? `${ytPct}%` : "…"}
                  </p>
                </div>
              )}
              <p className="text-[11px] text-slate-500">{t("youtube.audit_note")}</p>
            </div>
          ) : null}
          {ytError && <p className="mt-2 break-all font-mono text-xs text-red-400">{ytError}</p>}
        </div>

        {error && <p className="mt-3 break-all font-mono text-xs text-red-400">{error}</p>}
      </div>
    </div>
    </Modal>
  );
}
