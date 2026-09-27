import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import Moveable from "react-moveable";
import {
  ArrowLeft,
  Copy,
  Download,
  Loader2,
  Pause,
  Play,
  Plus,
  Redo2,
  Scissors,
  Trash2,
  Type,
  X,
  Undo2,
  Volume2,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import { TimelineView } from "./TimelineView";
import { ExportDialog } from "./ExportDialog";
import { AudioTimeline } from "./audioEngine";
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

/** Fraction (or absolute) rotation snapped to 45° steps within `tolerance`. */
function snapDegrees(rotation: number, tolerance = 7): number {
  const snapped = Math.round(rotation / 45) * 45;
  return Math.abs(rotation - snapped) <= tolerance ? snapped : rotation;
}

/** Absolutely-positioned text overlay on the preview frame.
 *
 *  Position uses left/top minus half the MEASURED layout size instead of a
 *  percentage translate, and the transform carries only scale+rotate: that is
 *  a transform react-moveable parses cleanly, so its control box always hugs
 *  the content (the old translate(-50%,-50%) confused it). */
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
  const elRef = useRef<HTMLDivElement | null>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });

  // Measured BEFORE paint: otherwise the first frames draw the element from
  // its top-left anchor (no half-size centering) and it visibly snaps.
  useLayoutEffect(() => {
    const el = elRef.current;
    if (!el) return;
    const measure = () => setSize({ w: el.offsetWidth, h: el.offsetHeight });
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [overlay.text, overlay.fontSize, overlay.kind]);

  const ready = size.w > 0 && size.h > 0 && frame.w > 0 && frame.h > 0;

  return (
    <div
      ref={(el) => {
        elRef.current = el;
        refCb(overlay.id, el);
      }}
      onClick={(e) => {
        e.stopPropagation();
        useEditorStore.getState().select({ kind: "overlay", id: overlay.id });
      }}
      className={`absolute cursor-move select-none whitespace-pre-wrap text-center ${
        selected ? "outline outline-1 outline-dashed outline-cyan-300/70" : ""
      }`}
      style={{
        // Anchored at 0,0 with a leading px translate: Moveable updates that
        // same translate, so its control box always hugs the element and the
        // committed position is exactly what you see. Hidden until measured
        // (it is still laid out, so the measurement can happen).
        visibility: ready ? "visible" : "hidden",
        left: 0,
        top: 0,
        transform: `translate(${overlay.x * frame.w - size.w / 2}px, ${
          overlay.y * frame.h - size.h / 2
        }px) scale(${overlay.scale}) rotate(${overlay.rotation}deg)`,
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

/** Small labelled slider used by the clip-adjustment panel. */
function Adjust({
  label,
  value,
  min,
  max,
  step = 0.01,
  onChange,
  format,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  onChange: (v: number) => void;
  format?: (v: number) => string;
}) {
  return (
    <label className="space-y-0.5 text-[10px] text-slate-400">
      <span className="flex items-center">
        {label}
        <span className="ml-auto font-mono">{format ? format(value) : value.toFixed(2)}</span>
      </span>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        className="w-full accent-cyan-400"
      />
    </label>
  );
}

/** Per-track level meters (own rAF, direct DOM writes: no React re-renders). */
function LevelMeters({
  engineRef,
  labels,
}: {
  engineRef: React.MutableRefObject<AudioTimeline | null>;
  labels: [string, string];
}) {
  const bars = useRef<(HTMLElement | null)[]>([]);
  useEffect(() => {
    let raf = 0;
    const tick = () => {
      const engine = engineRef.current;
      const l = engine ? engine.levels() : { game: 0, mic: 0 };
      const vals = [l.game, l.mic];
      vals.forEach((v, i) => {
        const el = bars.current[i];
        if (el) el.style.width = `${Math.min(100, Math.round(v * 160))}%`;
      });
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [engineRef]);
  return (
    <div className="space-y-1">
      {labels.map((label, i) => (
        <div key={label} className="flex items-center gap-2 text-[10px] text-slate-500">
          <span className="w-12 shrink-0">{label}</span>
          <span className="h-1.5 flex-1 overflow-hidden rounded-full bg-white/10">
            <span
              ref={(el) => {
                bars.current[i] = el;
              }}
              className="block h-full w-0 rounded-full bg-emerald-400/80 transition-[width] duration-75"
            />
          </span>
        </div>
      ))}
    </div>
  );
}

/** Renders an overlay only while the playhead is inside its range (or it is
 *  selected). Subscribing here — instead of in the editor root — keeps the
 *  whole editor from re-rendering on every clock tick. */
function OverlayLayer(props: {
  overlay: Overlay;
  frame: { w: number; h: number };
  selected: boolean;
  refCb: (id: string, el: HTMLDivElement | null) => void;
}) {
  const { overlay, selected } = props;
  const visible = useEditorStore(
    (s) => s.playheadMs >= overlay.startMs && s.playheadMs < overlay.startMs + overlay.durationMs,
  );
  if (!visible && !selected) return null;
  return <OverlayView {...props} />;
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
  const [showLibrary, setShowLibrary] = useState(false);
  const [frame, setFrame] = useState({ w: 0, h: 0 });
  const [stems, setStems] = useState<{ label: string; peaks: number[][] }[]>([]);

  /** Fallback clock when the active source has no decodable audio. */
  const [videoClock, setVideoClock] = useState(false);

  const videoRef = useRef<HTMLVideoElement | null>(null);
  const frameRef = useRef<HTMLDivElement | null>(null);

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
  const lastTranslate = useRef<[number, number]>([0, 0]);
  const [, forceTick] = useState(0);

  const project = useEditorStore((s) => s.project);
  const sources = useEditorStore((s) => s.sources);
  const selection = useEditorStore((s) => s.selection);
  const playing = useEditorStore((s) => s.playing);
  const activeSourceId = useEditorStore((s) => s.activeSourceId);
  const total = project ? projectDurationMs(project) : 0;
  const selectedOverlay =
    project?.overlays.find((o) => selection?.kind === "overlay" && o.id === selection.id) ??
    null;
  const selectedSegment =
    project?.segments.find((s) => selection?.kind === "segment" && s.id === selection.id) ??
    null;
  // True when the selected clip is the one under the playhead (what you would
  // hear). Boolean selector: it re-evaluates each frame but only re-renders
  // the editor when it flips, so moving a slider of an off-playhead clip is
  // explained instead of looking broken.
  const selectedClipAudible = useEditorStore((s) => {
    if (!s.project || s.selection?.kind !== "segment") return true;
    const at = s.project.segments.find(
      (seg) =>
        s.playheadMs >= seg.timelineStartMs &&
        s.playheadMs < seg.timelineStartMs + segmentDurationMs(seg),
    );
    return !at || at.id === s.selection.id;
  });

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
        invoke<ClipMetadata[]>("list_clips")
          .then((clips) => {
            setLibrary(clips);
            void invoke("editor_log", {
              message: `library loaded: ${clips.length} clip(s)`,
            }).catch(() => {});
          })
          .catch((e) => {
            void invoke("editor_log", {
              message: `library FAILED to load: ${String(e)}`,
            }).catch(() => {});
          });
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

  const masterGain = project?.gainMaster ?? 1;
  /** Selected clip's own audio mix (falls back to the active source). */
  const clipSegment = selectedSegment ?? null;

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
    // KDE remembers per-app mute: clear it for our own stream (and retry
    // until the AudioContext has created it).
    void invoke<number>("editor_audio_health").catch(() => {});
    void decodeProject(project).then(() => {
      const id =
        project.segments.slice().sort((a, b) => a.timelineStartMs - b.timelineStartMs)[0]
          ?.sourceClipId ?? null;
      if (id) {
        useEditorStore.getState().setActiveSource(id);
        setStems(engineRef.current?.peaksFor(id) ?? []);
        setVideoClock(!engineRef.current?.hasAudio(id));
      }
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session]);

  // Which source's waveforms are shown: the one under the playhead. The clock
  // tick updates `activeSourceId` only when it changes, so this component does
  // not re-render every frame.
  const waveSourceId = activeSourceId;

  // One waveform plate PER SEGMENT (a source can appear many times). The
  // timeline position/size are computed inside TimelineView with dnd-timeline's
  // own `valueToPixels`, so the waves share the clip bars' scale exactly.
  const plates = useMemo(() => {
    if (!project || !waveSourceId) return [];
    const source = useEditorStore.getState().sources[waveSourceId];
    if (!source) return [];
    // Prefer the decoded stem length over the container metadata (a few ms).
    const decoded = engineRef.current?.durationMsFor(waveSourceId) ?? null;
    const dur = Math.max(1, decoded ?? source.durationMs);
    return project.segments
      .filter((s) => s.sourceClipId === waveSourceId)
      .sort((a, b) => a.timelineStartMs - b.timelineStartMs)
      .map((seg) => ({
        id: seg.id,
        startMs: seg.timelineStartMs,
        durationMs: segmentDurationMs(seg),
        from: Math.min(1, Math.max(0, seg.inMs / dur)),
        to: Math.min(1, Math.max(0, seg.outMs / dur)),
        gainMix: seg.gainMix,
        gainGame: seg.gainGame,
        gainMic: seg.gainMic,
      }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project, waveSourceId, session, stems]);

  // Paused, clicking a clip shows ITS waveforms even when the playhead is
  // elsewhere. Only on an actual selection change: scrubbing/playing must not
  // be overridden by an old selection.
  const lastWaveSelRef = useRef<string | null>(null);
  useEffect(() => {
    const selId = selection?.kind === "segment" ? selection.id : null;
    if (selId === lastWaveSelRef.current) return;
    lastWaveSelRef.current = selId;
    if (!selId || playing) return;
    const seg = useEditorStore.getState().project?.segments.find((s) => s.id === selId);
    if (!seg || seg.sourceClipId === useEditorStore.getState().activeSourceId) return;
    useEditorStore.getState().setActiveSource(seg.sourceClipId);
    setStems(engineRef.current?.peaksFor(seg.sourceClipId) ?? []);
  }, [selection, playing]);

  // Grow the visible window with the timeline (single source of truth).
  useEffect(() => {
    if (total + 1000 > visibleMs) setVisibleMs(total + 2000);
  }, [total, visibleMs]);

  // ---- Video slaving / fallback clock ------------------------------------
  // The <video> is a MUTED picture: it plays freely and is only corrected for
  // real desyncs (>=1 s, at most once every 3 s). Seeking per frame was a
  // death spiral (each seek restarts the GStreamer pipeline and re-requests
  // ranges from the media server) that ate the CPU and stuttered the audio.
  const lastCorrectionRef = useRef(0);
  const pendingSeekRef = useRef<number | null>(null);
  const seekRafRef = useRef(0);

  const flushVideoSeek = useCallback(() => {
    seekRafRef.current = 0;
    const v = videoRef.current;
    const target = pendingSeekRef.current;
    pendingSeekRef.current = null;
    if (v && target !== null) v.currentTime = target;
  }, []);

  /** Coalesce seeks to one per animation frame (scrubbing fires ~120 Hz). */
  const scheduleVideoSeek = useCallback(
    (sec: number) => {
      pendingSeekRef.current = sec;
      if (!seekRafRef.current) {
        seekRafRef.current = requestAnimationFrame(flushVideoSeek);
      }
    },
    [flushVideoSeek],
  );

  const syncVideo = useCallback(
    (ms: number, playing: boolean) => {
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
      if (needsSrc) {
        videoSegRef.current = seg;
        lastCorrectionRef.current = performance.now();
        v.playbackRate = Math.max(0.25, Math.min(4, seg.speed));
        v.src = source.videoUrl;
        v.load();
        const onMeta = () => {
          v.removeEventListener("loadedmetadata", onMeta);
          v.currentTime = targetSec;
          if (playing) void v.play().catch(() => {});
        };
        v.addEventListener("loadedmetadata", onMeta);
        return;
      }
      if (videoSegRef.current?.id !== seg.id) {
        videoSegRef.current = seg;
        v.playbackRate = Math.max(0.25, Math.min(4, seg.speed));
        if (playing) {
          // Same-source cut: hard-seek right away instead of waiting for the
          // 1 s drift correction, so the picture follows the audio instantly.
          lastCorrectionRef.current = performance.now();
          scheduleVideoSeek(targetSec);
        }
      }
      if (!playing) {
        // Paused scrub: follow the playhead, coalesced per frame.
        scheduleVideoSeek(targetSec);
        return;
      }
      if (v.paused) void v.play().catch(() => {});
      const driftMs = targetSec * 1000 - v.currentTime * 1000;
      const now = performance.now();
      if (Math.abs(driftMs) > 1000 && now - lastCorrectionRef.current > 3000) {
        lastCorrectionRef.current = now;
        scheduleVideoSeek(targetSec);
      }
    },
    [scheduleVideoSeek],
  );

  // ---- Transport ---------------------------------------------------------
  const pause = useCallback(() => {
    engineRef.current?.pause();
    videoRef.current?.pause();
    useEditorStore.getState().setPlaying(false);
  }, []);

  const play = useCallback(async () => {
    const p = useEditorStore.getState().project;
    if (!p) return;
    // Playing deselects overlays: editors hide the transform box while
    // previewing, so nothing looks "selected by itself".
    if (useEditorStore.getState().selection?.kind === "overlay") {
      useEditorStore.getState().select(null);
    }
    const engine = await decodeProject(p);
    void invoke<number>("editor_audio_health").catch(() => {});
    const ms = useEditorStore.getState().playheadMs;
    const seg = segmentAt(p.segments, ms) ?? p.segments[0];
    const hasAudio = seg ? engine.hasAudio(seg.sourceClipId) : false;
    setVideoClock(!hasAudio);
    syncVideo(ms, true);
    if (hasAudio) {
      await engine.play(p.segments, ms, masterGain);
      const v = videoRef.current;
      if (v) {
        v.muted = true;
        v.volume = 1;
      }
    } else {
      const v = videoRef.current;
      if (v) {
        v.muted = false;
        v.volume = Math.min(1, Math.max(0, (seg?.gainMix ?? 1) * masterGain));
      }
    }
    useEditorStore.getState().setPlaying(true);
  }, [decodeProject, masterGain, syncVideo]);

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
      const segUnder = segmentAt(p.segments, target);
      const srcId = segUnder?.sourceClipId ?? null;
      useEditorStore.getState().setActiveSource(srcId);
      setStems(engineRef.current?.peaksFor(srcId ?? "") ?? []);
      syncVideo(target, wasPlaying);
      void engineRef.current?.seek(target, p.segments, wasPlaying, masterGain);
    },
    [masterGain, syncVideo],
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
      const seg = segmentAt(p.segments, ms);
      const srcId = seg?.sourceClipId ?? null;
      if (srcId !== useEditorStore.getState().activeSourceId) {
        useEditorStore.getState().setActiveSource(srcId);
        setStems(engine?.peaksFor(srcId ?? "") ?? []);
      }
      if (!videoClock) {
        syncVideo(ms, true);
      } else if (v) {
        // Fallback: the audio comes from the video, so follow the clip's own
        // mix gain there too (per-clip sliders must keep working).
        v.volume = Math.min(1, Math.max(0, (seg?.gainMix ?? 1) * p.gainMaster));
      }
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

  // Live output level (shared GainNode, no reschedule ever).
  useEffect(() => {
    const engine = engineRef.current;
    engine?.setMaster(masterGain);
    const v = videoRef.current;
    if (!v) return;
    if (videoClock) {
      // Fallback: single-track audio comes from the video itself.
      v.muted = false;
      v.volume = Math.min(1, Math.max(0, masterGain));
    } else {
      // Invariant: the engine is the only audio source.
      v.muted = true;
    }
  }, [masterGain, videoClock, session]);

  // Per-clip mix sliders update the already-scheduled nodes in place.
  useEffect(() => {
    if (project) engineRef.current?.applySegmentGains(project.segments);
  }, [project]);

  // Frame size (font scaling / overlay coordinates) before paint too.
  useLayoutEffect(() => {
    const el = frameRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      setFrame({ w: el.clientWidth, h: el.clientHeight });
    });
    ro.observe(el);
    setFrame({ w: el.clientWidth, h: el.clientHeight });
    return () => ro.disconnect();
  }, [session]);

  // Selecting an overlay mounts its element in the same commit; one extra
  // render makes the ref available to Moveable.
  const overlaySelId = selection?.kind === "overlay" ? selection.id : null;
  useEffect(() => {
    forceTick((n) => n + 1);
  }, [overlaySelId]);

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
  // Clip audio panel: Game/Mic for multi-stem sources, one Volume (the mix
  // fallback) otherwise. The selected clip's own source decides, not the one
  // under the playhead.
  const clipStems = clipSegment
    ? (sources[clipSegment.sourceClipId]?.stems.length ?? 0)
    : 0;
  const selectedClipTitle = clipSegment
    ? (sources[clipSegment.sourceClipId]?.gameTitle ?? "")
    : "";
  const clipGainSliders: ["gainGame" | "gainMic" | "gainMix", string][] =
    clipStems > 1
      ? [
          ["gainGame", t("editor.game")],
          ["gainMic", t("editor.mic")],
        ]
      : [["gainMix", t("editor.clip_volume")]];
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
          <button onClick={() => setVisibleMs((v) => Math.max(5000, v / 1.6))} className={iconBtn} title={t("editor.zoom_in")}>
            <ZoomIn size={15} />
          </button>
          <button
            onClick={() => setVisibleMs((v) => Math.min(Math.max(total * 1.2, 5000), v * 1.6))}
            className={iconBtn}
            title={t("editor.zoom_out")}
          >
            <ZoomOut size={15} />
          </button>
          <button
            onClick={() => setShowLibrary(true)}
            className="ml-1 inline-flex items-center gap-1.5 rounded-lg border border-white/15 bg-white/5 px-3 py-1.5 text-xs font-semibold text-slate-200 transition hover:bg-white/10"
          >
            <Plus size={14} /> {t("editor.add_clip")}
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
          {/* Audio first: it is the panel people actually touch, and it must
              never be pushed off-screen by the clip-adjustment sliders. */}
          <p className="flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wide text-slate-500">
            <Volume2 size={12} /> {t("editor.audio")}
          </p>
          <p className="text-[10px] leading-snug text-slate-600">{t("editor.mix_hint")}</p>
          <p className="font-mono text-[10px] text-slate-500">
            {videoClock
              ? t("editor.mode_video")
              : t("editor.mode_stems", {
                  stems: stems.map((s) => s.label).join("+") || "—",
                })}
          </p>
          <LevelMeters engineRef={engineRef} labels={[t("editor.game"), t("editor.mic")]} />
          {project.gainMaster <= 0 && (
            <div className="flex items-center gap-2 rounded-lg border border-amber-400/40 bg-amber-400/10 px-2 py-1.5 text-[10px] text-amber-100">
              <span className="min-w-0 flex-1">{t("editor.master_zero")}</span>
              <button
                onClick={() => useEditorStore.getState().setMasterGain(1)}
                className="rounded border border-amber-300/40 bg-amber-300/20 px-2 py-0.5 font-semibold"
              >
                {t("editor.master_reset")}
              </button>
            </div>
          )}
          <label className="space-y-1 text-[11px] text-slate-400">
            <span className="flex items-center gap-1">
              {t("editor.master_global")}
              <span className="ml-auto font-mono">
                {Math.round(project.gainMaster * 100)}%
              </span>
            </span>
            <input
              type="range"
              min={0}
              max={200}
              value={Math.round(project.gainMaster * 100)}
              onChange={(e) =>
                useEditorStore.getState().setMasterGain(Number(e.target.value) / 100)
              }
              className="w-full accent-cyan-400"
            />
          </label>

          <div className="space-y-2 rounded-lg border border-cyan-500/20 bg-cyan-500/5 p-2">
            <p className="text-[10px] font-semibold uppercase tracking-wide text-cyan-200/80">
              {t("editor.clip_audio")}
            </p>
            {!clipSegment && (
              <p className="text-[10px] leading-snug text-slate-500">
                {t("editor.clip_audio_hint")}
              </p>
            )}
            {clipSegment && (
              <>
                {selectedClipTitle && (
                  <p className="truncate font-mono text-[10px] text-slate-500">
                    {selectedClipTitle}
                  </p>
                )}
                {clipGainSliders.map(([field, label]) => (
                  <label key={field} className="space-y-1 text-[11px] text-slate-400">
                    <span className="flex items-center gap-1">
                      {label}
                      <span className="ml-auto font-mono">
                        {Math.round(clipSegment[field] * 100)}%
                      </span>
                    </span>
                    <input
                      type="range"
                      min={0}
                      max={200}
                      value={Math.round(clipSegment[field] * 100)}
                      onChange={(e) =>
                        useEditorStore
                          .getState()
                          .setSegmentGain(
                            clipSegment.id,
                            field,
                            Number(e.target.value) / 100,
                          )
                      }
                      className="w-full accent-cyan-400"
                    />
                  </label>
                ))}
                <button
                  onClick={() =>
                    useEditorStore.getState().updateSegment(clipSegment.id, {
                      gainMix: 1,
                      gainGame: 1,
                      gainMic: 1,
                    })
                  }
                  className="w-full rounded border border-white/10 bg-white/5 px-2 py-1 text-[10px] text-slate-300 transition hover:bg-white/10"
                >
                  {t("editor.clip_audio_reset")}
                </button>
                {playing && !selectedClipAudible && (
                  <p className="text-[10px] leading-snug text-amber-300/90">
                    {t("editor.clip_audio_not_audible")}
                  </p>
                )}
              </>
            )}
          </div>
          <div className="mt-1 flex gap-2">
            <button
              onClick={() => setShowLibrary(true)}
              className="inline-flex flex-1 items-center justify-center gap-1.5 rounded-lg border border-cyan-500/30 bg-cyan-500/10 px-2 py-1.5 text-[11px] font-semibold text-cyan-100 transition hover:bg-cyan-500/20"
            >
              <Plus size={13} /> {t("editor.add_clip")}
            </button>
            <button
              onClick={addText}
              className="inline-flex flex-1 items-center justify-center gap-1.5 rounded-lg border border-amber-400/30 bg-amber-400/10 px-2 py-1.5 text-[11px] font-semibold text-amber-100 transition hover:bg-amber-400/20"
            >
              <Type size={13} /> {t("editor.add_text")}
            </button>
          </div>

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

          {selectedSegment && (
            <div className="space-y-2 rounded-lg border border-white/10 bg-black/20 p-2">
              <p className="text-[11px] font-semibold text-slate-300">{t("editor.speed")}</p>
              <div className="flex flex-wrap gap-1">
                {[0.25, 0.5, 1, 1.5, 2, 3, 4].map((v) => (
                  <button
                    key={v}
                    onClick={() => {
                      useEditorStore
                        .getState()
                        .updateSegment(selectedSegment.id, { speed: v });
                      // Re-schedule the audio at the new rate if playing.
                      if (useEditorStore.getState().playing) {
                        seek(useEditorStore.getState().playheadMs);
                      }
                    }}
                    className={`rounded border px-1.5 py-0.5 font-mono text-[10px] transition ${
                      Math.abs(selectedSegment.speed - v) < 0.001
                        ? "border-cyan-400/60 bg-cyan-500/20 text-cyan-100"
                        : "border-white/10 bg-white/5 text-slate-400 hover:bg-white/10"
                    }`}
                  >
                    {v}x
                  </button>
                ))}
              </div>
              <p className="text-[10px] text-slate-500">{t("editor.speed_hint")}</p>
            </div>
          )}

          {selectedSegment && (
            <details className="rounded-lg border border-white/10 bg-black/20">
              <summary className="cursor-pointer select-none px-2 py-1.5 text-[11px] font-semibold text-slate-300 [&::-webkit-details-marker]:hidden">
                {t("editor.adjust")}
              </summary>
              <div className="space-y-2 p-2 pt-0">
              <Adjust
                label={t("editor.zoom")}
                value={selectedSegment.zoom}
                min={1}
                max={3}
                onChange={(v) =>
                  useEditorStore.getState().updateSegment(selectedSegment.id, { zoom: v })
                }
                format={(v) => `${v.toFixed(2)}x`}
              />
              <Adjust
                label={t("editor.pos_x")}
                value={selectedSegment.offsetX}
                min={-1}
                max={1}
                onChange={(v) =>
                  useEditorStore.getState().updateSegment(selectedSegment.id, { offsetX: v })
                }
              />
              <Adjust
                label={t("editor.pos_y")}
                value={selectedSegment.offsetY}
                min={-1}
                max={1}
                onChange={(v) =>
                  useEditorStore.getState().updateSegment(selectedSegment.id, { offsetY: v })
                }
              />
              <Adjust
                label={t("editor.rotation")}
                value={selectedSegment.rotation}
                min={-180}
                max={180}
                step={1}
                onChange={(v) =>
                  useEditorStore.getState().updateSegment(selectedSegment.id, { rotation: v })
                }
                format={(v) => `${Math.round(v)}°`}
              />
              <Adjust
                label={t("editor.crop_l")}
                value={selectedSegment.cropLeft}
                min={0}
                max={0.45}
                onChange={(v) =>
                  useEditorStore.getState().updateSegment(selectedSegment.id, { cropLeft: v })
                }
                format={(v) => `${Math.round(v * 100)}%`}
              />
              <Adjust
                label={t("editor.crop_r")}
                value={selectedSegment.cropRight}
                min={0}
                max={0.45}
                onChange={(v) =>
                  useEditorStore.getState().updateSegment(selectedSegment.id, { cropRight: v })
                }
                format={(v) => `${Math.round(v * 100)}%`}
              />
              <Adjust
                label={t("editor.crop_t")}
                value={selectedSegment.cropTop}
                min={0}
                max={0.45}
                onChange={(v) =>
                  useEditorStore.getState().updateSegment(selectedSegment.id, { cropTop: v })
                }
                format={(v) => `${Math.round(v * 100)}%`}
              />
              <Adjust
                label={t("editor.crop_b")}
                value={selectedSegment.cropBottom}
                min={0}
                max={0.45}
                onChange={(v) =>
                  useEditorStore.getState().updateSegment(selectedSegment.id, { cropBottom: v })
                }
                format={(v) => `${Math.round(v * 100)}%`}
              />
              <Adjust
                label={t("editor.opacity")}
                value={selectedSegment.opacity}
                min={0}
                max={1}
                onChange={(v) =>
                  useEditorStore.getState().updateSegment(selectedSegment.id, { opacity: v })
                }
                format={(v) => `${Math.round(v * 100)}%`}
              />
              <Adjust
                label={t("editor.brightness")}
                value={selectedSegment.brightness}
                min={-0.6}
                max={0.6}
                onChange={(v) =>
                  useEditorStore.getState().updateSegment(selectedSegment.id, { brightness: v })
                }
              />
              <Adjust
                label={t("editor.contrast")}
                value={selectedSegment.contrast}
                min={0.5}
                max={1.8}
                onChange={(v) =>
                  useEditorStore.getState().updateSegment(selectedSegment.id, { contrast: v })
                }
              />
              <Adjust
                label={t("editor.saturation")}
                value={selectedSegment.saturation}
                min={0}
                max={2}
                onChange={(v) =>
                  useEditorStore.getState().updateSegment(selectedSegment.id, { saturation: v })
                }
              />
              <Adjust
                label={t("editor.temperature")}
                value={selectedSegment.temperature}
                min={-1}
                max={1}
                onChange={(v) =>
                  useEditorStore.getState().updateSegment(selectedSegment.id, { temperature: v })
                }
              />
              <Adjust
                label={t("editor.vignette")}
                value={selectedSegment.vignette}
                min={0}
                max={1}
                onChange={(v) =>
                  useEditorStore.getState().updateSegment(selectedSegment.id, { vignette: v })
                }
                format={(v) => `${Math.round(v * 100)}%`}
              />
              <button
                onClick={() =>
                  useEditorStore.getState().updateSegment(selectedSegment.id, {
                    zoom: 1,
                    offsetX: 0,
                    offsetY: 0,
                    rotation: 0,
                    cropLeft: 0,
                    cropTop: 0,
                    cropRight: 0,
                    cropBottom: 0,
                    opacity: 1,
                    brightness: 0,
                    contrast: 1,
                    saturation: 1,
                    gamma: 1,
                    temperature: 0,
                    vignette: 0,
                  })
                }
                className="w-full rounded border border-white/10 bg-white/5 px-2 py-1 text-[10px] text-slate-300 transition hover:bg-white/10"
              >
                {t("editor.reset")}
              </button>
              </div>
            </details>
          )}

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
              {/* Transport state is owned by play()/pause() only: the video
                  fires pause on every src swap/load, and reacting to it froze
                  the playhead while the audio engine kept playing. */}
              <video
                ref={videoRef}
                muted
                preload="auto"
                onEnded={() => {
                  if (videoClock) pause();
                }}
                className="h-full w-full cursor-pointer object-contain"
                style={
                  selectedSegment
                    ? {
                        transform:
                          `translate(${selectedSegment.offsetX * frame.w}px, ` +
                          `${selectedSegment.offsetY * frame.h}px) ` +
                          `scale(${selectedSegment.zoom}) rotate(${selectedSegment.rotation}deg)`,
                        opacity: selectedSegment.opacity,
                        clipPath:
                          selectedSegment.cropLeft > 0 ||
                          selectedSegment.cropTop > 0 ||
                          selectedSegment.cropRight > 0 ||
                          selectedSegment.cropBottom > 0
                            ? `inset(${selectedSegment.cropTop * 100}% ${selectedSegment.cropRight * 100}% ${
                                selectedSegment.cropBottom * 100
                              }% ${selectedSegment.cropLeft * 100}%)`
                            : undefined,
                        filter:
                          selectedSegment.brightness !== 0 ||
                          selectedSegment.contrast !== 1 ||
                          selectedSegment.saturation !== 1 ||
                          selectedSegment.temperature !== 0
                            ? `brightness(${1 + selectedSegment.brightness}) ` +
                              `contrast(${selectedSegment.contrast}) ` +
                              `saturate(${selectedSegment.saturation}) ` +
                              (selectedSegment.temperature > 0
                                ? `sepia(${selectedSegment.temperature * 0.5})`
                                : `hue-rotate(${selectedSegment.temperature * 20}deg)`)
                            : undefined,
                      }
                    : undefined
                }
              />
              {selectedSegment && selectedSegment.vignette > 0 && (
                <div
                  className="pointer-events-none absolute inset-0"
                  style={{
                    background:
                      "radial-gradient(ellipse at center, transparent 45%, rgba(0,0,0,0.9) 100%)",
                    opacity: selectedSegment.vignette,
                  }}
                />
              )}
              {project.overlays.map((o) => (
                <OverlayLayer
                  key={o.id}
                  overlay={o}
                  frame={frame}
                  selected={selection?.kind === "overlay" && selection.id === o.id}
                  refCb={(id, el) => {
                    overlayEls.current[id] = el;
                  }}
                />
              ))}
              {moveableTarget && moveableTarget.offsetWidth > 0 && !playing && (
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
                  onDrag={({ target, transform, translate }) => {
                    (target as HTMLElement).style.transform = transform;
                    lastTranslate.current = [translate[0], translate[1]];
                  }}
                  onDragEnd={() => {
                    const el = overlayEls.current[selection!.id];
                    const frect = frameRef.current?.getBoundingClientRect();
                    if (!el || !frect || frect.width === 0 || frect.height === 0) return;
                    // The leading translate places the element's top-left; the
                    // center is that plus half its layout size.
                    const cx = lastTranslate.current[0] + el.offsetWidth / 2;
                    const cy = lastTranslate.current[1] + el.offsetHeight / 2;
                    useEditorStore.getState().updateOverlay(selection!.id, {
                      x: Math.min(1, Math.max(0, cx / frect.width)),
                      y: Math.min(1, Math.max(0, cy / frect.height)),
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
                  onRotate={({ target, transform, rotation }) => {
                    const snapped = snapDegrees(rotation);
                    lastRotation.current = snapped;
                    // Apply the snapped rotation ourselves so what you see and
                    // what gets stored always match.
                    (target as HTMLElement).style.transform =
                      snapped === rotation
                        ? transform
                        : transform.replace(/rotate\([^)]*\)/, `rotate(${snapped}deg)`);
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

      {/* Timeline toolbar: always-visible entries for adding content */}
      <div className="flex items-center gap-2 border-t border-white/10 bg-black/40 px-3 py-1.5">
        <button
          onClick={() => setShowLibrary(true)}
          className="inline-flex items-center gap-1.5 rounded-lg border border-cyan-500/30 bg-cyan-500/10 px-2.5 py-1 text-xs font-semibold text-cyan-100 transition hover:bg-cyan-500/20"
        >
          <Plus size={13} /> {t("editor.add_clip")}
        </button>
        <button
          onClick={addText}
          className="inline-flex items-center gap-1.5 rounded-lg border border-amber-400/30 bg-amber-400/10 px-2.5 py-1 text-xs font-semibold text-amber-100 transition hover:bg-amber-400/20"
        >
          <Type size={13} /> {t("editor.add_text")}
        </button>
        <span className="ml-auto font-mono text-[10px] text-slate-600">
          {library.filter((c) => c.exists).length} clips
        </span>
      </div>

      {/* Timeline */}
      <div className="h-[210px] shrink-0 overflow-hidden border-t border-white/10 bg-black/30">
        <TimelineView
          audioTracks={session.audioTracks}
          stems={stems}
          plates={plates}
          master={masterGain}
          visibleEnd={visibleMs}
          onVisibleEnd={(ms) => setVisibleMs(Math.max(5000, ms))}
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

      {showLibrary && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4">
          <div className="w-full max-w-md rounded-2xl border border-white/10 bg-[#0b0f19] p-4 shadow-2xl">
            <div className="mb-3 flex items-center">
              <h3 className="text-sm font-semibold text-slate-100">{t("editor.add_clip")}</h3>
              <button
                onClick={() => setShowLibrary(false)}
                className="ml-auto rounded-lg p-1.5 text-slate-500 transition hover:bg-white/10 hover:text-slate-200"
              >
                <X size={15} />
              </button>
            </div>
            <div className="max-h-[50vh] space-y-1 overflow-y-auto pr-1">
              {library.filter((c) => c.exists).length === 0 && (
                <p className="text-xs text-slate-500">{t("editor.no_clips")}</p>
              )}
              {library
                .filter((c) => c.exists)
                .slice(0, 60)
                .map((c) => (
                  <button
                    key={c.id}
                    onClick={() => {
                      setShowLibrary(false);
                      void addClip(c);
                    }}
                    className="flex w-full items-center gap-2 rounded-lg border border-white/5 bg-black/30 px-3 py-1.5 text-left text-xs text-slate-200 transition hover:bg-cyan-500/10"
                    title={c.file_name}
                  >
                    <Plus size={13} className="shrink-0 text-cyan-300" />
                    <span className="min-w-0 flex-1 truncate">{c.game_title}</span>
                    <span className="font-mono text-[10px] text-slate-500">
                      {fmt(c.duration_ms)}
                    </span>
                  </button>
                ))}
            </div>
          </div>
        </div>
      )}

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
