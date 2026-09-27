import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { thumbnailUrl } from "../../lib/media";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  Clapperboard,
  Cloud,
  CloudUpload,
  FolderOpen,
  Gamepad2,
  HardDriveDownload,
  Pencil,
  Scissors,
  Star,
  Trash2,
  Wand2,
} from "lucide-react";
import { DriveBrowser } from "./DriveBrowser";
import { ShareDialog } from "./ShareDialog";
import { TrimPanel } from "./TrimPanel";
import { useClips } from "../../hooks/useClips";
import { useRegisteredInputs } from "../../hooks/useRegisteredInputs";
import type { ClipMetadata } from "../../types";

// NOTE (Tauri v2 convention, do NOT "fix"): #[tauri::command] auto-converts
// Rust snake_case params to camelCase wire keys. Rust `file_name` arrives as
// `fileName` — the frontend must send camelCase, never mirror the Rust name.
async function absOf(fileName: string): Promise<string> {
  return invoke<string>("resolve_clip_src", { fileName });
}

/** "Game folder/Replay ….mp4" -> "Replay ….mp4" */
function bareName(fileName: string): string {
  const i = fileName.lastIndexOf("/");
  return i >= 0 ? fileName.slice(i + 1) : fileName;
}

/** File stem (name without extension) — what the rename input edits. */
function stemOf(fileName: string): string {
  const bare = bareName(fileName);
  const dot = bare.lastIndexOf(".");
  return dot > 0 ? bare.slice(0, dot) : bare;
}

const GROUP_KEY = "moonclip.gallery.group";
/** Clips whose row has no folder (missing legacy files). */
const FLAT = "__flat__";

function Thumb({
  clip,
  onOpen,
  onError,
}: {
  clip: ClipMetadata;
  onOpen: () => void;
  onError: (msg: string) => void;
}) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    let url: string | null = null;
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
        if (!cancelled) {
          onError(`thumb: ${String(e)}`);
          setSrc(null);
        }
      },
    );
    return () => {
      cancelled = true;
      if (url) URL.revokeObjectURL(url);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [clip.thumbnail_name]);
  if (!src) {
    return (
      <button
        onClick={onOpen}
        className="flex aspect-video w-full items-center justify-center rounded-lg bg-black/40 transition hover:bg-black/60 sm:aspect-auto sm:h-16 sm:w-28"
        title={clip.file_name}
      >
        <Clapperboard size={18} className="text-slate-600" />
      </button>
    );
  }
  return (
    <button onClick={onOpen} title={clip.file_name} className="w-full shrink-0 sm:w-auto">
      <img
        src={src}
        alt=""
        onError={() => onError(`thumb asset blocked: ${clip.thumbnail_name}`)}
        className="aspect-video w-full rounded-lg object-cover transition hover:brightness-125 sm:aspect-auto sm:h-16 sm:w-28"
      />
    </button>
  );
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

function timeNow() {
  return new Date().toLocaleTimeString();
}

interface RowActions {
  onToggleFavorite: (id: string) => void;
  onDelete: (id: string) => void;
  onTrim: (clip: ClipMetadata) => void;
  onAdvancedEdit: (clip: ClipMetadata) => void;
  onRename: (clip: ClipMetadata, name: string) => void;
  onShare: (clip: ClipMetadata) => void;
  onError: (msg: string) => void;
  onSuccess: () => void;
}

