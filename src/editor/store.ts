import { create } from "zustand";
import { temporal } from "zundo";
import { immer } from "zustand/middleware/immer";
import type {
  EditProject,
  EditorSourceInfo,
  OutputSettings,
  Overlay,
  Segment,
} from "./types";
import { defaultTextOverlay, projectDurationMs, segmentDurationMs } from "./types";

export type Selection =
  | { kind: "segment"; id: string }
  | { kind: "overlay"; id: string }
  | null;

interface EditorState {
  /** Loaded session result; null until the backend session is open. */
  project: EditProject | null;
  /** Media (video URL + stems) per source clip; not part of undo. */
  sources: Record<string, EditorSourceInfo>;
  selection: Selection;
  playheadMs: number;
  playing: boolean;
  /** Timeline zoom, pixels per second. */
  pxPerSecond: number;

  setProject: (project: EditProject) => void;
  setMasterGain: (value: number) => void;
  setSegmentGain: (
    id: string,
    field: "gainMix" | "gainGame" | "gainMic",
    value: number,
  ) => void;
  setSources: (sources: EditorSourceInfo[]) => void;
  upsertSource: (source: EditorSourceInfo) => void;
  addSegmentFromSource: (source: EditorSourceInfo, atMs: number | null) => void;
  addTextOverlay: (startMs: number, durationMs: number) => void;
  updateOverlay: (id: string, patch: Partial<Overlay>) => void;
  moveOverlay: (id: string, startMs: number) => void;
  resizeOverlay: (id: string, durationMs: number) => void;
  deleteOverlay: (id: string) => void;
  select: (selection: Selection) => void;
  setOutput: (patch: Partial<OutputSettings>) => void;
  updateSegment: (id: string, patch: Partial<Segment>) => void;
  moveSegment: (id: string, timelineStartMs: number) => void;
  trimSegment: (id: string, edge: "start" | "end", timelineMs: number) => void;
  splitAtPlayhead: () => void;
  duplicateSegment: (id: string) => void;
  deleteSegment: (id: string) => void;
  setPlayhead: (ms: number) => void;
  setPlaying: (playing: boolean) => void;
  setPxPerSecond: (px: number) => void;
  resetEditor: () => void;
}

const clamped = (v: number, min: number, max: number) => Math.min(Math.max(v, min), max);

/** Enforce non-overlapping segments: earlier starts win; on an exact tie the
 *  just-dropped segment (`pinnedId`) stays and the other is pushed right.
 *  Overlaps made two clips play at once (doubled audio) while the preview
 *  could only show one of them. */
function resolveOverlaps(segments: Segment[], pinnedId?: string) {
  segments.sort(
    (a, b) =>
      a.timelineStartMs - b.timelineStartMs ||
      (a.id === pinnedId ? -1 : b.id === pinnedId ? 1 : 0),
  );
  let cursor = 0;
  for (const seg of segments) {
    if (seg.timelineStartMs < cursor) seg.timelineStartMs = cursor;
    cursor = seg.timelineStartMs + segmentDurationMs(seg);
  }
}

