import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import Moveable from "react-moveable";
import WaveSurfer from "wavesurfer.js";
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
        left: `${overlay.x * 100}%`,
        top: `${overlay.y * 100}%`,
        transform: `translate(-50%, -50%) scale(${overlay.scale}) rotate(${overlay.rotation}deg)`,
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

/** Heavy editor (E2/E3a/E5a): lazy chunk, maximized view, full teardown. */
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

  const videoRef = useRef<HTMLVideoElement | null>(null);
  const frameRef = useRef<HTMLDivElement | null>(null);
  const overlayEls = useRef<Record<string, HTMLDivElement | null>>({});
  const laneRefs = useRef<(HTMLDivElement | null)[]>([]);
  const wsRef = useRef<WaveSurfer[]>([]);
  const loadedClipRef = useRef<string | null>(null);
  const currentSegRef = useRef<Segment | null>(null);
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
      wsRef.current.forEach((w) => w.destroy());
      wsRef.current = [];
      const v = videoRef.current;
      if (v) {
        v.pause();
        v.removeAttribute("src");
        v.load();
      }
      if (!closedRef.current && sessionRef.current) {
        closedRef.current = true;
        void invoke("editor_close", { sessionId: sessionRef.current.sessionId }).catch(() => {});
      }
    };
  }, [clipId]);

  const exit = useCallback(async () => {
    videoRef.current?.pause();
    wsRef.current.forEach((w) => w.pause());
    if (sessionRef.current && !closedRef.current) {
      closedRef.current = true;
      await invoke("editor_close", { sessionId: sessionRef.current.sessionId }).catch(() => {});
    }
    useEditorStore.getState().resetEditor();
    useEditorStore.temporal.getState().clear();
    onExit();
  }, [onExit]);

  // ---- Playback (virtual player over segments/sources) ------------------
  const applyGains = useCallback(() => {
    const gains = [
      selectedSegment?.gainMix,
      selectedSegment?.gainGame,
      selectedSegment?.gainMic,
    ];
    wsRef.current.forEach((w, i) => w.setVolume(Math.min(1, gains[i] ?? 1)));
  }, [selectedSegment]);

  const loadSegment = useCallback(
    (seg: Segment, autoplay: boolean, seekMs?: number) => {
      const source = useEditorStore.getState().sources[seg.sourceClipId];
      const v = videoRef.current;
      if (!source || !v) return;
      currentSegRef.current = seg;
      const targetMs = seekMs ?? seg.inMs;
      if (loadedClipRef.current !== source.clipId) {
        loadedClipRef.current = source.clipId;
        v.src = source.videoUrl;
        v.load();
        wsRef.current.forEach((w, i) => {
          const stem = source.stems[i];
          if (stem) void w.load(stem.url);
        });
        const onMeta = () => {
          v.removeEventListener("loadedmetadata", onMeta);
          v.currentTime = targetMs / 1000;
          useEditorStore.getState().setPlayhead(
            seg.timelineStartMs + (targetMs - seg.inMs) / Math.max(0.05, seg.speed),
          );
          if (autoplay) {
            void v.play();
            wsRef.current.forEach((w) => void w.play());
            useEditorStore.getState().setPlaying(true);
          }
        };
        v.addEventListener("loadedmetadata", onMeta);
      } else {
        v.currentTime = targetMs / 1000;
        useEditorStore.getState().setPlayhead(
          seg.timelineStartMs + (targetMs - seg.inMs) / Math.max(0.05, seg.speed),
        );
        if (autoplay) {
          void v.play();
          wsRef.current.forEach((w) => void w.play());
          useEditorStore.getState().setPlaying(true);
        }
      }
      applyGains();
    },
    [applyGains],
  );

  const seek = useCallback(
    (ms: number) => {
      const p = useEditorStore.getState().project;
      if (!p) return;
      const clampedMs = Math.max(0, Math.min(ms, Math.max(0, projectDurationMs(p) - 30)));
      const seg = segmentAt(p.segments, clampedMs);
      const v = videoRef.current;
      if (!seg) {
        // Gap (or past the end): park the playhead without starting a source.
        currentSegRef.current = null;
        v?.pause();
        wsRef.current.forEach((w) => w.pause());
        useEditorStore.getState().setPlayhead(clampedMs);
        return;
      }
      const wasPlaying = useEditorStore.getState().playing;
      loadSegment(seg, wasPlaying, sourceMsFor(seg, clampedMs));
    },
    [loadSegment],
  );

  const pause = useCallback(() => {
    videoRef.current?.pause();
    wsRef.current.forEach((w) => w.pause());
    useEditorStore.getState().setPlaying(false);
  }, []);

  const play = useCallback(() => {
    const p = useEditorStore.getState().project;
    if (!p) return;
    const ms = useEditorStore.getState().playheadMs;
    const seg = segmentAt(p.segments, ms) ?? p.segments[0];
    if (!seg) return;
    loadSegment(seg, true, sourceMsFor(seg, segmentAt(p.segments, ms) ? ms : seg.timelineStartMs));
  }, [loadSegment]);

  const toggle = useCallback(() => {
    if (useEditorStore.getState().playing) pause();
    else play();
  }, [pause, play]);

  // Playback clock: video drives the timeline; segment boundaries advance.
  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    const tick = () => {
      const v = videoRef.current;
      const p = useEditorStore.getState().project;
      const seg = currentSegRef.current;
      if (!v || !p || !seg) return;
      const ms = v.currentTime * 1000;
      const timelineMs =
        seg.timelineStartMs + (ms - seg.inMs) / Math.max(0.05, seg.speed);
      useEditorStore.getState().setPlayhead(timelineMs);
      wsRef.current.forEach((w) => {
        try {
          if (Math.abs(w.getCurrentTime() * 1000 - ms) > 250) w.setTime(ms / 1000);
        } catch {
          /* not ready */
        }
      });
      if (ms >= seg.outMs - 25) {
        const end = seg.timelineStartMs + segmentDurationMs(seg);
        const next = p.segments
          .filter((s) => s.timelineStartMs >= end - 1)
          .sort((a, b) => a.timelineStartMs - b.timelineStartMs)[0];
        if (next) {
          loadSegment(next, true);
          raf = requestAnimationFrame(tick);
          return;
        }
        pause();
        useEditorStore.getState().setPlayhead(end);
        return;
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing, pause, loadSegment]);

  // ---- Waveforms ---------------------------------------------------------
  useEffect(() => {
    if (!session) return;
    const created: WaveSurfer[] = [];
    session.sources[0]?.stems.forEach((stem, i) => {
      const el = laneRefs.current[i];
      if (!el) return;
      const ws = WaveSurfer.create({
        container: el,
        url: stem.url,
        height: 30,
        waveColor: "#334155",
        progressColor: "#0891b2",
        cursorWidth: 0,
        barWidth: 1,
        barGap: 1,
        normalize: true,
        interact: false,
      });
      created.push(ws);
    });
    wsRef.current = created;
    loadedClipRef.current = null;
    // Sync the preview with the first segment (right source + frame).
    const first = session.project.segments
      .slice()
      .sort((a, b) => a.timelineStartMs - b.timelineStartMs)[0];
    if (first) {
      const v = videoRef.current;
      if (v && !v.src) v.src = session.sources[0].videoUrl;
      requestAnimationFrame(() => {
        const seg = useEditorStore.getState().project?.segments.find((s) => s.id === first.id);
        if (seg) loadSegment(seg, false, seg.inMs);
      });
    }
    return () => {
      created.forEach((w) => w.destroy());
      wsRef.current = [];
    };
  }, [session, loadSegment]);

  useEffect(() => {
    applyGains();
  }, [applyGains, session]);

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

  // Selecting an overlay needs one extra render: the element ref is attached
  // during commit, after `moveableTarget` was computed.
  useEffect(() => {
    forceTick((n) => n + 1);
  }, [selection]);

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
      forceTick((n) => n + 1);
    },
    [session],
  );

  const addText = useCallback(() => {
    const store = useEditorStore.getState();
    const at = store.playheadMs;
    store.addTextOverlay(at, 3000);
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
          <button onClick={() => setVisibleMs((v) => Math.max(1000, v / 1.6))} className={iconBtn} title={t("editor.zoom_in")}>
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
                  target={moveableTarget}
                  draggable
                  scalable
                  rotatable
                  keepRatio
                  origin={false}
                  onDrag={({ target, transform }) => {
                    (target as HTMLElement).style.transform =
                      `translate(-50%, -50%) ${transform}`;
                  }}
                  onDragEnd={({ target }) => {
                    const frect = frameRef.current?.getBoundingClientRect();
                    const trect = (target as HTMLElement).getBoundingClientRect();
                    if (!frect || frect.width === 0 || frect.height === 0) return;
                    useEditorStore.getState().updateOverlay(selection!.id, {
                      x: (trect.left + trect.width / 2 - frect.left) / frect.width,
                      y: (trect.top + trect.height / 2 - frect.top) / frect.height,
                    });
                  }}
                  onScale={({ scale }) => {
                    lastScale.current = scale[0];
                  }}
                  onScaleEnd={() => {
                    useEditorStore.getState().updateOverlay(selection!.id, {
                      scale: Math.max(0.1, (selectedOverlay?.scale ?? 1) * lastScale.current),
                    });
                    lastScale.current = 1;
                  }}
                  onRotate={({ rotation }) => {
                    lastRotation.current = rotation;
                  }}
                  onRotateEnd={() => {
                    useEditorStore
                      .getState()
                      .updateOverlay(selection!.id, { rotation: lastRotation.current });
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
                  play();
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
              </span>
            </div>
          </div>
        </div>
      </div>

      {/* Timeline */}
      <div className="h-[210px] shrink-0 overflow-hidden border-t border-white/10 bg-black/30">
        <TimelineView
          audioTracks={session.audioTracks}
          laneRefs={laneRefs}
          visibleEnd={visibleMs}
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
              play();
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
