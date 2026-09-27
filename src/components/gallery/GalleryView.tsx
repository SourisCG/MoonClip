import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { thumbnailUrl } from "../../lib/media";
import {
  Clapperboard,
  Cloud,
  CloudUpload,
  Gamepad2,
  HardDriveDownload,
  Search,
  Star,
  Upload,
} from "lucide-react";
import { DriveBrowser } from "./DriveBrowser";
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

const GROUP_KEY = "moonclip.gallery.group";
const SORT_KEY = "moonclip.gallery.sort";
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

/** Medal-style 16:9 card: thumbnail, duration, quick actions on hover. */
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
  const iconBtn =
    "rounded-lg bg-black/60 p-1.5 text-slate-300 transition hover:bg-black/80 hover:text-cyan-200";

  return (
    <div className="group overflow-hidden rounded-xl border border-white/5 bg-black/30 transition hover:border-cyan-500/20">
      <div className="relative">
        <button onClick={onOpen} className="block w-full" title={clip.file_name}>
          <div className="flex aspect-video w-full items-center justify-center overflow-hidden bg-black/40">
            {src ? (
              <img
                src={src}
                alt=""
                onError={() => actions.onError(`thumb asset blocked: ${clip.thumbnail_name}`)}
                className="h-full w-full object-cover transition group-hover:brightness-110"
              />
            ) : (
              <Clapperboard size={22} className="text-slate-600" />
            )}
          </div>
        </button>
        <span className="pointer-events-none absolute bottom-1.5 right-1.5 rounded bg-black/75 px-1.5 py-0.5 font-mono text-[10px] text-slate-200">
          {fmtDuration(clip.duration_ms)}
        </span>
        {clip.cloud && (
          <span className="pointer-events-none absolute left-1.5 top-1.5 inline-flex items-center gap-0.5 rounded bg-black/75 px-1.5 py-0.5 text-[10px] text-cyan-300">
            <Cloud size={10} /> {t("gallery.cloud")}
          </span>
        )}
        <div className="absolute right-1.5 top-1.5 flex gap-1 opacity-0 transition group-hover:opacity-100">
          <button
            onClick={() => actions.onToggleFavorite(clip.id)}
            className={`${iconBtn} ${clip.is_favorite ? "text-amber-300" : ""}`}
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
      <div className="min-w-0 px-2 py-1.5">
        <p className="truncate text-sm font-medium text-slate-200" title={clip.file_name}>
          {bareName(clip.file_name)}
        </p>
        <p className="truncate font-mono text-[11px] text-slate-500">
          {gameLabel} · {fmtSize(clip.file_size_bytes)}{" "}
          {!clip.cloud && clip.drive_file_id && (
            <span className="inline-flex items-center gap-0.5 text-emerald-300/80">
              <Upload size={10} /> {t("gallery.uploaded")}
            </span>
          )}
          {!clip.cloud && !clip.drive_file_id && !clip.exists && (
            <span className="text-amber-400">({t("gallery.missing")})</span>
          )}
        </p>
      </div>
    </div>
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
  // Single shared instance: cards act on THIS list (a per-card instance would
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
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<string>(() => localStorage.getItem(SORT_KEY) ?? "recent");
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
    const fixed = ["all", "favorites", "uploaded", "cloud"];
    if (fixed.includes(group)) return;
    if (!groups.some((g) => g.key === group)) setGroup("all");
  }, [groups, group]);

  const select = (key: string) => {
    setGroup(key);
    localStorage.setItem(GROUP_KEY, key);
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

  const changeSort = (value: string) => {
    setSort(value);
    localStorage.setItem(SORT_KEY, value);
  };

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
  const uploadedCount = clips.filter((c) => !!c.drive_file_id).length;
  const cloudCount = clips.filter((c) => c.cloud).length;
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
            <button className={navBtn(group === "uploaded")} onClick={() => select("uploaded")}>
              <Upload size={13} className="shrink-0" />
              <span className="truncate">{t("gallery.uploaded_section")}</span>
              <span className={countBadge}>{uploadedCount}</span>
            </button>
            <button className={navBtn(group === "cloud")} onClick={() => select("cloud")}>
              <Cloud size={13} className="shrink-0" />
              <span className="truncate">{t("gallery.cloud_section")}</span>
              <span className={countBadge}>{cloudCount}</span>
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
          <div className="mb-3 flex flex-wrap items-center gap-2 sm:gap-3">
            <label className="relative min-w-0 flex-1 sm:max-w-xs">
              <Search
                size={13}
                className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-slate-500"
              />
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder={t("gallery.search")}
                className="w-full rounded-lg border border-white/10 bg-black/30 py-1.5 pl-7 pr-2 text-xs text-slate-200 outline-none placeholder:text-slate-600 focus:border-cyan-500/40"
              />
            </label>
            <select
              value={sort}
              onChange={(e) => changeSort(e.target.value)}
              className="rounded-lg border border-white/10 bg-black/30 px-2 py-1.5 text-xs text-slate-300 outline-none focus:border-cyan-500/40"
              title={t("gallery.sort")}
            >
              <option value="recent">{t("gallery.sort_recent")}</option>
              <option value="name">{t("gallery.sort_name")}</option>
              <option value="size">{t("gallery.sort_size")}</option>
            </select>
            <button
              onClick={() => setShowDrive(true)}
              className="inline-flex items-center gap-1 text-xs text-slate-500 transition hover:text-cyan-200"
              title={t("drive.title")}
            >
              <HardDriveDownload size={12} /> {t("drive.browse")}
            </button>
            <button
              onClick={onPurge}
              className="text-xs text-slate-500 transition hover:text-slate-200"
              title={t("gallery.purge")}
            >
              {t("gallery.purge")}
            </button>
          </div>
          {lastError && (
            <p className="mb-2 truncate font-mono text-xs text-red-400" title={lastError}>
              {lastError}
            </p>
          )}
          {purged !== null && !lastError && !download && (
            <p className="mb-2 text-xs text-slate-500">{t("gallery.purged", { count: purged })}</p>
          )}
          {download && (
            <p className="mb-2 text-xs text-cyan-300/80">
              {t("gallery.downloading")}{" "}
              {download.total > 0
                ? `${Math.min(100, Math.round((download.sent / download.total) * 100))}%`
                : ""}
            </p>
          )}
          {visible.length === 0 ? (
            <p className="text-xs text-slate-500">{t("gallery.empty_group")}</p>
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
        </div>
      </div>
    </>
  );
}