export const useEditorStore = create<EditorState>()(
  temporal(
    immer((set) => ({
      project: null,
      sources: {},
      selection: null,
      playheadMs: 0,
      playing: false,
      pxPerSecond: 60,

      setProject: (project) =>
        set((s) => {
          s.project = project;
          s.selection = project.segments[0]
            ? { kind: "segment", id: project.segments[0].id }
            : null;
          s.playheadMs = 0;
        }),

      setMasterGain: (value) =>
        set((s) => {
          if (s.project) s.project.gainMaster = Math.max(0, Math.min(4, value));
        }),

      setSegmentGain: (id, field, value) =>
        set((s) => {
          const seg = s.project?.segments.find((x) => x.id === id);
          if (seg) seg[field] = Math.max(0, Math.min(4, value));
        }),

      setSources: (sources) =>
        set((s) => {
          s.sources = Object.fromEntries(sources.map((x) => [x.clipId, x]));
        }),

      upsertSource: (source) =>
        set((s) => {
          s.sources[source.clipId] = source;
        }),

      addSegmentFromSource: (source, atMs) =>
        set((s) => {
          const p = s.project;
          if (!p) return;
          const end = p.segments.reduce(
            (max, seg) => Math.max(max, seg.timelineStartMs + segmentDurationMs(seg)),
            0,
          );
          const seg: Segment = {
            id: crypto.randomUUID(),
            sourceClipId: source.clipId,
            inMs: 0,
            outMs: source.durationMs,
            timelineStartMs: atMs ?? end,
            speed: 1,
            freezeAtMs: 0,
            freezeMs: 0,
            // Game+Mic are the sum that the recording's Mix track contains:
            // default to those so nothing is doubled.
            gainMix: 0,
            gainGame: 1,
            gainMic: 1,
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
          };
          p.segments.push(seg);
          resolveOverlaps(p.segments, seg.id);
          s.selection = { kind: "segment", id: seg.id };
        }),

      addTextOverlay: (startMs, durationMs) =>
        set((s) => {
          const p = s.project;
          if (!p) return;
          const overlay = defaultTextOverlay(startMs, durationMs);
          p.overlays.push(overlay);
          s.selection = { kind: "overlay", id: overlay.id };
        }),

      updateOverlay: (id, patch) =>
        set((s) => {
          const o = s.project?.overlays.find((x) => x.id === id);
          if (!o) return;
          Object.assign(o, patch);
          // Keep overlays reachable: center inside the frame, sane scale/alpha.
          o.x = Math.min(1, Math.max(0, o.x));
          o.y = Math.min(1, Math.max(0, o.y));
          o.scale = Math.min(8, Math.max(0.1, o.scale));
          o.opacity = Math.min(1, Math.max(0, o.opacity));
        }),

      moveOverlay: (id, startMs) =>
        set((s) => {
          const o = s.project?.overlays.find((x) => x.id === id);
          if (o) o.startMs = Math.max(0, Math.round(startMs));
        }),

      resizeOverlay: (id, durationMs) =>
        set((s) => {
          const o = s.project?.overlays.find((x) => x.id === id);
          if (o) o.durationMs = Math.max(200, Math.round(durationMs));
        }),

      deleteOverlay: (id) =>
        set((s) => {
          if (!s.project) return;
          s.project.overlays = s.project.overlays.filter((x) => x.id !== id);
          if (s.selection?.kind === "overlay" && s.selection.id === id) {
            s.selection = null;
          }
        }),

      setOutput: (patch) =>
        set((s) => {
          if (!s.project) return;
          Object.assign(s.project.output, patch);
        }),

      updateSegment: (id, patch) =>
        set((s) => {
          const seg = s.project?.segments.find((x) => x.id === id);
          if (seg) Object.assign(seg, patch);
        }),

      moveSegment: (id, timelineStartMs) =>
        set((s) => {
          const p = s.project;
          const seg = p?.segments.find((x) => x.id === id);
          if (!p || !seg) return;
          seg.timelineStartMs = Math.max(0, Math.round(timelineStartMs));
          resolveOverlaps(p.segments, id);
        }),

      trimSegment: (id, edge, timelineMs) =>
        set((s) => {
          const p = s.project;
          const seg = p?.segments.find((x) => x.id === id);
          if (!p || !seg) return;
          const speed = Math.max(0.05, seg.speed);
          if (edge === "start") {
            // Timeline ms -> source ms at the new start; keep >=30 frames-ish.
            const delta = Math.round(timelineMs) - seg.timelineStartMs;
            const nextIn = clamped(seg.inMs + Math.round(delta * speed), 0, seg.outMs - 100);
            const applied = Math.round((nextIn - seg.inMs) / speed);
            seg.inMs = nextIn;
            seg.timelineStartMs = Math.max(0, seg.timelineStartMs + applied);
          } else {
            const wanted = Math.round(timelineMs) - seg.timelineStartMs;
            const nextOut = clamped(
              seg.inMs + Math.round(wanted * speed),
              seg.inMs + 100,
              24 * 60 * 60 * 1000,
            );
            seg.outMs = nextOut;
          }
          resolveOverlaps(p.segments, id);
        }),

      splitAtPlayhead: () =>
        set((s) => {
          const p = s.project;
          if (!p) return;
          const at = s.playheadMs;
          const idx = p.segments.findIndex(
            (seg) =>
              at > seg.timelineStartMs &&
              at < seg.timelineStartMs + segmentDurationMs(seg) - 50,
          );
          if (idx < 0) return;
          const seg = p.segments[idx];
          const speed = Math.max(0.05, seg.speed);
          const sourceSplit = Math.round(seg.inMs + (at - seg.timelineStartMs) * speed);
          const right: Segment = {
            ...seg,
            id: crypto.randomUUID(),
            inMs: sourceSplit,
            timelineStartMs: at,
          };
          p.segments[idx] = { ...seg, outMs: sourceSplit };
          p.segments.splice(idx + 1, 0, right);
          s.selection = { kind: "segment", id: right.id };
        }),

      duplicateSegment: (id) =>
        set((s) => {
          const p = s.project;
          const seg = p?.segments.find((x) => x.id === id);
          if (!p || !seg) return;
          const copy: Segment = {
            ...seg,
            id: crypto.randomUUID(),
            timelineStartMs: seg.timelineStartMs + segmentDurationMs(seg),
          };
          p.segments.push(copy);
          s.selection = { kind: "segment", id: copy.id };
        }),

      deleteSegment: (id) =>
        set((s) => {
          if (!s.project) return;
          s.project.segments = s.project.segments.filter((x) => x.id !== id);
          if (s.selection?.kind === "segment" && s.selection.id === id) {
            s.selection = s.project.segments[0]
              ? { kind: "segment", id: s.project.segments[0].id }
              : null;
          }
        }),

      select: (selection) =>
        set((s) => {
          s.selection = selection;
        }),

      setPlayhead: (ms) =>
        set((s) => {
          s.playheadMs = Math.max(0, ms);
        }),

      setPlaying: (playing) =>
        set((s) => {
          s.playing = playing;
        }),

      setPxPerSecond: (px) =>
        set((s) => {
          s.pxPerSecond = clamped(px, 8, 600);
        }),

      resetEditor: () =>
        set((s) => {
          s.project = null;
          s.sources = {};
          s.selection = null;
          s.playheadMs = 0;
          s.playing = false;
        }),
    })),
    {
      // Undo/redo only the project (selection/playhead are ephemeral), and
      // never record a history entry when only ephemeral state changed: at
      // 60 fps the default equality pushed a state per frame.
      partialize: (state) => ({ project: state.project }),
      equality: (a, b) => a.project === b.project,
      limit: 200,
    },
  ),
);

export function undo() {
  useEditorStore.temporal.getState().undo();
}
export function redo() {
  useEditorStore.temporal.getState().redo();
}
export function canUndo(): boolean {
  return useEditorStore.temporal.getState().pastStates.length > 0;
}
export function canRedo(): boolean {
  return useEditorStore.temporal.getState().futureStates.length > 0;
}

/** Total timeline length of the loaded project. */
export function timelineMs(): number {
  const p = useEditorStore.getState().project;
  return p ? projectDurationMs(p) : 0;
}
