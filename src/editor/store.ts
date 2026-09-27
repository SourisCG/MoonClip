import { create } from "zustand";
import { temporal } from "zundo";
import { immer } from "zustand/middleware/immer";
import type { EditProject, OutputSettings, Segment } from "./types";
import { projectDurationMs, segmentDurationMs } from "./types";

interface EditorState {
  /** Loaded session result; null until the backend session is open. */
  project: EditProject | null;
  selectedSegmentId: string | null;
  playheadMs: number;
  playing: boolean;
  /** Timeline zoom, pixels per second. */
  pxPerSecond: number;

  setProject: (project: EditProject) => void;
  setOutput: (patch: Partial<OutputSettings>) => void;
  updateSegment: (id: string, patch: Partial<Segment>) => void;
  moveSegment: (id: string, timelineStartMs: number) => void;
  trimSegment: (id: string, edge: "start" | "end", timelineMs: number) => void;
  splitAtPlayhead: () => void;
  duplicateSegment: (id: string) => void;
  deleteSegment: (id: string) => void;
  selectSegment: (id: string | null) => void;
  setPlayhead: (ms: number) => void;
  setPlaying: (playing: boolean) => void;
  setPxPerSecond: (px: number) => void;
  resetEditor: () => void;
}

const clamped = (v: number, min: number, max: number) => Math.min(Math.max(v, min), max);

export const useEditorStore = create<EditorState>()(
  temporal(
    immer((set) => ({
      project: null,
      selectedSegmentId: null,
      playheadMs: 0,
      playing: false,
      pxPerSecond: 60,

      setProject: (project) =>
        set((s) => {
          s.project = project;
          s.selectedSegmentId = project.segments[0]?.id ?? null;
          s.playheadMs = 0;
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
          const seg = s.project?.segments.find((x) => x.id === id);
          if (seg) seg.timelineStartMs = Math.max(0, Math.round(timelineStartMs));
        }),

      trimSegment: (id, edge, timelineMs) =>
        set((s) => {
          const seg = s.project?.segments.find((x) => x.id === id);
          if (!seg) return;
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
          s.selectedSegmentId = right.id;
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
          s.selectedSegmentId = copy.id;
        }),

      deleteSegment: (id) =>
        set((s) => {
          if (!s.project) return;
          s.project.segments = s.project.segments.filter((x) => x.id !== id);
          if (s.selectedSegmentId === id) {
            s.selectedSegmentId = s.project.segments[0]?.id ?? null;
          }
        }),

      selectSegment: (id) =>
        set((s) => {
          s.selectedSegmentId = id;
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
          s.selectedSegmentId = null;
          s.playheadMs = 0;
          s.playing = false;
        }),
    })),
    {
      // Undo/redo only the project (selection/playhead are ephemeral).
      partialize: (state) => ({ project: state.project }),
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