function ClipRow({
  clip,
  gameLabel,
  actions,
}: {
  clip: ClipMetadata;
  gameLabel: string;
  actions: RowActions;
}) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const startRename = () => {
    setDraft(stemOf(clip.file_name));
    setEditing(true);
  };
  const commitRename = () => {
    setEditing(false);
    const name = draft.trim();
    if (name && name !== stemOf(clip.file_name)) actions.onRename(clip, name);
  };
  const reveal = async () => {
    try {
      const abs = await absOf(clip.file_name);
      try {
        // Primary: select the file (needs a FileManager1 owner on the bus;
        // often missing on KDE without Dolphin running).
        await revealItemInDir(abs);
      } catch {
        // Fallback: plain-open the containing folder (xdg-open, guaranteed).
        const sep = abs.includes("\\") ? "\\" : "/";
        await openPath(abs.slice(0, abs.lastIndexOf(sep)));
      }
      actions.onSuccess();
    } catch (e) {
      actions.onError(`reveal: ${String(e)}`);
    }
  };

  const iconBtn =
    "rounded-lg p-1.5 text-slate-500 transition hover:bg-white/10 hover:text-slate-200";

  return (
    <li className="rounded-xl border border-white/5 bg-black/30 p-2.5 sm:pr-4">
      <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:gap-3">
        <Thumb clip={clip} onOpen={() => actions.onTrim(clip)} onError={actions.onError} />
        <div className="flex min-w-0 flex-1 items-center gap-3">
          <div className="min-w-0 flex-1 text-sm">
            {editing ? (
              <input
                autoFocus
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onFocus={(e) => e.currentTarget.select()}
                onKeyDown={(e) => {
                  if (e.key === "Enter") commitRename();
                  if (e.key === "Escape") setEditing(false);
                }}
                onBlur={() => setEditing(false)}
                className="w-full rounded-md border border-cyan-400/30 bg-black/40 px-2 py-0.5 font-medium text-slate-100 outline-none focus:border-cyan-400/60"
                title={t("gallery.rename")}
              />
            ) : (
              <p className="truncate font-medium text-slate-200" title={clip.file_name}>
                {bareName(clip.file_name)}
              </p>
            )}
            <p className="truncate font-mono text-xs text-slate-500">
              {gameLabel} ·{" "}
              <span className="text-cyan-300/80">{fmtDuration(clip.duration_ms)}</span> ·{" "}
              {fmtSize(clip.file_size_bytes)}{" "}
              {clip.cloud ? (
                <span className="inline-flex items-center gap-0.5 text-xs text-cyan-300/80">
                  <Cloud size={11} /> {t("gallery.cloud")}
                </span>
              ) : clip.drive_file_id ? (
                <span className="inline-flex items-center gap-0.5 text-xs text-emerald-300/80">
                  <CloudUpload size={11} /> {t("gallery.uploaded")}
                </span>
              ) : (
                !clip.exists && (
                  <span className="text-xs text-amber-400">({t("gallery.missing")})</span>
                )
              )}
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-1">
            <button
              onClick={() => actions.onAdvancedEdit(clip)}
              className={`${iconBtn} text-cyan-300/80 hover:text-cyan-200`}
              title={t("editor.open")}
            >
              <Wand2 size={15} />
            </button>
            <button onClick={startRename} className={iconBtn} title={t("gallery.rename")}>
              <Pencil size={15} />
            </button>
            <button
              onClick={() => actions.onShare(clip)}
              className={`${iconBtn} text-cyan-300/80 hover:text-cyan-200`}
              title={t("share.title")}
            >
              <CloudUpload size={15} />
            </button>
            <button
              onClick={() => actions.onTrim(clip)}
              className={iconBtn}
              title={t("trim.title")}
            >
              <Scissors size={15} />
            </button>
            <button onClick={() => void reveal()} className={iconBtn} title={t("gallery.reveal")}>
              <FolderOpen size={15} />
            </button>
            <button
              onClick={() => actions.onToggleFavorite(clip.id)}
              className={`rounded-lg p-1.5 transition ${clip.is_favorite ? "text-amber-300" : "text-slate-500 hover:text-amber-200"}`}
              title={t("gallery.favorite")}
            >
              <Star size={15} fill={clip.is_favorite ? "currentColor" : "none"} />
            </button>
            {clip.cloud && confirmDelete ? (
              <button
                onClick={() => actions.onDelete(clip.id)}
                className="rounded-lg bg-red-500/20 px-2 py-1 text-[10px] font-medium text-red-200 transition hover:bg-red-500/30"
                title={t("gallery.confirm_cloud_delete")}
              >
                {t("gallery.confirm_cloud_delete")}
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
                className="rounded-lg p-1.5 text-slate-500 transition hover:bg-red-500/20 hover:text-red-300"
                title={clip.cloud ? t("gallery.confirm_cloud_delete") : t("common.delete")}
              >
                <Trash2 size={15} />
              </button>
            )}
          </div>
        </div>
      </div>
    </li>
  );
}

