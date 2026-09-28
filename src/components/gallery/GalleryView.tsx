import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { thumbnailUrl } from "../../lib/media";
import { EmptyState } from "../ui/EmptyState";
import { Clapperboard, Cloud, CloudUpload, Search, Star, Upload } from "lucide-react";
import { ShareDialog } from "./ShareDialog";
import { TrimPanel, type PanelActions } from "./TrimPanel";
import { useClips } from "../../hooks/useClips";
import { useRegisteredInputs } from "../../hooks/useRegisteredInputs";
import type { ClipMetadata } from "../../types";

/** "Game folder/Replay ….mp4" -> "Replay ….mp4" */
function bareName(fileName: string): string {
  const i = fileName.lastIndexOf("/");
  return i >= 0 ? fileName.slice(i + 1) : fileName;
}

/** Clips whose row has no folder (missing legacy files). */
const FLAT = "__flat__";

function fmtDuration(ms: number) {
  const s = Math.round(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

function fmtSize(bytes: number) {
  if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
  if (bytes >= 1_000_000) return `${(bytes / 1_000_000).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1000))} KB`;
}

function timeNow() {
  return new Date().toLocaleTimeString();
}

/** Thumbnail bytes over IPC with a revocable blob URL. */
function useThumbnail(clip: ClipMetadata, onError: (msg: string) => void) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    let url: string | null = null;
    setSrc(null);
    thumbnailUrl(clip.thumbnail_name).then(
      (u) => {
        if (cancelled) {
          URL.revokeObjectURL(u);
        } else {
          url = u;
          setSrc(u);
        }
      },
      (e) => {
        if (!cancelled) onError(`thumb: ${String(e)}`);
      },
    );
    return () => {
      cancelled = true;
      if (url) URL.revokeObjectURL(url);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [clip.thumbnail_name]);
  return src;
}

interface CardActions {
  onToggleFavorite: (id: string) => void;
  onShare: (clip: ClipMetadata) => void;
  onError: (msg: string) => void;
}

/**
 * Medal-style hover preview: after a short dwell, a muted looping video
 * replaces the thumbnail (local clips only — cloud clips would trigger a
 * full download). The video unmounts on leave, stopping playback.
 */
function useHoverPreview(clip: ClipMetadata) {
  const eligible = !clip.cloud && clip.exists;
  const [url, setUrl] = useState<string | null>(null);
  const timer = useRef<number | null>(null);
  const cancelled = useRef(false);

  const clearTimer = () => {
    if (timer.current !== null) {
      window.clearTimeout(timer.current);
      timer.current = null;
    }
  };

  useEffect(
    () => () => {
      cancelled.current = true;
      clearTimer();
    },
    [],
  );

  const onEnter = () => {
    if (!eligible || url) return;
    clearTimer();
    timer.current = window.setTimeout(async () => {
      try {
        const media = await invoke<string>("media_url", { clipId: clip.id });
        if (!cancelled.current) setUrl(media);
      } catch {
        // Media server unavailable: the thumbnail stays.
      }
    }, 260);
  };

  const onLeave = () => {
    clearTimer();
    setUrl(null);
  };

  return { url, onEnter, onLeave };
}

/** Medal-style 16:9 card: thumbnail + hover preview, duration, quick actions. */
function ClipCard({
  clip,
  gameLabel,
  onOpen,
  actions,
}: {
  clip: ClipMetadata;
  gameLabel: string;
  onOpen: () => void;
  actions: CardActions;
}) {
  const { t } = useTranslation();
  const src = useThumbnail(clip, actions.onError);
  const preview = useHoverPreview(clip);
  const iconBtn =
    "rounded-control border border-line bg-black/80 p-1.5 text-ink-soft transition-colors hover:border-line-strong hover:bg-black/90 hover:text-ink";

  return (
    <div
      className="group overflow-hidden rounded-card border border-line bg-surface transition-colors duration-150 hover:border-line-strong"
      onMouseEnter={preview.onEnter}
      onMouseLeave={preview.onLeave}
    >
      <div className="relative">
        <button onClick={onOpen} className="block w-full" title={clip.file_name}>
          <div className="relative flex aspect-video w-full items-center justify-center overflow-hidden bg-base">
            {src ? (
              <img
                src={src}
                alt=""
                onError={() => actions.onError(`thumb asset blocked: ${clip.thumbnail_name}`)}
                className={
                  "h-full w-full object-cover transition duration-200 " +
                  (preview.url ? "opacity-0" : "opacity-100 group-hover:brightness-110")
                }
              />
            ) : (
              <Clapperboard size={22} className="text-ink-faint" />
            )}
            {preview.url && (
              <video
                src={preview.url}
                muted
                loop
                autoPlay
                playsInline
                preload="none"
                className="absolute inset-0 h-full w-full object-cover"
              />
            )}
          </div>
        </button>
        <span className="pointer-events-none absolute bottom-1.5 right-1.5 rounded-[5px] bg-black/80 px-1.5 py-0.5 font-mono text-[10px] text-ink">
          {fmtDuration(clip.duration_ms)}
        </span>
        {clip.cloud && (
          <span className="pointer-events-none absolute left-1.5 top-1.5 inline-flex items-center gap-1 rounded-[5px] border border-link/50 bg-black/80 px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wider text-link-bright">
            <Cloud size={10} /> {t("gallery.cloud")}
          </span>
        )}
        <div className="absolute right-1.5 top-1.5 flex gap-1 opacity-0 transition-opacity group-hover:opacity-100">
          <button
            onClick={() => actions.onToggleFavorite(clip.id)}
            className={`${iconBtn} ${clip.is_favorite ? "border-warn/60 text-warn-bright" : ""}`}
            title={t("gallery.favorite")}
          >
            <Star size={13} fill={clip.is_favorite ? "currentColor" : "none"} />
          </button>
          <button
            onClick={() => actions.onShare(clip)}
            className={iconBtn}
            title={t("share.title")}
          >
            <CloudUpload size={13} />
          </button>
        </div>
      </div>
      <div className="min-w-0 px-2.5 py-2">
        <p className="truncate text-[13px] font-medium text-ink" title={clip.file_name}>
          {bareName(clip.file_name)}
        </p>
        <p className="truncate font-mono text-[11px] text-ink-faint">
          {gameLabel} · {fmtSize(clip.file_size_bytes)}{" "}
          {!clip.cloud && clip.drive_file_id && (
            <span className="inline-flex items-center gap-0.5 text-ok-bright">
              <Upload size={10} /> {t("gallery.uploaded")}
            </span>
          )}
          {!clip.cloud && !clip.drive_file_id && !clip.exists && (
            <span className="text-warn-bright">({t("gallery.missing")})</span>
          )}
        </p>
      </div>
    </div>
  );
}

export function GalleryView({
  refreshToken,
  onAdvancedEdit,
  group,
  query,
  sort,
}: {
  refreshToken: number;
  onAdvancedEdit?: (clip: ClipMetadata) => void;
  group: string;
  query: string;
  sort: string;
}) {
  const { t } = useTranslation();
  // Single shared instance: cards act on THIS list (a per-card instance would
  // refresh a phantom copy and the UI would look dead).
  const { clips, loading, refresh, toggleFavorite, deleteClip, renameClip } = useClips();
  // Registered games only provide nicer labels; the groups themselves come
  // from the clips, so a deleted registration keeps its group forever.
  const { inputs } = useRegisteredInputs();
  const [lastError, setLastError] = useState<string | null>(null);
  const [trimClip, setTrimClip] = useState<ClipMetadata | null>(null);
  const [shareClip, setShareClip] = useState<ClipMetadata | null>(null);
  const [download, setDownload] = useState<{ sent: number; total: number } | null>(null);

  useEffect(() => {
    if (refreshToken > 0) void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshToken]);

  // On-demand downloads (cloud clips) show a small progress note.
  useEffect(() => {
    let timer: number | undefined;
    const unlisten = listen<{ sent: number; total: number }>(
      "moonclip://download-progress",
      (event) => {
        setDownload(event.payload);
        window.clearTimeout(timer);
        timer = window.setTimeout(() => setDownload(null), 2500);
      },
    );
    return () => {
      window.clearTimeout(timer);
      void unlisten.then((fn) => fn());
    };
  }, []);

  const labelFor = (key: string) => {
    if (key === FLAT) return t("gallery.unfiled");
    if (key === "Unknown") return t("gallery.no_game");
    return inputs.find((i) => i.clips_folder === key)?.display_name ?? key;
  };

  const visible = useMemo(() => {
    let list: ClipMetadata[];
    if (group === "all") list = clips;
    else if (group === "favorites") list = clips.filter((c) => c.is_favorite);
    else if (group === "uploaded") list = clips.filter((c) => !!c.drive_file_id);
    else if (group === "cloud") list = clips.filter((c) => c.cloud);
    else list = clips.filter((c) => (c.folder || FLAT) === group);
    const q = query.trim().toLowerCase();
    if (q) {
      list = list.filter(
        (c) =>
          bareName(c.file_name).toLowerCase().includes(q) ||
          c.game_title.toLowerCase().includes(q),
      );
    }
    const sorted = [...list];
    if (sort === "name") {
      sorted.sort((a, b) => bareName(a.file_name).localeCompare(bareName(b.file_name)));
    } else if (sort === "size") {
      sorted.sort((a, b) => b.file_size_bytes - a.file_size_bytes);
    }
    // "recent" keeps the backend order (created_at DESC).
    return sorted;
  }, [clips, group, query, sort]);

  const fail = (msg: string) => setLastError(`${timeNow()} · ${msg}`);
  const actions: PanelActions = {
    onToggleFavorite: (id) =>
      toggleFavorite(id).then(
        () => setLastError(null),
        (e) => fail(String(e)),
      ),
    onDelete: (id) =>
      deleteClip(id).then(
        () => {
          setLastError(null);
          setTrimClip((current) => (current?.id === id ? null : current));
        },
        (e) => fail(String(e)),
      ),
    onRename: (clip, name) =>
      renameClip(clip.id, name).then(
        () => setLastError(null),
        (e) => fail(String(e)),
      ),
    onShare: (clip) => setShareClip(clip),
    onError: fail,
    onSuccess: () => setLastError(null),
  };
  const cardActions: CardActions = {
    onToggleFavorite: actions.onToggleFavorite,
    onShare: actions.onShare,
    onError: actions.onError,
  };
  // ←/→ in the panel walk the CURRENT filtered list.
  const trimIndex = trimClip ? visible.findIndex((c) => c.id === trimClip.id) : -1;
  const navTrim = (dir: number) => {
    if (trimIndex < 0) return;
    const next = visible[trimIndex + dir];
    if (next) setTrimClip(next);
  };

  if (loading && clips.length === 0)
    return (
      <p className="font-mono text-xs uppercase tracking-[0.2em] text-ink-muted">
        {t("common.loading")}
      </p>
    );

  if (clips.length === 0) {
    return (
      <EmptyState
        icon={<Clapperboard size={24} />}
        title={t("gallery.empty")}
        hint={t("gallery.coming")}
        className="mx-auto mt-6 max-w-lg"
      />
    );
  }

  return (
    <>
      {shareClip && (
        <ShareDialog
          clip={shareClip}
          onClose={() => setShareClip(null)}
          onUploaded={() => {
            void refresh();
          }}
        />
      )}
      {trimClip && (
        <TrimPanel
          clip={trimClip}
          gameLabel={labelFor(trimClip.folder || FLAT)}
          onAdvancedEdit={onAdvancedEdit}
          onClose={() => setTrimClip(null)}
          onSaved={() => {
            setLastError(null);
            void refresh();
          }}
          onPrev={trimIndex > 0 ? () => navTrim(-1) : undefined}
          onNext={trimIndex >= 0 && trimIndex < visible.length - 1 ? () => navTrim(1) : undefined}
          actions={actions}
        />
      )}
      {lastError && (
        <p className="mb-2 truncate font-mono text-xs text-brand-bright" title={lastError}>
          {lastError}
        </p>
      )}
      {download && (
        <p className="mb-2 font-mono text-xs text-link-bright">
          {t("gallery.downloading")}{" "}
          {download.total > 0
            ? `${Math.min(100, Math.round((download.sent / download.total) * 100))}%`
            : ""}
        </p>
      )}
      {visible.length === 0 ? (
        <EmptyState
          icon={<Search size={20} />}
          title={t("gallery.empty_group")}
          hint={t("gallery.search")}
          className="mx-auto max-w-md"
        />
      ) : (
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4">
          {visible.map((c) => (
            <ClipCard
              key={c.id}
              clip={c}
              gameLabel={labelFor(c.folder || FLAT)}
              onOpen={() => setTrimClip(c)}
              actions={cardActions}
            />
          ))}
        </div>
      )}
    </>
  );
}
