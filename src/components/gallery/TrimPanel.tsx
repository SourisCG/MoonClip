import { useCallback, useEffect, useRef, useState } from "react";
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
  Pause,
  Pencil,
  Play,
  Repeat,
  Scissors,
  Star,
  Trash2,
  Upload,
  Wand2,
  X,
} from "lucide-react";
import { thumbnailUrl } from "../../lib/media";
import type { ClipMetadata } from "../../types";
import { Modal } from "../Modal";

/** Actions the gallery owns (single shared clip list). */
export interface PanelActions {
  onToggleFavorite: (id: string) => void;
  onDelete: (id: string) => void;
  onRename: (clip: ClipMetadata, name: string) => void;
  onShare: (clip: ClipMetadata) => void;
  onError: (msg: string) => void;
  onSuccess: () => void;
}

/** Payload of `moonclip://edit-progress` (Rust `EditProgress`). */
interface EditProgress {
  op: string;
  clipId: string;
  percent: number;
  done: boolean;
}

function fmt(ms: number) {
  const total = Math.max(0, Math.round(ms / 1000));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}.${String(
    Math.max(0, Math.round(ms / 100)) % 10,
  )}`;
}

function fmtShort(ms: number) {
  const s = Math.round(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

function fmtSize(bytes: number) {
  if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
  if (bytes >= 1_000_000) return `${(bytes / 1_000_000).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1000))} KB`;
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

/**
 * Medal-style clip panel: watch the clip and quick-trim it in one place.
 * Light (no editor chunk); lossless copy by default, precise on demand.
 * Cloud clips download on demand and the temporary copy is dropped on close.
 */
export function TrimPanel({
  clip,
  gameLabel,
  onClose,
  onSaved,
  onAdvancedEdit,
  onPrev,
  onNext,
  actions,
}: {
  clip: ClipMetadata;
  gameLabel: string;
  onClose: () => void;
  onSaved: () => void;
  onAdvancedEdit?: (clip: ClipMetadata) => void;
  onPrev?: () => void;
  onNext?: () => void;
  actions: PanelActions;
}) {
  const { t } = useTranslation();
  const videoRef = useRef<HTMLVideoElement>(null);
  const barRef = useRef<HTMLDivElement>(null);
  const playheadRef = useRef<HTMLDivElement>(null);
  const [url, setUrl] = useState<string | null>(null);
  const [poster, setPoster] = useState<string | null>(null);
  const [download, setDownload] = useState<{ sent: number; total: number } | null>(null);
  const [duration, setDuration] = useState(Math.max(100, clip.duration_ms));
  const [start, setStart] = useState(0);
  const [end, setEnd] = useState(Math.max(100, clip.duration_ms));
  const [pos, setPos] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [precise, setPrecise] = useState(false);
  const [loop, setLoop] = useState(false);
  const [saving, setSaving] = useState(false);
  const [percent, setPercent] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [drag, setDrag] = useState<"start" | "end" | "seek" | null>(null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);

  // Closing the panel releases the on-demand copy of a cloud clip (local
  // clips have no cache, so this is a no-op for them).
  const close = useCallback(() => {
    void invoke("cloud_cache_cleanup", { clipId: clip.id }).catch(() => {});
    onClose();
  }, [clip.id, onClose]);

  useEffect(() => {
    let cancelled = false;
    setUrl(null);
    // Loopback HTTP URL: `asset://` cannot play media on WebKitGTK.
    invoke<string>("media_url", { clipId: clip.id })
      .then((u) => {
        if (!cancelled) setUrl(u);
      })
      .catch((e) => !cancelled && setError(String(e)));
    return () => {
      cancelled = true;
    };
  }, [clip.id]);

  // Thumbnail as poster: something is visible while the media pipeline warms
  // up (no black frame while the file is being indexed).
  useEffect(() => {
    let cancelled = false;
    let blob: string | null = null;
    thumbnailUrl(clip.thumbnail_name).then(
      (u) => {
        if (cancelled) {
          URL.revokeObjectURL(u);
        } else {
          blob = u;
          setPoster(u);
        }
      },
      () => {},
    );
    return () => {
      cancelled = true;
      if (blob) URL.revokeObjectURL(blob);
    };
  }, [clip.thumbnail_name]);

  // Cloud downloads show their progress while the URL is being prepared.
  useEffect(() => {
    const unlisten = listen<{ fileId: string; sent: number; total: number }>(
      "moonclip://download-progress",
      (event) => {
        if (event.payload.fileId === clip.id) setDownload(event.payload);
      },
    );
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [clip.id]);

  // Tear the media pipeline down on close: WebKitGTK can keep playing audio
  // from a removed <video> unless the source is cleared explicitly.
  useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    return () => {
      try {
        v.pause();
        v.removeAttribute("src");
        v.load();
      } catch {
        /* element already gone */
      }
    };
  }, [url]);

  // Leaving the app (alt-tab / minimize) must not keep the preview sounding.
  useEffect(() => {
    const pause = () => videoRef.current?.pause();
    const onVisibility = () => {
      if (document.hidden) pause();
    };
    window.addEventListener("blur", pause);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.removeEventListener("blur", pause);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, []);

  // ←/→ change clip, Esc closes (never while typing a rename).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA")) return;
      if (e.key === "Escape") close();
      if (e.key === "ArrowLeft") onPrev?.();
      if (e.key === "ArrowRight") onNext?.();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [close, onPrev, onNext]);

  // Trim progress for THIS clip (the backend emits per handled clip).
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<EditProgress>("moonclip://edit-progress", (e) => {
      if (e.payload.op === "trim" && e.payload.clipId === clip.id) {
        setPercent(e.payload.percent);
      }
    }).then((u) => {
      unlisten = u;
    });
    return () => unlisten?.();
  }, [clip.id]);

  // Real media duration wins over the stored one (legacy rows may be off).
  const onLoaded = () => {
    const v = videoRef.current;
    if (v && Number.isFinite(v.duration) && v.duration > 0) {
      const ms = Math.round(v.duration * 1000);
      setDuration(ms);
      setEnd((e) => Math.min(e, ms));
    }
  };

  const seek = useCallback((ms: number) => {
    const v = videoRef.current;
    if (v) v.currentTime = Math.max(0, ms) / 1000;
    setPos(ms);
  }, []);

  // Smooth playhead: while playing, a rAF loop moves it directly (no React
  // re-render); onTimeUpdate only updates the time label (~10 Hz via pos).
  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    let lastLabel = 0;
    const tick = (now: number) => {
      const v = videoRef.current;
      if (v && playheadRef.current) {
        const ms = v.currentTime * 1000;
        playheadRef.current.style.left = `${(ms / Math.max(1, duration)) * 100}%`;
        if (now - lastLabel > 100) {
          lastLabel = now;
          setPos(ms);
        }
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing, duration]);

  // Parked/seeking positions come from state (the rAF only runs while playing).
  useEffect(() => {
    if (!playheadRef.current || playing) return;
    playheadRef.current.style.left = `${(pos / Math.max(1, duration)) * 100}%`;
  }, [pos, playing, duration]);

  const togglePlay = useCallback(() => {
    const v = videoRef.current;
    if (!v) return;
    if (v.paused) {
      // Never play outside the selection: jump to In first.
      const ms = v.currentTime * 1000;
      if (ms < start || ms >= end - 20) seek(start);
      void v.play();
    } else {
      v.pause();
    }
  }, [end, seek, start]);

  const onTimeUpdate = () => {
    const v = videoRef.current;
    if (!v) return;
    const ms = v.currentTime * 1000;
    if (ms < start - 30) {
      v.currentTime = start / 1000;
      setPos(start);
      return;
    }
    if (ms >= end) {
      if (!v.paused && loop) {
        v.currentTime = start / 1000;
      } else {
        // No loop by default: stop at the edge (Medal-style).
        if (!v.paused) v.pause();
        v.currentTime = end / 1000;
        setPos(end);
      }
      return;
    }
    // While playing the rAF loop drives the playhead and the label; this
    // event only matters when paused/seeking (fewer re-renders).
    if (!v.paused) return;
    setPos(ms);
  };

  const msFromClientX = useCallback(
    (clientX: number) => {
      const bar = barRef.current;
      if (!bar) return 0;
      const r = bar.getBoundingClientRect();
      const x = Math.min(Math.max(clientX - r.left, 0), r.width);
      return (x / r.width) * duration;
    },
    [duration],
  );

  const onBarMove = (e: React.PointerEvent) => {
    if (!drag) return;
    const ms = msFromClientX(e.clientX);
    if (drag === "start") {
      const next = Math.min(ms, end - 200);
      setStart(next);
      seek(next);
    } else if (drag === "end") {
      const next = Math.max(ms, start + 200);
      setEnd(next);
    } else {
      seek(Math.min(Math.max(ms, start), end));
    }
  };

  const onBarDown = (e: React.PointerEvent) => {
    barRef.current?.setPointerCapture(e.pointerId);
    setDrag("seek");
    seek(Math.min(Math.max(msFromClientX(e.clientX), start), end));
  };

  const save = async () => {
    videoRef.current?.pause();
    setSaving(true);
    setError(null);
    setPercent(0);
    try {
      await invoke("trim_clip", {
        clipId: clip.id,
        startMs: Math.round(start),
        endMs: Math.round(end),
        precise,
      });
      onSaved();
      close();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

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

  const handle =
    "absolute top-0 h-full w-3 -translate-x-1/2 cursor-ew-resize rounded-full border border-cyan-300/60 bg-cyan-400/70 shadow-[0_0_10px_rgba(34,211,238,0.5)]";
  const pct = (ms: number) => `${(ms / Math.max(1, duration)) * 100}%`;
  const actionBtn =
    "inline-flex items-center gap-1.5 rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-xs text-slate-200 transition hover:border-cyan-500/40 hover:text-cyan-200 disabled:opacity-50";
  const chip = "inline-flex items-center gap-1 rounded bg-white/5 px-1.5 py-0.5";

  const downloadPct =
    download && download.total > 0
      ? Math.min(100, Math.round((download.sent / download.total) * 100))
      : null;

  return (
    <Modal>
      <div className="fixed inset-0 z-40 flex items-center justify-center bg-black/75 p-4">
        <div className="max-h-[calc(100vh-2rem)] w-full max-w-5xl overflow-y-auto rounded-2xl border border-white/10 bg-gradient-to-b from-[#0d1220] to-[#0b0f19] p-4 shadow-2xl">
        {/* Header: title (rename inline), metadata chips, navigation */}
        <div className="mb-3 flex items-start gap-2">
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
                className="w-full max-w-md rounded-md border border-cyan-400/30 bg-black/40 px-2 py-0.5 text-sm font-semibold text-slate-100 outline-none focus:border-cyan-400/60"
              />
            ) : (
              <p
                className="truncate text-sm font-semibold text-slate-100"
                title={clip.file_name}
              >
                {bareName(clip.file_name)}
              </p>
            )}
            <div className="mt-1 flex flex-wrap items-center gap-1.5 font-mono text-[11px] text-slate-500">
              <span className={`${chip} text-slate-300`}>{gameLabel}</span>
              <span className={chip}>{fmtShort(clip.duration_ms)}</span>
              <span className={chip}>{fmtSize(clip.file_size_bytes)}</span>
              <span className={chip}>{clip.created_at.slice(0, 16)}</span>
              {clip.is_favorite && (
                <span className={`${chip} text-amber-300`}>
                  <Star size={10} fill="currentColor" />
                </span>
              )}
              {clip.cloud && (
                <span className={`${chip} text-cyan-300`}>
                  <Cloud size={10} /> {t("gallery.cloud")}
                </span>
              )}
              {!clip.cloud && clip.drive_file_id && (
                <span className={`${chip} text-emerald-300`}>
                  <Upload size={10} /> {t("gallery.uploaded")}
                </span>
              )}
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-1">
            <button onClick={onPrev} disabled={!onPrev} className={actionBtn} title="←">
              <ChevronLeft size={14} />
            </button>
            <button onClick={onNext} disabled={!onNext} className={actionBtn} title="→">
              <ChevronRight size={14} />
            </button>
            <button
              onClick={close}
              disabled={saving}
              className="rounded-lg p-1.5 text-slate-500 transition hover:bg-white/10 hover:text-slate-200 disabled:opacity-50"
            >
              <X size={16} />
            </button>
          </div>
        </div>

        {/* Player: the frame reserves its height from the first paint, so the
            video settles instantly (no resize jump when metadata arrives). */}
        <div className="relative h-[52vh] w-full overflow-hidden rounded-xl border border-white/5 bg-black/70">
          {url ? (
            <video
              ref={videoRef}
              src={url}
              poster={poster ?? undefined}
              preload="auto"
              onLoadedMetadata={onLoaded}
              onTimeUpdate={onTimeUpdate}
              onPlay={() => setPlaying(true)}
              onPause={() => setPlaying(false)}
              onClick={togglePlay}
              onError={() => {
                const code = videoRef.current?.error?.code;
                setError(`${t("trim.play_error")} (${code ?? "?"})`);
              }}
              className="absolute inset-0 h-full w-full cursor-pointer object-contain"
            />
          ) : (
            <div className="flex h-full flex-col items-center justify-center gap-2 text-xs text-slate-500">
              {clip.cloud ? (
                <>
                  <Loader2 size={18} className="animate-spin text-cyan-300" />
                  <span className="text-cyan-300/80">
                    {t("gallery.downloading")} {downloadPct !== null ? `${downloadPct}%` : ""}
                  </span>
                </>
              ) : (
                <Loader2 size={18} className="animate-spin" />
              )}
            </div>
          )}
        </div>

        {/* Transport + selection bar */}
        <div className="mt-3 flex items-center gap-3">
          <button
            onClick={togglePlay}
            className="rounded-lg border border-white/10 bg-white/5 p-2 text-slate-200 transition hover:bg-white/10"
            title={playing ? t("trim.pause") : t("trim.play")}
          >
            {playing ? <Pause size={14} /> : <Play size={14} />}
          </button>
          <span className="w-24 shrink-0 font-mono text-xs text-slate-400">
            {fmt(pos)} / {fmt(duration)}
          </span>
          <div
            ref={barRef}
            onPointerDown={onBarDown}
            onPointerMove={onBarMove}
            onPointerUp={() => setDrag(null)}
            onPointerCancel={() => setDrag(null)}
            className="relative h-8 flex-1 cursor-pointer touch-none select-none rounded-lg border border-white/10 bg-white/5"
          >
            <div
              className="absolute inset-y-0 bg-cyan-400/20"
              style={{ left: pct(start), width: pct(end - start) }}
            />
            {/* The rAF loop owns this position while playing (no React
                re-render can snap it back); the effect parks it otherwise. */}
            <div
              ref={playheadRef}
              className="absolute inset-y-0 w-0.5 bg-cyan-300 will-change-[left]"
            />
            <div
              role="slider"
              aria-valuenow={Math.round(start)}
              className={handle}
              style={{ left: pct(start) }}
              onPointerDown={(e) => {
                e.stopPropagation();
                setDrag("start");
              }}
            />
            <div
              role="slider"
              aria-valuenow={Math.round(end)}
              className={handle}
              style={{ left: pct(end) }}
              onPointerDown={(e) => {
                e.stopPropagation();
                setDrag("end");
              }}
            />
          </div>
        </div>

        {/* Clip actions */}
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <button onClick={() => actions.onShare(clip)} className={actionBtn} title={t("share.title")}>
            <CloudUpload size={13} /> {t("share.title")}
          </button>
          {onAdvancedEdit && (
            <button
              onClick={() => {
                close();
                onAdvancedEdit(clip);
              }}
              disabled={saving}
              className={`${actionBtn} border-cyan-500/30 bg-cyan-500/10 text-cyan-200`}
              title={t("editor.open")}
            >
              <Wand2 size={13} /> {t("editor.open")}
            </button>
          )}
          <button
            onClick={() => actions.onToggleFavorite(clip.id)}
            className={`${actionBtn} ${clip.is_favorite ? "text-amber-300" : ""}`}
            title={t("gallery.favorite")}
          >
            <Star size={13} fill={clip.is_favorite ? "currentColor" : "none"} />
          </button>
          <button onClick={startRename} className={actionBtn} title={t("gallery.rename")}>
            <Pencil size={13} />
          </button>
          <button onClick={() => void reveal()} className={actionBtn} title={t("gallery.reveal")}>
            <FolderOpen size={13} />
          </button>
          {clip.cloud && confirmDelete ? (
            <button
              onClick={() => actions.onDelete(clip.id)}
              className="ml-auto inline-flex items-center gap-1.5 rounded-lg bg-red-500/20 px-2.5 py-1.5 text-xs font-medium text-red-200 transition hover:bg-red-500/30"
            >
              <Trash2 size={13} /> {t("gallery.confirm_cloud_delete")}
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
              className={`${actionBtn} ml-auto hover:border-red-500/40 hover:text-red-300`}
              title={t("common.delete")}
            >
              <Trash2 size={13} />
            </button>
          )}
        </div>

        {/* Trim options */}
        <div className="mt-3 flex flex-wrap items-center gap-3 border-t border-white/5 pt-3">
          <label className="flex items-center gap-2 text-xs text-slate-300">
            <input
              type="checkbox"
              checked={precise}
              onChange={(e) => setPrecise(e.target.checked)}
              disabled={saving}
              className="accent-cyan-400"
            />
            {t("trim.precise")}
          </label>
          <label className="flex items-center gap-2 text-xs text-slate-300">
            <input
              type="checkbox"
              checked={loop}
              onChange={(e) => setLoop(e.target.checked)}
              disabled={saving}
              className="accent-cyan-400"
            />
            <Repeat size={12} /> {t("trim.loop")}
          </label>
          <span className="text-[11px] text-slate-500">{t("trim.hint")}</span>
          <span className="ml-auto font-mono text-xs text-slate-400">
            {fmt(start)} – {fmt(end)} ({((end - start) / 1000).toFixed(1)}s)
          </span>
        </div>

        {saving && (
          <div className="mt-3">
            <div className="h-1.5 overflow-hidden rounded-full bg-white/10">
              <div
                className="h-full bg-cyan-400 transition-[width] duration-200"
                style={{ width: `${Math.min(100, percent)}%` }}
              />
            </div>
            <p className="mt-1 text-[11px] text-slate-400">{t("trim.saving")}</p>
          </div>
        )}
        {error && <p className="mt-2 break-words font-mono text-xs text-red-400">{error}</p>}

        <div className="mt-4 flex items-center justify-end gap-2">
          <span className="mr-auto hidden text-[10px] text-slate-600 sm:block">
            {t("viewer.hint")}
          </span>
          <button
            onClick={close}
            disabled={saving}
            className="rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-sm text-slate-200 transition hover:bg-white/10 disabled:opacity-50"
          >
            {t("trim.cancel")}
          </button>
          <button
            onClick={() => void save()}
            disabled={saving || end - start < 100}
            className="inline-flex items-center gap-1.5 rounded-lg border border-cyan-400/40 bg-cyan-500/20 px-3 py-1.5 text-sm font-medium text-cyan-100 transition hover:bg-cyan-500/30 disabled:cursor-wait disabled:opacity-50"
          >
            {saving ? <Loader2 size={14} className="animate-spin" /> : <Scissors size={14} />}
            {saving ? t("trim.saving_short") : t("trim.save")}
          </button>
        </div>
        </div>
      </div>
    </Modal>
  );
}