export function GalleryView({
  refreshToken,
  onAdvancedEdit,
}: {
  refreshToken: number;
  onAdvancedEdit?: (clip: ClipMetadata) => void;
}) {
  const { t } = useTranslation();
  // Single shared instance: rows act on THIS list (a per-row instance would
  // refresh a phantom copy and the UI would look dead).
  const { clips, loading, refresh, toggleFavorite, deleteClip, purgeMissing, renameClip } =
    useClips();
  // Registered games only provide nicer labels; the groups themselves come
  // from the clips, so a deleted registration keeps its group forever.
  const { inputs } = useRegisteredInputs();
  const [lastError, setLastError] = useState<string | null>(null);
  const [purged, setPurged] = useState<number | null>(null);
  const [trimClip, setTrimClip] = useState<ClipMetadata | null>(null);
  const [shareClip, setShareClip] = useState<ClipMetadata | null>(null);
  const [showDrive, setShowDrive] = useState(false);
  const [download, setDownload] = useState<{ sent: number; total: number } | null>(null);
  const [group, setGroup] = useState<string>(
    () => localStorage.getItem(GROUP_KEY) ?? "all",
  );

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

  const groups = useMemo(() => {
    const map = new Map<string, { count: number; last: string }>();
    for (const c of clips) {
      const key = c.folder || FLAT;
      const g = map.get(key) ?? { count: 0, last: "" };
      g.count += 1;
      if (c.created_at > g.last) g.last = c.created_at;
      map.set(key, g);
    }
    return [...map.entries()]
      .map(([key, g]) => ({ key, ...g }))
      .sort((a, b) => b.last.localeCompare(a.last));
  }, [clips]);

  const labelFor = (key: string) => {
    if (key === FLAT) return t("gallery.unfiled");
    if (key === "Unknown") return t("gallery.no_game");
    return inputs.find((i) => i.clips_folder === key)?.display_name ?? key;
  };

  // A deleted game group disappears only when its last clip is deleted.
  useEffect(() => {
    if (group === "all" || group === "favorites") return;
    if (!groups.some((g) => g.key === group)) setGroup("all");
  }, [groups, group]);

  const select = (key: string) => {
    setGroup(key);
    localStorage.setItem(GROUP_KEY, key);
  };

  const visible = useMemo(() => {
    if (group === "all") return clips;
    if (group === "favorites") return clips.filter((c) => c.is_favorite);
    return clips.filter((c) => (c.folder || FLAT) === group);
  }, [clips, group]);

  const fail = (msg: string) => setLastError(`${timeNow()} · ${msg}`);
  const actions: RowActions = {
    onToggleFavorite: (id) =>
      toggleFavorite(id).then(
        () => setLastError(null),
        (e) => fail(String(e)),
      ),
    onDelete: (id) =>
      deleteClip(id).then(
        () => setLastError(null),
        (e) => fail(String(e)),
      ),
    onTrim: (clip) => setTrimClip(clip),
    onAdvancedEdit: (clip) => onAdvancedEdit?.(clip),
    onRename: (clip, name) =>
      renameClip(clip.id, name).then(
        () => setLastError(null),
        (e) => fail(String(e)),
      ),
    onShare: (clip) => setShareClip(clip),
    onError: fail,
    onSuccess: () => setLastError(null),
  };

  if (loading && clips.length === 0)
    return <p className="text-sm text-slate-400">{t("common.loading")}</p>;

  if (clips.length === 0) {
    return (
      <>
        <p className="mt-1 text-sm text-slate-400">{t("gallery.coming")}</p>
        <div className="mt-6 grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3">
          {[0, 1, 2].map((i) => (
            <div
              key={i}
              className="relative overflow-hidden rounded-xl border border-dashed border-white/10 bg-moonclip-card/60 p-6 text-center"
            >
              <Clapperboard size={22} className="mx-auto text-slate-600" />
              <p className="mt-2 text-xs text-slate-500">{t("gallery.empty")}</p>
            </div>
          ))}
        </div>
      </>
    );
  }

  const onPurge = () => {
    setPurged(null);
    purgeMissing().then(
      (n) => {
        setPurged(n);
        setLastError(null);
      },
      (e) => fail(String(e)),
    );
  };

  const favCount = clips.filter((c) => c.is_favorite).length;
  const navBtn = (active: boolean) =>
    `flex w-full shrink-0 items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs transition ${
      active
        ? "border border-cyan-400/20 bg-cyan-500/10 text-cyan-200"
        : "border border-transparent text-slate-400 hover:bg-white/5 hover:text-slate-200"
    }`;
  const countBadge = "ml-auto font-mono text-[10px] text-slate-500";

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
      {showDrive && (
        <DriveBrowser
          onClose={() => setShowDrive(false)}
          onDownloaded={() => {
            void refresh();
          }}
        />
      )}
      {trimClip && (
        <TrimPanel
          clip={trimClip}
          onAdvancedEdit={onAdvancedEdit}
          onClose={() => setTrimClip(null)}
          onSaved={() => {
            setLastError(null);
            void refresh();
          }}
        />
      )}
      <div className="mt-2 flex flex-col gap-4 lg:flex-row">
        <aside className="shrink-0 lg:w-52">
          <nav className="flex gap-1 overflow-x-auto pb-1 lg:flex-col lg:overflow-visible lg:pb-0">
            <button className={navBtn(group === "all")} onClick={() => select("all")}>
              <Clapperboard size={13} className="shrink-0" />
              <span className="truncate">{t("gallery.all")}</span>
              <span className={countBadge}>{clips.length}</span>
            </button>
            <button className={navBtn(group === "favorites")} onClick={() => select("favorites")}>
              <Star size={13} className="shrink-0" />
              <span className="truncate">{t("gallery.favorites")}</span>
              <span className={countBadge}>{favCount}</span>
            </button>
            <p className="hidden px-2.5 pt-2 text-[10px] uppercase tracking-wide text-slate-600 lg:block">
              {t("gallery.games")}
            </p>
            {groups.map((g) => (
              <button
                key={g.key}
                className={navBtn(group === g.key)}
                onClick={() => select(g.key)}
                title={g.key === FLAT ? t("gallery.unfiled") : g.key}
              >
                <Gamepad2 size={13} className="shrink-0" />
                <span className="truncate">{labelFor(g.key)}</span>
                <span className={countBadge}>{g.count}</span>
              </button>
            ))}
          </nav>
        </aside>
        <div className="min-w-0 flex-1">
          <div className="mb-2 flex flex-wrap items-center gap-2 sm:gap-3">
            {lastError && (
              <p className="flex-1 truncate font-mono text-xs text-red-400" title={lastError}>
                {lastError}
              </p>
            )}
            {purged !== null && !lastError && !download && (
              <p className="flex-1 text-xs text-slate-500">{t("gallery.purged", { count: purged })}</p>
            )}
            {download && (
              <p className="flex-1 text-xs text-cyan-300/80">
                {t("gallery.downloading")}{" "}
                {download.total > 0
                  ? `${Math.min(100, Math.round((download.sent / download.total) * 100))}%`
                  : ""}
              </p>
            )}
            <button
              onClick={() => setShowDrive(true)}
              className="ml-auto inline-flex items-center gap-1 text-xs text-slate-500 transition hover:text-cyan-200"
              title={t("drive.title")}
            >
              <HardDriveDownload size={12} /> {t("drive.browse")}
            </button>
            <button onClick={onPurge} className="text-xs text-slate-500 transition hover:text-slate-200" title={t("gallery.purge")}>
              {t("gallery.purge")}
            </button>
          </div>
          {visible.length === 0 ? (
            <p className="text-xs text-slate-500">{t("gallery.empty_group")}</p>
          ) : (
            <ul className="space-y-2">
              {visible.map((c) => (
                <ClipRow key={c.id} clip={c} gameLabel={labelFor(c.folder || FLAT)} actions={actions} />
              ))}
            </ul>
          )}
        </div>
      </div>
    </>
  );
}
