import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import Moveable from "react-moveable";
import {
  ArrowLeft,
  Copy,
  Download,
  Film,
  Loader2,
  Pause,
  Play,
  Redo2,
  Scissors,
  Trash2,
  Type,
  Undo2,
  Volume2,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import { TimelineView } from "./TimelineView";
import { ExportDialog } from "./ExportDialog";
import { AudioTimeline, VIDEO_SYNC_TOLERANCE_MS } from "./audioEngine";
import { canRedo, canUndo, redo, undo, useEditorStore } from "./store";
import {
  projectDurationMs,
  segmentDurationMs,
  type EditorOpenResult,
  type EditorSourceInfo,
  type Overlay,
  type Segment,
} from "./types";
import type { ClipMetadata } from "../types";

const SIDEBAR_WIDTH = 88;

function fmt(ms: number) {
  const s = Math.max(0, ms) / 1000;
  const m = Math.floor(s / 60);
  return `${m}:${(s % 60).toFixed(1).padStart(4, "0")}`;
}

const segmentAt = (segments: Segment[], ms: number) =>
  segments.find(
    (s) => ms >= s.timelineStartMs && ms < s.timelineStartMs + segmentDurationMs(s),
  );
const sourceMsFor = (seg: Segment, timelineMs: number) =>
  seg.inMs + (timelineMs - seg.timelineStartMs) * Math.max(0.05, seg.speed);

function TimeLabel({ total }: { total: number }) {
  const ms = useEditorStore((s) => s.playheadMs);
  return (
    <span className="font-mono text-xs text-slate-400">
      {fmt(ms)} / {fmt(total)}
    </span>
  );
}

/** Scrub bar under the preview: click/drag anywhere to move the playhead. */
function ScrubBar({
  total,
  onSeek,
  onScrubStart,
  onScrubEnd,
}: {
  total: number;
  onSeek: (ms: number) => void;
  onScrubStart: () => void;
  onScrubEnd: () => void;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  const dragging = useRef(false);
  const ms = useEditorStore((s) => s.playheadMs);
  const at = (clientX: number) => {
    const rect = ref.current?.getBoundingClientRect();
    if (!rect || rect.width <= 0) return 0;
    return ((clientX - rect.left) / rect.width) * total;
  };
  return (
    <div
      ref={ref}
      className="group h-2 cursor-pointer rounded-full bg-white/10"
      onPointerDown={(e) => {
        dragging.current = true;
        onScrubStart();
        (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
        onSeek(at(e.clientX));
      }}
      onPointerMove={(e) => {
        if (dragging.current) onSeek(at(e.clientX));
      }}
      onPointerUp={(e) => {
        if (!dragging.current) return;
        dragging.current = false;
        try {
          (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
        } catch {
          /* released */
        }
        onScrubEnd();
      }}
    >
      <div
        className="h-full rounded-full bg-cyan-400/80 transition-[width] duration-75"
        style={{ width: `${total > 0 ? Math.min(100, (ms / total) * 100) : 0}%` }}
      />
    </div>
  );
}

/** Single source of truth for an overlay's transform: React renders exactly
 *  what Moveable produces, so the control box always hugs the content. */
function overlayTransform(o: Overlay, frame: { w: number; h: number }): string {
  const cx = o.x * frame.w;
  const cy = o.y * frame.h;
  return `translate(${cx}px, ${cy}px) translate(-50%, -50%) scale(${o.scale}) rotate(${o.rotation}deg)`;
}

/** Absolutely-positioned text overlay on the preview frame. */
function OverlayView({
  overlay,
  frame,
  selected,
  refCb,
}: {
  overlay: Overlay;
  frame: { w: number; h: number };
  selected: boolean;
  refCb: (id: string, el: HTMLDivElement | null) => void;
}) {
  const scale = frame.h > 0 ? frame.h / 1080 : 1;
  const isText = overlay.kind === "text";
  return (
    <div
      ref={(el) => refCb(overlay.id, el)}
      onClick={(e) => {
        e.stopPropagation();
        useEditorStore.getState().select({ kind: "overlay", id: overlay.id });
      }}
      className={`absolute cursor-move select-none whitespace-pre-wrap text-center ${
        selected ? "outline outline-1 outline-dashed outline-cyan-300/70" : ""
      }`}
      style={{
        left: 0,
        top: 0,
        transform: overlayTransform(overlay, frame),
        opacity: overlay.opacity,
        color: overlay.color,
        fontSize: `${Math.max(8, overlay.fontSize * scale)}px`,
        fontFamily: "sans-serif",
        fontWeight: isText ? 700 : 400,
        lineHeight: 1.15,
        WebkitTextStroke:
          overlay.strokeWidth > 0 && isText
            ? `${Math.max(1, overlay.strokeWidth * scale)}px ${overlay.strokeColor}`
            : undefined,
        textShadow: overlay.shadow ? "0 2px 6px rgba(0,0,0,0.65)" : undefined,
        paintOrder: "stroke fill",
      }}
    >
      {overlay.kind === "text" ? overlay.text : overlay.kind}
    </div>
  );
}

/** Heavy editor: lazy chunk, maximized view, full teardown.
 *  Playback uses a single AudioContext clock; the video is muted and slaved. */
export default function EditorApp({
  clipId,
  onExit,
  onClipSaved,
}: {
  clipId: string;
  onExit: () => void;
  onClipSaved: () => void;
}) {
  const { t } = useTranslation();
  const [session, setSession] = useState<EditorOpenResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [visibleMs, setVisibleMs] = useState(8000);
  const [showExport, setShowExport] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const [library, setLibrary] = useState<ClipMetadata[]>([]);
  const [frame, setFrame] = useState({ w: 0, h: 0 });
  const [stems, setStems] = useState<{ label: string; peaks: number[][] }[]>([]);
  const [laneWidth, setLaneWidth] = useState(0);
  /** Fallback clock when the active source has no decodable audio. */
  const [videoClock, setVideoClock] = useState(false);

  const videoRef = useRef<HTMLVideoElement | null>(null);
  const frameRef = useRef<HTMLDivElement | null>(null);
  const timelineWrapRef = useRef<HTMLDivElement | null>(null);
  const overlayEls = useRef<Record<string, HTMLDivElement | null>>({});
  const engineRef = useRef<AudioTimeline | null>(null);
  const videoSegRef = useRef<Segment | null>(null);
  const saveTimer = useRef<number | undefined>(undefined);
  const openingRef = useRef(false);
  const resumeAfterScrub = useRef(false);
  const sessionRef = useRef<EditorOpenResult | null>(null);
  const closedRef = useRef(false);
  const lastScale = useRef(1);
  const lastRotation = useRef(0);
  const [, forceTick] = useState(0);

  const project = useEditorStore((s) => s.project);
  const selection = useEditorStore((s) => s.selection);
  const playing = useEditorStore((s) => s.playing);
  const playhead = useEditorStore((s) => s.playheadMs);
  const total = project ? projectDurationMs(project) : 0;
  const selectedSegment =
    project?.segments.find((s) => selection?.kind === "segment" && s.id === selection.id) ??
    null;
  const selectedOverlay =
    project?.overlays.find((o) => selection?.kind === "overlay" && o.id === selection.id) ??
    null;

  // ---- Session lifecycle -------------------------------------------------
  useEffect(() => {
    if (openingRef.current) return;
    openingRef.current = true;
    invoke<EditorOpenResult>("editor_open", { clipId })
      .then((s) => {
        sessionRef.current = s;
        useEditorStore.getState().setProject(s.project);
        useEditorStore.getState().setSources(s.sources);
        useEditorStore.temporal.getState().clear();
        setSession(s);
        setVisibleMs(
          Math.max(6000, Math.min(30_000, s.project.segments[0]?.outMs ?? 8000) + 2000),
        );
        invoke<ClipMetadata[]>("list_clips").then(setLibrary).catch(() => {});
      })
      .catch((e) => setError(String(e)));
    return () => {
      const v = videoRef.current;
      if (v) {
        v.pause();
        v.removeAttribute("src");
        v.load();
      }
      engineRef.current?.dispose();
      engineRef.current = null;
      if (!closedRef.current && sessionRef.current) {
        closedRef.current = true;
        void invoke("editor_close", { sessionId: sessionRef.current.sessionId }).catch(() => {});
      }
    };
  }, [clipId]);

  const exit = useCallback(async () => {
    videoRef.current?.pause();
    engineRef.current?.dispose();
    engineRef.current = null;
    if (sessionRef.current && !closedRef.current) {
      closedRef.current = true;
      await invoke("editor_close", { sessionId: sessionRef.current.sessionId }).catch(() => {});
    }
    useEditorStore.getState().resetEditor();
    useEditorStore.temporal.getState().clear();
    onExit();
  }, [onExit]);

  // ---- Audio engine ------------------------------------------------------
  const ensureEngine = useCallback(() => {
    if (!engineRef.current) engineRef.current = new AudioTimeline();
    return engineRef.current;
  }, []);

  const gainsOf = useCallback(
    (seg: Segment) => ({
      mix: seg.gainMix,
      game: seg.gainGame,
      mic: seg.gainMic,
    }),
    [],
  );

  /** Decode every source used by the project so playback never stalls. */
  const decodeProject = useCallback(
    async (p: NonNullable<typeof project>) => {
      const engine = ensureEngine();
      const ids = Array.from(new Set(p.segments.map((s) => s.sourceClipId)));
      const store = useEditorStore.getState();
      for (const id of ids) {
        const source = store.sources[id];
        if (source) await engine.ensureSource(source);
      }
      return engine;
    },
    [ensureEngine],
  );

  // Decode the primary source as soon as the session is ready.
  useEffect(() => {
    if (!session || !project) return;
    void decodeProject(project).then(() => {
      const id = project.segments[0]?.sourceClipId;
      if (id) {
        setStems(engineRef.current?.peaksFor(id) ?? []);
        setVideoClock(!engineRef.current?.hasAudio(id));
      }
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session]);

  // Which source's waveforms are shown (the one under the playhead).
  const waveSourceId = useMemo(() => {
    if (!project) return null;
    const under = segmentAt(project.segments, playhead);
    if (under) return under.sourceClipId;
    const sorted = project.segments.slice().sort((a, b) => a.timelineStartMs - b.timelineStartMs);
    return sorted[0]?.sourceClipId ?? null;
  }, [project, playhead]);

  useEffect(() => {
    const engine = engineRef.current;
    if (!session || !engine || !waveSourceId) return;
    const source = useEditorStore.getState().sources[waveSourceId];
    if (!source) return;
    void engine.ensureSource(source).then(() => {
      setStems(engine.peaksFor(waveSourceId));
      const seg = useEditorStore.getState().project?.segments.find(
        (s) => s.sourceClipId === waveSourceId,
      );
      setVideoClock(!engine.hasAudio(seg?.sourceClipId ?? waveSourceId));
    });
  }, [session, waveSourceId, project?.segments.length]);

  // Lane width + waveform plate (same time->px mapping as the ruler).
  useEffect(() => {
    const el = timelineWrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setLaneWidth(Math.max(0, el.clientWidth - SIDEBAR_WIDTH)));
    ro.observe(el);
    setLaneWidth(Math.max(0, el.clientWidth - SIDEBAR_WIDTH));
    return () => ro.disconnect();
  }, [session]);

  const plate = useMemo(() => {
    if (!project || !waveSourceId) return { width: 0, offset: 0 };
    const source = useEditorStore.getState().sources[waveSourceId];
    const first = project.segments
      .filter((s) => s.sourceClipId === waveSourceId)
      .sort((a, b) => a.timelineStartMs - b.timelineStartMs)[0];
    if (!source || !first || laneWidth <= 0) return { width: 0, offset: 0 };
    const pxPerMs = laneWidth / Math.max(1000, visibleMs);
    return {
      width: Math.max(8, source.durationMs * pxPerMs),
      offset: (first.timelineStartMs - first.inMs) * pxPerMs,
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project, waveSourceId, laneWidth, visibleMs, session]);

  // Grow the visible window with the timeline (single source of truth).
  useEffect(() => {
    if (total + 1000 > visibleMs) setVisibleMs(total + 2000);
  }, [total, visibleMs]);

  // ---- Video slaving / fallback clock ------------------------------------
  const syncVideo = useCallback(
    (ms: number, autoplay: boolean) => {
      const p = useEditorStore.getState().project;
      const v = videoRef.current;
      if (!p || !v) return;
      const seg = segmentAt(p.segments, ms);
      if (!seg) {
        v.pause();
        return;
      }
      const source = useEditorStore.getState().sources[seg.sourceClipId];
      if (!source) return;
      const targetSec = sourceMsFor(seg, ms) / 1000;
      const needsSrc = videoSegRef.current?.sourceClipId !== seg.sourceClipId || !v.src;
      videoSegRef.current = seg;
      if (needsSrc) {
        v.src = source.videoUrl;
        v.load();
        const onMeta = () => {
          v.removeEventListener("loadedmetadata", onMeta);
          v.currentTime = targetSec;
          if (autoplay) void v.play().catch(() => {});
        };
        v.addEventListener("loadedmetadata", onMeta);
        return;
      }
      if (Math.abs(v.currentTime - targetSec) * 1000 > VIDEO_SYNC_TOLERANCE_MS) {
        v.currentTime = targetSec;
      }
      v.playbackRate = Math.max(0.25, Math.min(4, seg.speed));
      if (autoplay && v.paused) void v.play().catch(() => {});
    },
    [],
  );

  // ---- Transport ---------------------------------------------------------
  const pause = useCallback(() => {
    void engineRef.current?.pause();
    videoRef.current?.pause();
    useEditorStore.getState().setPlaying(false);
  }, []);

  const play = useCallback(async () => {
    const p = useEditorStore.getState().project;
    if (!p) return;
    const engine = await decodeProject(p);
    const ms = useEditorStore.getState().playheadMs;
    const seg = segmentAt(p.segments, ms) ?? p.segments[0];
    const hasAudio = seg ? engine.hasAudio(seg.sourceClipId) : false;
    setVideoClock(!hasAudio);
    syncVideo(ms, true);
    if (hasAudio) {
      await engine.play(p.segments, ms, gainsOf);
      await engine.resume();
      const v = videoRef.current;
      if (v) v.muted = true;
    } else {
      const v = videoRef.current;
      if (v) {
        v.muted = false;
        v.volume = Math.min(1, Math.max(0, seg?.gainMix ?? 1));
      }
    }
    useEditorStore.getState().setPlaying(true);
  }, [decodeProject, gainsOf, syncVideo]);

  const toggle = useCallback(() => {
    if (useEditorStore.getState().playing) pause();
    else void play();
  }, [pause, play]);

  const seek = useCallback(
    (ms: number) => {
      const p = useEditorStore.getState().project;
      if (!p) return;
      const target = Math.max(0, Math.min(ms, Math.max(0, projectDurationMs(p) - 30)));
      const wasPlaying = useEditorStore.getState().playing;
      useEditorStore.getState().setPlayhead(target);
      syncVideo(target, wasPlaying);
      void engineRef.current?.seek(target, p.segments, wasPlaying, gainsOf);
    },
    [gainsOf, syncVideo],
  );

  // Single clock: the playhead always follows the audio (or the video when the
  // source has no decodable audio).
  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    const tick = () => {
      const p = useEditorStore.getState().project;
      const v = videoRef.current;
      const engine = engineRef.current;
      if (!p) return;
      let ms: number;
      if (!videoClock && engine && engine.isPlaying()) {
        ms = engine.currentTimeMs();
      } else if (v) {
        const seg = videoSegRef.current;
        ms = seg
          ? seg.timelineStartMs + (v.currentTime * 1000 - seg.inMs) / Math.max(0.05, seg.speed)
          : v.currentTime * 1000;
      } else {
        return;
      }
      useEditorStore.getState().setPlayhead(ms);
      if (!videoClock) syncVideo(ms, true);
      const end = projectDurationMs(p);
      if (ms >= end - 20) {
        pause();
        useEditorStore.getState().setPlayhead(end);
        return;
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing, videoClock, syncVideo, pause]);

  // Live track gains (shared GainNodes, no reschedule).
  useEffect(() => {
    const engine = engineRef.current;
    if (!engine || !selectedSegment) return;
    engine.setTrackVolume("mix", Math.min(2, selectedSegment.gainMix));
    engine.setTrackVolume("game", Math.min(2, selectedSegment.gainGame));
    engine.setTrackVolume("mic", Math.min(2, selectedSegment.gainMic));
    const v = videoRef.current;
    if (v && videoClock) v.volume = Math.min(1, Math.max(0, selectedSegment.gainMix));
  }, [selectedSegment, videoClock, session]);

  // Frame size (font scaling / overlay coordinates).
  useEffect(() => {
    const el = frameRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      setFrame({ w: el.clientWidth, h: el.clientHeight });
    });
    ro.observe(el);
    setFrame({ w: el.clientWidth, h: el.clientHeight });
    return () => ro.disconnect();
  }, [session]);

  // ---- Autosave ----------------------------------------------------------
  useEffect(() => {
    if (!session) return;
    const unsub = useEditorStore.subscribe((s, prev) => {
      if (!s.project || s.project === prev.project) return;
      setDirty(true);
      window.clearTimeout(saveTimer.current);
      saveTimer.current = window.setTimeout(() => {
        invoke("editor_save_project", { project: s.project })
          .then(() => setDirty(false))
          .catch(() => {});
      }, 800);
    });
    return () => {
      unsub();
      window.clearTimeout(saveTimer.current);
    };
  }, [session]);

  // ---- Hotkeys -----------------------------------------------------------
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null;
      if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable))
        return;
      const mod = e.ctrlKey || e.metaKey;
      if (mod && e.key.toLowerCase() === "z" && !e.shiftKey) {
        e.preventDefault();
        undo();
      } else if (mod && (e.key.toLowerCase() === "y" || (e.shiftKey && e.key.toLowerCase() === "z"))) {
        e.preventDefault();
        redo();
      } else if (mod && (e.key.toLowerCase() === "k" || e.key.toLowerCase() === "s")) {
        e.preventDefault();
        useEditorStore.getState().splitAtPlayhead();
      } else if (mod && e.key.toLowerCase() === "d") {
        e.preventDefault();
        if (selection?.kind === "segment") {
          useEditorStore.getState().duplicateSegment(selection.id);
        }
      } else if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        if (selection?.kind === "segment") useEditorStore.getState().deleteSegment(selection.id);
        else if (selection?.kind === "overlay") useEditorStore.getState().deleteOverlay(selection.id);
      } else if (e.key === " ") {
        e.preventDefault();
        toggle();
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        seek(useEditorStore.getState().playheadMs - (e.shiftKey ? 1000 : 33));
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        seek(useEditorStore.getState().playheadMs + (e.shiftKey ? 1000 : 33));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [seek, selection, toggle]);

  // ---- Multi-clip: add another gallery clip ------------------------------
  const addClip = useCallback(
    async (clip: ClipMetadata) => {
      if (!session) return;
      const store = useEditorStore.getState();
      let source = store.sources[clip.id];
      if (!source) {
        source = await invoke<EditorSourceInfo>("editor_add_source", {
          sessionId: session.sessionId,
          clipId: clip.id,
        });
        store.upsertSource(source);
      }
      store.addSegmentFromSource(source, null);
      const engine = ensureEngine();
      await engine.ensureSource(source);
      forceTick((n) => n + 1);
    },
    [session, ensureEngine],
  );

  const addText = useCallback(() => {
    const store = useEditorStore.getState();
    store.addTextOverlay(store.playheadMs, 3000);
  }, []);

  const exportDone = useCallback(() => {
    onClipSaved();
    setToast(t("editor.export_done"));
    setShowExport(false);
    window.setTimeout(() => setToast(null), 4000);
  }, [onClipSaved, t]);

  // ---- Render ------------------------------------------------------------
  if (error) {
    return (
      <div className="flex h-full items-center justify-center">
        <div className="max-w-md rounded-xl border border-red-500/30 bg-red-500/10 p-4 text-sm text-red-200">
          <p className="break-words font-mono text-xs">{error}</p>
          <button
            onClick={() => void exit()}
            className="mt-3 rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-slate-200"
          >
            {t("editor.back")}
          </button>
        </div>
      </div>
    );
  }
  if (!session || !project) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 text-slate-400">
        <Loader2 size={22} className="animate-spin" />
        <p className="text-sm">{t("editor.loading")}</p>
      </div>
    );
  }

  const iconBtn =
    "rounded-lg border border-white/10 bg-white/5 p-2 text-slate-300 transition hover:bg-white/10 disabled:opacity-40";
  type GainField = "gainMix" | "gainGame" | "gainMic";
  const gains: [string, GainField, string][] = [
    ["mix", "gainMix", t("editor.mix")],
    ["game", "gainGame", t("editor.game")],
    ["mic", "gainMic", t("editor.mic")],
  ];
  const visibleOverlays = project.overlays.filter(
    (o) =>
      (playhead >= o.startMs && playhead < o.startMs + o.durationMs) ||
      (selection?.kind === "overlay" && selection.id === o.id),
  );
  const moveableTarget =
    selection?.kind === "overlay" ? overlayEls.current[selection.id] : null;

  return (
    <div className="flex h-full min-h-0 flex-col bg-[#070a12]">
      {/* Header */}
      <div className="flex flex-wrap items-center gap-2 border-b border-white/10 px-3 py-2">
        <button onClick={() => void exit()} className={iconBtn} title={t("editor.back")}>
          <ArrowLeft size={15} />
        </button>
        <div className="min-w-0">
          <p className="truncate text-sm font-semibold text-slate-100">{project.name}</p>
          <p className="truncate text-[11px] text-slate-500">
            {session.clip.gameTitle} · {session.clip.width}×{session.clip.height}
            {session.usingProxy ? ` · ${t("editor.proxy_active")}` : ""}
            {dirty ? " · …" : ""}
          </p>
        </div>
        <div className="ml-auto flex items-center gap-1.5">
          <button onClick={() => undo()} disabled={!canUndo()} className={iconBtn} title={t("editor.undo")}>
            <Undo2 size={15} />
          </button>
          <button onClick={() => redo()} disabled={!canRedo()} className={iconBtn} title={t("editor.redo")}>
            <Redo2 size={15} />
          </button>
          <button
            onClick={() => useEditorStore.getState().splitAtPlayhead()}
            className={iconBtn}
            title={`${t("editor.split")} (Ctrl+K)`}
          >
            <Scissors size={15} />
          </button>
          <button
            onClick={() =>
              selection?.kind === "segment" &&
              useEditorStore.getState().duplicateSegment(selection.id)
            }
            disabled={selection?.kind !== "segment"}
            className={iconBtn}
            title={`${t("editor.duplicate")} (Ctrl+D)`}
          >
            <Copy size={15} />
          </button>
          <button
            onClick={() =>
              selection?.kind === "segment"
                ? useEditorStore.getState().deleteSegment(selection.id)
                : selection?.kind === "overlay"
                  ? useEditorStore.getState().deleteOverlay(selection.id)
                  : undefined
            }
            disabled={!selection}
            className={iconBtn}
            title={`${t("editor.delete")} (Supr)`}
          >
            <Trash2 size={15} />
          </button>
          <button onClick={() => setVisibleMs((v) => Math.max(4000, v / 1.6))} className={iconBtn} title={t("editor.zoom_in")}>
            <ZoomIn size={15} />
          </button>
          <button
            onClick={() => setVisibleMs((v) => Math.min(Math.max(total * 1.2, 4000), v * 1.6))}
            className={iconBtn}
            title={t("editor.zoom_out")}
          >
            <ZoomOut size={15} />
          </button>
          <button
            onClick={() => setShowExport(true)}
            className="ml-1 inline-flex items-center gap-1.5 rounded-lg border border-cyan-500/40 bg-cyan-500/15 px-3 py-1.5 text-xs font-semibold text-cyan-100 transition hover:bg-cyan-500/25"
          >
            <Download size={14} /> {t("editor.export")}
          </button>
        </div>
      </div>

      <div className="flex min-h-0 flex-1">
        {/* Tool rail */}
        <aside className="flex w-56 shrink-0 flex-col gap-3 overflow-y-auto border-r border-white/10 p-3">
          <p className="flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wide text-slate-500">
            <Film size={12} /> {t("editor.clips")}
          </p>
          <div className="max-h-40 space-y-1 overflow-y-auto pr-1">
            {library
              .filter((c) => c.exists)
              .slice(0, 40)
              .map((c) => (
                <button
                  key={c.id}
                  onClick={() => void addClip(c)}
                  className="flex w-full items-center gap-2 rounded-lg border border-white/5 bg-black/20 px-2 py-1 text-left text-[11px] text-slate-300 transition hover:bg-white/10"
                  title={c.file_name}
                >
                  <span className="min-w-0 flex-1 truncate">{c.game_title}</span>
                  <span className="font-mono text-[10px] text-slate-500">
                    {fmt(c.duration_ms)}
                  </span>
                </button>
              ))}
          </div>
          <button
            onClick={addText}
            className="inline-flex items-center justify-center gap-1.5 rounded-lg border border-amber-400/30 bg-amber-400/10 px-3 py-1.5 text-xs font-semibold text-amber-100 transition hover:bg-amber-400/20"
          >
            <Type size={13} /> {t("editor.add_text")}
          </button>

          {selectedOverlay && (
            <div className="space-y-2 rounded-lg border border-amber-400/20 bg-amber-400/5 p-2">
              <p className="text-[11px] font-semibold text-amber-100">{t("editor.text")}</p>
              <textarea
                rows={2}
                value={selectedOverlay.text ?? ""}
                onChange={(e) =>
                  useEditorStore
                    .getState()
                    .updateOverlay(selectedOverlay.id, { text: e.target.value })
                }
                className="w-full rounded border border-white/10 bg-black/30 px-2 py-1 text-xs text-slate-100 outline-none focus:border-amber-300/50"
              />
              <label className="block text-[10px] text-slate-400">
                {t("editor.text_size")}
                <input
                  type="range"
                  min={16}
                  max={180}
                  value={selectedOverlay.fontSize}
                  onChange={(e) =>
                    useEditorStore
                      .getState()
                      .updateOverlay(selectedOverlay.id, { fontSize: Number(e.target.value) })
                  }
                  className="w-full accent-amber-300"
                />
              </label>
              <div className="flex gap-2">
                <label className="flex flex-1 items-center gap-1 text-[10px] text-slate-400">
                  {t("editor.text_color")}
                  <input
                    type="color"
                    value={selectedOverlay.color}
                    onChange={(e) =>
                      useEditorStore
                        .getState()
                        .updateOverlay(selectedOverlay.id, { color: e.target.value })
                    }
                    className="h-6 w-8 cursor-pointer rounded border border-white/10 bg-transparent"
                  />
                </label>
                <label className="flex flex-1 items-center gap-1 text-[10px] text-slate-400">
                  {t("editor.text_stroke")}
                  <input
                    type="color"
                    value={selectedOverlay.strokeColor}
                    onChange={(e) =>
                      useEditorStore
                        .getState()
                        .updateOverlay(selectedOverlay.id, { strokeColor: e.target.value })
                    }
                    className="h-6 w-8 cursor-pointer rounded border border-white/10 bg-transparent"
                  />
                </label>
              </div>
              <label className="block text-[10px] text-slate-400">
                {t("editor.text_opacity")}
                <input
                  type="range"
                  min={0}
                  max={100}
                  value={Math.round(selectedOverlay.opacity * 100)}
                  onChange={(e) =>
                    useEditorStore
                      .getState()
                      .updateOverlay(selectedOverlay.id, {
                        opacity: Number(e.target.value) / 100,
                      })
                  }
                  className="w-full accent-amber-300"
                />
              </label>
            </div>
          )}

          <p className="flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wide text-slate-500">
            <Volume2 size={12} /> {t("editor.audio")}
          </p>
          <p className="text-[10px] leading-snug text-slate-600">{t("editor.mix_hint")}</p>
          {!selectedSegment && <p className="text-[11px] text-slate-600">{t("editor.no_clip")}</p>}
          {selectedSegment &&
            gains.map(([key, field, label]) => (
              <label key={key} className="space-y-1 text-[11px] text-slate-400">
                <span className="flex items-center gap-1">
                  {label}
                  <span className="ml-auto font-mono">
                    {Math.round(selectedSegment[field] * 100)}%
                  </span>
                </span>
                <input
                  type="range"
                  min={0}
                  max={200}
                  value={Math.round(selectedSegment[field] * 100)}
                  onChange={(e) =>
                    useEditorStore
                      .getState()
                      .updateSegment(selectedSegment.id, {
                        [field]: Number(e.target.value) / 100,
                      })
                  }
                  className="w-full accent-cyan-400"
                />
              </label>
            ))}
          <p className="mt-auto text-[10px] leading-relaxed text-slate-600">
            {t("editor.shortcuts")}
          </p>
        </aside>

        {/* Preview + transport */}
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="flex min-h-0 flex-1 items-center justify-center bg-black/50 p-2">
            <div
              ref={frameRef}
              className="relative h-full"
              style={{
                aspectRatio: `${Math.max(2, session.clip.width)} / ${Math.max(2, session.clip.height)}`,
                maxWidth: "100%",
              }}
              onClick={toggle}
            >
              <video
                ref={videoRef}
                muted
                preload="auto"
                onPlay={() => useEditorStore.getState().setPlaying(true)}
                onPause={() => useEditorStore.getState().setPlaying(false)}
                className="h-full w-full cursor-pointer object-contain"
              />
              {visibleOverlays.map((o) => (
                <OverlayView
                  key={o.id}
                  overlay={o}
                  frame={frame}
                  selected={selection?.kind === "overlay" && selection.id === o.id}
                  refCb={(id, el) => {
                    overlayEls.current[id] = el;
                  }}
                />
              ))}
              {moveableTarget && (
                <Moveable
                  key={selection!.id}
                  target={moveableTarget}
                  draggable
                  scalable
                  rotatable
                  keepRatio
                  origin={false}
                  snappable
                  snapRotationDegrees={[0, 45, 90, 135, 180, 225, 270, 315]}
                  snapRotationThreshold={7}
                  snapDirections={{
                    top: true,
                    left: true,
                    bottom: true,
                    right: true,
                    center: true,
                    middle: true,
                  }}
                  verticalGuidelines={[0, frame.w / 2, frame.w]}
                  horizontalGuidelines={[0, frame.h / 2, frame.h]}
                  elementGuidelines={videoRef.current ? [videoRef.current] : []}
                  snapHorizontalThreshold={6}
                  snapVerticalThreshold={6}
                  snapGap={false}
                  onDrag={({ target, transform }) => {
                    (target as HTMLElement).style.transform = transform;
                  }}
                  onDragEnd={({ target }) => {
                    const frect = frameRef.current?.getBoundingClientRect();
                    const trect = (target as HTMLElement).getBoundingClientRect();
                    if (!frect || frect.width === 0 || frect.height === 0) return;
                    useEditorStore.getState().updateOverlay(selection!.id, {
                      x: (trect.left + trect.width / 2 - frect.left) / frect.width,
                      y: (trect.top + trect.height / 2 - frect.top) / frect.height,
                    });
                    forceTick((n) => n + 1);
                  }}
                  onScale={({ scale }) => {
                    lastScale.current = scale[0];
                  }}
                  onScaleEnd={() => {
                    useEditorStore.getState().updateOverlay(selection!.id, {
                      scale: Math.max(0.1, (selectedOverlay?.scale ?? 1) * lastScale.current),
                    });
                    lastScale.current = 1;
                    forceTick((n) => n + 1);
                  }}
                  onRotate={({ rotation }) => {
                    lastRotation.current = rotation;
                  }}
                  onRotateEnd={() => {
                    useEditorStore
                      .getState()
                      .updateOverlay(selection!.id, { rotation: lastRotation.current });
                    forceTick((n) => n + 1);
                  }}
                />
              )}
            </div>
          </div>
          <div className="space-y-2 border-t border-white/10 px-3 py-2">
            <ScrubBar
              total={total}
              onSeek={seek}
              onScrubStart={() => {
                if (useEditorStore.getState().playing) {
                  resumeAfterScrub.current = true;
                  pause();
                }
              }}
              onScrubEnd={() => {
                if (resumeAfterScrub.current) {
                  resumeAfterScrub.current = false;
                  void play();
                }
              }}
            />
            <div className="flex items-center gap-3">
              <button onClick={toggle} className={iconBtn} title={playing ? t("editor.pause") : t("editor.play")}>
                {playing ? <Pause size={15} /> : <Play size={15} />}
              </button>
              <TimeLabel total={total} />
              <span className="ml-auto font-mono text-[11px] text-slate-500">
                {project.segments.length} clips · {project.overlays.length} text
                {videoClock ? " · audio del video" : ""}
              </span>
            </div>
          </div>
        </div>
      </div>

      {/* Timeline */}
      <div ref={timelineWrapRef} className="h-[210px] shrink-0 overflow-hidden border-t border-white/10 bg-black/30">
        <TimelineView
          audioTracks={session.audioTracks}
          stems={stems}
          plate={plate}
          visibleEnd={visibleMs}
          onVisibleEnd={(ms) => setVisibleMs(Math.max(1000, ms))}
          onSeek={seek}
          onScrubStart={() => {
            if (useEditorStore.getState().playing) {
              resumeAfterScrub.current = true;
              pause();
            }
          }}
          onScrubEnd={() => {
            if (resumeAfterScrub.current) {
              resumeAfterScrub.current = false;
              void play();
            }
          }}
        />
      </div>

      {showExport && (
        <ExportDialog session={session} onClose={() => setShowExport(false)} onDone={exportDone} />
      )}
      {toast && (
        <div className="pointer-events-none fixed bottom-6 left-1/2 z-50 -translate-x-1/2 rounded-lg border border-cyan-500/30 bg-cyan-500/15 px-4 py-2 text-sm text-cyan-100">
          {toast}
        </div>
      )}
    </div>
  );
}
