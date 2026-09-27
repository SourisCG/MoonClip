import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { CloudUpload, Copy, X } from "lucide-react";
import type { ClipMetadata } from "../../types";

interface UploadResult {
  file_id: string;
  name: string;
  web_link: string | null;
}

interface Progress {
  clipId: string;
  sent: number;
  total: number;
}

/** Upload one clip to Google Drive (private by default; optional public
 *  anyone-with-the-link + copy). */
export function ShareDialog({ clip, onClose }: { clip: ClipMetadata; onClose: () => void }) {
  const { t } = useTranslation();
  const [makePublic, setMakePublic] = useState(false);
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [result, setResult] = useState<UploadResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    const unlisten = listen<Progress>("moonclip://upload-progress", (event) => {
      if (event.payload.clipId === clip.id) setProgress(event.payload);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [clip.id]);

  const upload = async () => {
    setBusy(true);
    setError(null);
    setProgress(null);
    try {
      const res = await invoke<UploadResult>("drive_upload_clip", {
        clipId: clip.id,
        makePublic,
      });
      setResult(res);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
      setProgress(null);
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

  const pct =
    progress && progress.total > 0
      ? Math.min(100, Math.round((progress.sent / progress.total) * 100))
      : null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4">
      <div className="w-full max-w-md rounded-2xl border border-white/10 bg-[#0b0f19] p-4 shadow-2xl">
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

        {!result ? (
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
            <button
              onClick={() => void upload()}
              disabled={busy}
              className="flex w-full items-center justify-center gap-2 rounded-lg border border-cyan-400/30 bg-cyan-500/10 px-3 py-2 text-sm text-cyan-100 transition hover:bg-cyan-500/20 disabled:opacity-50"
            >
              <CloudUpload size={15} />
              {busy ? t("share.uploading") : t("share.drive_upload")}
            </button>
            {busy && (
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
          </>
        ) : (
          <div className="space-y-2">
            <p className="text-xs text-emerald-300/90">
              {t("share.upload_done")} · {result.name}
            </p>
            {result.web_link ? (
              <button
                onClick={() => void copyLink()}
                className="flex w-full items-center justify-center gap-2 rounded-lg border border-white/10 bg-white/5 px-3 py-2 text-sm text-slate-200 transition hover:border-cyan-500/40 hover:text-cyan-200"
              >
                <Copy size={14} />
                {copied ? t("share.copied") : t("share.copy_link")}
              </button>
            ) : (
              <p className="text-[11px] text-slate-500">{t("share.private_note")}</p>
            )}
          </div>
        )}

        {error && <p className="mt-3 break-all font-mono text-xs text-red-400">{error}</p>}
      </div>
    </div>
  );
}
