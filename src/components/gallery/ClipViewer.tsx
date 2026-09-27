import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  ChevronLeft,
  ChevronRight,
  Cloud,
  CloudUpload,
  FolderOpen,
  Loader2,
  Pencil,
  Scissors,
  Star,
  Trash2,
  Wand2,
  X,
} from "lucide-react";
import type { ClipMetadata } from "../../types";

export interface ViewerActions {
  onToggleFavorite: (id: string) => void;
  onDelete: (id: string) => void;
  onTrim: (clip: ClipMetadata) => void;
  onAdvancedEdit: (clip: ClipMetadata) => void;
  onRename: (clip: ClipMetadata, name: string) => void;
  onShare: (clip: ClipMetadata) => void;
  onError: (msg: string) => void;
  onSuccess: () => void;
}

function bareName(fileName: string) {
  const i = fileName.lastIndexOf("/");
  return i >= 0 ? fileName.slice(i + 1) : fileName;
}

function stemOf(fileName: string) {
  const bare = bareName(fileName);
  const dot = bare.lastIndexOf(".");
  return dot > 0 ? bare.slice(0, dot) : bare;
}

function fmtDuration(ms: number) {
  const s = Math.round(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

function fmtSize(bytes: number) {
  if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
  if (bytes >= 1_000_000) return `${(bytes / 1_000_000).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1000))} KB`;
}

/** Medal-style full-screen viewer: player + metadata + every action, with
 *  ←/→ to move through the current list and Esc to close. Cloud clips are
 *  downloaded on demand (progress shown). */
export function ClipViewer({
  clip,
  gameLabel,
  onClose,
  onPrev,
  onNext,
  actions,
}: {
  clip: ClipMetadata;
  gameLabel: string;
  onClose: () => void;
  onPrev?: () => void;
  onNext?: () => void;
  actions: ViewerActions;
}) {
  const { t } = useTranslation();
  const [url, setUrl] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [progress, setProgress] = useState<{ sent: number; total: number } | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setUrl(null);
    setError(null);
    // Loopback URL; cloud clips download on demand first (ensure_local).
    invoke<string>("media_url", { clipId: clip.id })
      .then((u) => {
        if (!cancelled) {
          setUrl(u);
          setLoading(false);
        }
      })
      .catch((e) => {
        if (!cancelled) {
          setError(String(e));
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [clip.id]);

  useEffect(() => {
    const unlisten = listen<{ sent: number; total: number }>(
      "moonclip://download-progress",
      (event) => setProgress(event.payload),
    );
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
      if (e.key === "ArrowLeft") onPrev?.();
      if (e.key === "ArrowRight") onNext?.();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, onPrev, onNext]);

  const reveal = async () => {
    try {
      const abs = await invoke<string>("resolve_clip_src", { fileName: clip.file_name });
      try {
        await revealItemInDir(abs);
      } catch {
        const sep = abs.includes("\\") ? "\\" : "/";
        await openPath(abs.slice(0, abs.lastIndexOf(sep)));
      }
      actions.onSuccess();
    } catch (e) {
      actions.onError(`reveal: ${String(e)}`);
    }
  };

  const startRename = () => {
    setDraft(stemOf(clip.file_name));
    setEditing(true);
  };
  const commitRename = () => {
    setEditing(false);
    const name = draft.trim();
    if (name && name !== stemOf(clip.file_name)) actions.onRename(clip, name);
  };

  const btn =
    "inline-flex items-center gap-1.5 rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-xs text-slate-200 transition hover:border-cyan-500/40 hover:text-cyan-200 disabled:opacity-50";
  const pct =
    progress && progress.total > 0
      ? Math.min(100, Math.round((progress.sent / progress.total) * 100))
      : null;

  return (
    <div className="fixed inset-0 z-40 flex flex-col bg-black/80 backdrop-blur-sm">
      <div className="flex items-center gap-2 px-4 py-3">
        <div className="min-w-0 flex-1">
          {editing ? (
            <input
              autoFocus
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onFocus={(e) => e.currentTarget.select()}
              onKeyDown={(e) => {
                if (e.key === "Enter") commitRename();
                if (e.key === "Escape") {
                  e.stopPropagation();
                  setEditing(false);
                }
              }}
              onBlur={() => setEditing(false)}
              className="w-full max-w-md rounded-md border border-cyan-400/30 bg-black/40 px-2 py-0.5 text-sm font-medium text-slate-100 outline-none focus:border-cyan-400/60"
            />
          ) : (
            <p className="truncate text-sm font-semibold text-slate-100" title={clip.file_name}>
              {bareName(clip.file_name)}
            </p>
          )}
          <p className="truncate font-mono text-[11px] text-slate-500">
            {gameLabel} · {fmtDuration(clip.duration_ms)} · {fmtSize(clip.file_size_bytes)} ·{" "}
            {clip.created_at.slice(0, 16)}
            {clip.cloud && (
              <span className="ml-2 inline-flex items-center gap-0.5 text-cyan-300/80">
                <Cloud size={11} /> {t("gallery.cloud")}
              </span>
            )}
            {!clip.cloud && clip.drive_file_id && (
              <span className="ml-2 inline-flex items-center gap-0.5 text-emerald-300/80">
                <CloudUpload size={11} /> {t("gallery.uploaded")}
              </span>
            )}
          </p>
        </div>
        <button onClick={onPrev} disabled={!onPrev} className={btn} title="←">
          <ChevronLeft size={14} />
        </button>
        <button onClick={onNext} disabled={!onNext} className={btn} title="→">
          <ChevronRight size={14} />
        </button>
        <button
          onClick={onClose}
          className="rounded-lg p-1.5 text-slate-400 transition hover:bg-white/10 hover:text-slate-100"
          title="Esc"
        >
          <X size={17} />
        </button>
      </div>

      <div className="flex min-h-0 flex-1 items-center justify-center px-4">
        {loading ? (
          <div className="flex flex-col items-center gap-2 text-slate-400">
            <Loader2 size={26} className="animate-spin" />
            {clip.cloud && (
              <p className="text-xs text-cyan-300/80">
                {t("gallery.downloading")} {pct !== null ? `${pct}%` : ""}
              </p>
            )}
          </div>
        ) : url ? (
          <video
            key={clip.id}
            src={url}
            controls
            autoPlay
            loop
            className="max-h-full max-w-full rounded-xl bg-black"
          />
        ) : (
          <p className="max-w-md break-all text-center font-mono text-xs text-red-400">{error}</p>
        )}
      </div>

      <div className="flex flex-wrap items-center justify-center gap-2 px-4 py-3">
        <button
          onClick={() => actions.onToggleFavorite(clip.id)}
          className={`${btn} ${clip.is_favorite ? "text-amber-300" : ""}`}
          title={t("gallery.favorite")}
        >
          <Star size={14} fill={clip.is_favorite ? "currentColor" : "none"} />
        </button>
        <button onClick={() => actions.onShare(clip)} className={btn} title={t("share.title")}>
          <CloudUpload size={14} /> {t("share.title")}
        </button>
        <button onClick={() => actions.onTrim(clip)} className={btn} title={t("trim.title")}>
          <Scissors size={14} /> {t("trim.title")}
        </button>
        <button
          onClick={() => actions.onAdvancedEdit(clip)}
          className={`${btn} border-cyan-500/30 bg-cyan-500/10 text-cyan-200`}
          title={t("editor.open")}
        >
          <Wand2 size={14} /> {t("editor.open")}
        </button>
        <button onClick={startRename} className={btn} title={t("gallery.rename")}>
          <Pencil size={14} />
        </button>
        <button onClick={() => void reveal()} className={btn} title={t("gallery.reveal")}>
          <FolderOpen size={14} />
        </button>
        {clip.cloud && confirmDelete ? (
          <button
            onClick={() => actions.onDelete(clip.id)}
            className="inline-flex items-center gap-1.5 rounded-lg bg-red-500/20 px-2.5 py-1.5 text-xs font-medium text-red-200 transition hover:bg-red-500/30"
          >
            <Trash2 size={14} /> {t("gallery.confirm_cloud_delete")}
          </button>
        ) : (
          <button
            onClick={() => {
              if (clip.cloud) {
                setConfirmDelete(true);
                window.setTimeout(() => setConfirmDelete(false), 4000);
                return;
              }
              actions.onDelete(clip.id);
            }}
            className={`${btn} hover:border-red-500/40 hover:text-red-300`}
            title={t("common.delete")}
          >
            <Trash2 size={14} />
          </button>
        )}
        <span className="ml-2 hidden text-[10px] text-slate-600 sm:block">{t("viewer.hint")}</span>
      </div>
    </div>
  );
}
