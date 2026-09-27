import { useRef } from "react";
import {
  TimelineContext,
  useItem,
  useRow,
  useTimelineContext,
  type DragEndEvent,
  type Range,
  type ResizeEndEvent,
  type RowDefinition,
  type Span,
} from "dnd-timeline";
import { useEditorStore } from "./store";
import { WaveCanvas } from "./WaveCanvas";
import {
  segmentDurationMs,
  type EditorAudioTrack,
  type Overlay,
  type Segment,
} from "./types";

const SIDEBAR = 88;

function fmt(ms: number) {
  const s = Math.max(0, ms) / 1000;
  const m = Math.floor(s / 60);
  return `${m}:${(s % 60).toFixed(1).padStart(4, "0")}`;
}

/** One draggable/resizable video segment. dnd-timeline's `listeners` already
 *  route to drag-or-edge-resize: never override its onPointerDown. */
function SegmentItem({ segment }: { segment: Segment }) {
  const duration = segmentDurationMs(segment);
  const span: Span = {
    start: segment.timelineStartMs,
    end: segment.timelineStartMs + Math.max(100, duration),
  };
  const name = useEditorStore((s) => s.sources[segment.sourceClipId]?.gameTitle);
  const selected = useEditorStore(
    (s) => s.selection?.kind === "segment" && s.selection.id === segment.id,
  );
  const { setNodeRef, attributes, listeners, itemStyle, itemContentStyle } = useItem({
    id: segment.id,
    span,
    onResizeStart: () => useEditorStore.getState().select({ kind: "segment", id: segment.id }),
  });
  return (
    <div
      ref={setNodeRef}
      style={itemStyle}
      {...listeners}
      {...attributes}
      data-tl-item
      onClick={() => useEditorStore.getState().select({ kind: "segment", id: segment.id })}
    >
      <div style={itemContentStyle}>
        <div
          className={`h-full w-full cursor-grab overflow-hidden rounded-md border px-2 text-[11px] leading-[32px] ${
            selected
              ? "border-cyan-300/80 bg-cyan-500/25 text-cyan-50"
              : "border-white/15 bg-white/10 text-slate-200 hover:bg-white/15"
          }`}
          title={`${name ?? ""} · ${fmt(segment.inMs)} – ${fmt(segment.outMs)}`}
        >
          <span className="pointer-events-none select-none truncate">
            {name ? `${name} · ` : ""}
            {fmt(duration)}
            {Math.abs(segment.speed - 1) > 0.001 ? ` · ${segment.speed}x` : ""}
          </span>
        </div>
      </div>
    </div>
  );
}

/** Text/sticker overlay bar on its own timeline row. */
function OverlayItem({ overlay }: { overlay: Overlay }) {
  const span: Span = {
    start: overlay.startMs,
    end: overlay.startMs + Math.max(200, overlay.durationMs),
  };
  const selected = useEditorStore(
    (s) => s.selection?.kind === "overlay" && s.selection.id === overlay.id,
  );
  const { setNodeRef, attributes, listeners, itemStyle, itemContentStyle } = useItem({
    id: overlay.id,
    span,
    onResizeStart: () => useEditorStore.getState().select({ kind: "overlay", id: overlay.id }),
  });
  return (
    <div
      ref={setNodeRef}
      style={itemStyle}
      {...listeners}
      {...attributes}
      data-tl-item
      onClick={() => useEditorStore.getState().select({ kind: "overlay", id: overlay.id })}
    >
      <div style={itemContentStyle}>
        <div
          className={`h-full w-full cursor-grab overflow-hidden rounded-md border px-2 text-[11px] leading-[26px] ${
            selected
              ? "border-amber-300/80 bg-amber-400/25 text-amber-50"
              : "border-amber-400/30 bg-amber-400/10 text-amber-100 hover:bg-amber-400/20"
          }`}
          title={overlay.text ?? overlay.kind}
        >
          <span className="pointer-events-none select-none truncate">
            {overlay.kind === "text" ? overlay.text || "Texto" : overlay.kind}
          </span>
        </div>
      </div>
    </div>
  );
}

function Lane({
  row,
  label,
  height = 38,
  children,
}: {
  row: RowDefinition;
  label: string;
  height?: number;
  children?: React.ReactNode;
}) {
  const { setNodeRef, rowWrapperStyle, rowStyle, rowSidebarStyle } = useRow({ id: row.id });
  return (
    <div style={{ ...rowWrapperStyle, height }}>
      <div
        style={rowSidebarStyle}
        className="flex items-center border-r border-white/10 pr-2 text-[11px] text-slate-400"
      >
        {label}
      </div>
      <div
        ref={setNodeRef}
        style={rowStyle}
        className="overflow-hidden border-b border-white/5 bg-black/20"
      >
        {children}
      </div>
    </div>
  );
}

function Ruler({ visibleEnd }: { visibleEnd: number }) {
  const { range, valueToPixels, sidebarWidth } = useTimelineContext();
  const span = range.end - range.start;
  const stepMs = span > 60_000 ? 10_000 : span > 20_000 ? 5_000 : span > 8_000 ? 2_000 : 1_000;
  const ticks: number[] = [];
  for (let t = 0; t <= visibleEnd; t += stepMs) ticks.push(t);
  return (
    <div className="relative h-6 border-b border-white/10">
      {ticks.map((t) => (
        <div
          key={t}
          className="absolute top-0 h-full border-l border-white/10 pl-1 font-mono text-[9px] text-slate-500"
          style={{ left: sidebarWidth + valueToPixels(t - range.start) }}
        >
          {fmt(t)}
        </div>
      ))}
    </div>
  );
}

function Playhead() {
  const playhead = useEditorStore((s) => s.playheadMs);
  const { range, valueToPixels, sidebarWidth } = useTimelineContext();
  return (
    <div
      className="pointer-events-none absolute bottom-0 top-0 z-20 w-0.5 bg-cyan-300"
      style={{ left: sidebarWidth + valueToPixels(playhead - range.start) }}
    />
  );
}

function Inner({
  rows,
  waveSources,
  plates,
  master,
  visibleEnd,
  onSeek,
  onScrubStart,
  onScrubEnd,
}: {
  rows: (RowDefinition & { label?: string })[];
  waveSources: Record<string, { label: string; peaks: number[][] }[]>;
  plates: {
    id: string;
    sourceId: string;
    startMs: number;
    durationMs: number;
    from: number;
    to: number;
    gainMix: number;
    gainGame: number;
    gainMic: number;
  }[];
  master: number;
  visibleEnd: number;
  onSeek: (ms: number) => void;
  onScrubStart: () => void;
  onScrubEnd: () => void;
}) {
  const { setTimelineRef, style, pixelsToValue, valueToPixels, range, sidebarWidth } =
    useTimelineContext();
  const segments = useEditorStore((s) => s.project?.segments ?? []);
  const overlays = useEditorStore((s) => s.project?.overlays ?? []);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const scrubbing = useRef(false);

  const scrubTo = (clientX: number) => {
    const el = rootRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const x = clientX - rect.left - sidebarWidth;
    if (x < 0) return;
    onSeek(Math.max(0, pixelsToValue(x) + range.start));
  };

  const videoIdx = 0;
  const overlayStart = 1;
  const audioStart = 1 + overlays.length;

  return (
    <div
      ref={rootRef}
      onPointerDown={(e) => {
        const target = e.target as HTMLElement;
        if (target.closest("[data-tl-item]")) return;
        const rect = rootRef.current?.getBoundingClientRect();
        if (!rect || e.clientX - rect.left < sidebarWidth) return;
        scrubbing.current = true;
        onScrubStart();
        (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
        scrubTo(e.clientX);
      }}
      onPointerMove={(e) => {
        if (scrubbing.current) scrubTo(e.clientX);
      }}
      onPointerUp={(e) => {
        if (!scrubbing.current) return;
        scrubbing.current = false;
        try {
          (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
        } catch {
          /* already released */
        }
        onScrubEnd();
      }}
      onPointerCancel={() => {
        if (!scrubbing.current) return;
        scrubbing.current = false;
        onScrubEnd();
      }}
    >
      <div ref={setTimelineRef} style={style} className="select-none">
        <Ruler visibleEnd={visibleEnd} />
        {rows.map((row, idx) => (
          <Lane
            key={row.id}
            row={row}
            label={row.label ?? row.id}
            height={idx >= audioStart ? 34 : 38}
          >
            {idx === videoIdx &&
              segments.map((seg) => <SegmentItem key={seg.id} segment={seg} />)}
            {idx >= overlayStart &&
              idx < audioStart &&
              overlays
                .filter((o) => o.id === rows[idx].id)
                .map((o) => <OverlayItem key={o.id} overlay={o} />)}
            {idx >= audioStart &&
              (() => {
                const laneIdx = idx - audioStart;
                const laneLabel = rows[idx].label ?? "";
                const color = ["#0891b2", "#22c55e", "#f59e0b"][laneIdx] ?? "#0891b2";
                return (
                  <div className="relative h-full w-full">
                    {plates.map((pl) => {
                      // Each clip draws ITS OWN source's stems (matched by
                      // label, not by which clip is selected/under the playhead).
                      const src = waveSources[pl.sourceId] ?? [];
                      const stem =
                        src.find((s) => s.label === laneLabel) ?? src[laneIdx] ?? src[0];
                      // Position/size with dnd-timeline's own mapping: the exact
                      // same function used by the clip bars, the ruler and the
                      // playhead, so waves can never drift at any zoom.
                      const left = valueToPixels(pl.startMs - range.start);
                      const width = Math.max(2, valueToPixels(pl.durationMs));
                      // Scale each clip's wave by its own gain for this stem
                      // (x the global master), so the graph follows the mixer.
                      const laneGain =
                        stem?.label === "mic"
                          ? pl.gainMic
                          : stem?.label === "game"
                            ? pl.gainGame
                            : pl.gainMix;
                      return (
                        <div
                          key={pl.id}
                          className="absolute bottom-0 top-0 overflow-hidden"
                          style={{ left, width }}
                        >
                          <WaveCanvas
                            peaks={stem?.peaks}
                            color={color}
                            from={pl.from}
                            to={pl.to}
                            gain={laneGain * master}
                          />
                        </div>
                      );
                    })}
                  </div>
                );
              })()}
          </Lane>
        ))}
        <Playhead />
      </div>
    </div>
  );
}

/** Timeline: video lane (drag + trim by edges), one lane per text overlay and
 *  one lane per capture audio track. Clicking/dragging anywhere scrubs. */
export function TimelineView({
  audioTracks,
  waveSources,
  plates,
  master,
  visibleEnd,
  onVisibleEnd,
  onSeek,
  onScrubStart,
  onScrubEnd,
}: {
  audioTracks: EditorAudioTrack[];
  waveSources: Record<string, { label: string; peaks: number[][] }[]>;
  plates: {
    id: string;
    sourceId: string;
    startMs: number;
    durationMs: number;
    from: number;
    to: number;
    gainMix: number;
    gainGame: number;
    gainMic: number;
  }[];
  master: number;
  visibleEnd: number;
  onVisibleEnd: (ms: number) => void;
  onSeek: (ms: number) => void;
  onScrubStart: () => void;
  onScrubEnd: () => void;
}) {
  const project = useEditorStore((s) => s.project);
  const overlays = project?.overlays ?? [];
  // One window, derived from the parent: the ruler, the playhead and the
  // waveform plates must never use different scales (that was the drift).
  const range: Range = { start: 0, end: Math.max(5000, visibleEnd) };
  const rows: (RowDefinition & { label?: string })[] = [
    { id: "Video", label: "Video" },
    ...overlays.map((o, i) => ({
      id: o.id,
      label: `${o.kind === "text" ? "Texto" : o.kind} ${i + 1}`,
    })),
    ...audioTracks.map((t) => ({ id: t.label, label: t.label })),
  ];

  const onDragEnd = (event: DragEndEvent) => {
    const id = String(event.active.id);
    const span = event.active.data.current?.getSpanFromDragEvent?.(event) as Range | null;
    if (!span) return;
    const store = useEditorStore.getState();
    if (store.project?.segments.some((s) => s.id === id)) store.moveSegment(id, span.start);
    else if (store.project?.overlays.some((o) => o.id === id))
      store.moveOverlay(id, span.start);
  };
  const onResizeEnd = (event: ResizeEndEvent) => {
    const id = String(event.active.id);
    const span = event.active.data.current?.getSpanFromResizeEvent?.(event) as Range | null;
    if (!span) return;
    const store = useEditorStore.getState();
    const seg = store.project?.segments.find((s) => s.id === id);
    if (seg) {
      if (Math.abs(span.start - seg.timelineStartMs) > 1) store.trimSegment(id, "start", span.start);
      else store.trimSegment(id, "end", span.end);
      return;
    }
    const overlay = store.project?.overlays.find((o) => o.id === id);
    if (overlay) {
      const duration = span.end - span.start;
      if (Math.abs(span.start - overlay.startMs) > 1) {
        store.updateOverlay(id, { startMs: Math.max(0, span.start) });
      }
      store.resizeOverlay(id, duration);
    }
  };

  return (
    <TimelineContext
      range={range}
      sidebarWidth={SIDEBAR}
      onRangeChanged={(update) => onVisibleEnd(update(range).end)}
      onDragEnd={onDragEnd}
      onResizeEnd={onResizeEnd}
      resizeHandleWidth={14}
    >
      <Inner
        rows={rows}
        waveSources={waveSources}
        plates={plates}
        master={master}
        visibleEnd={visibleEnd}
        onSeek={onSeek}
        onScrubStart={onScrubStart}
        onScrubEnd={onScrubEnd}
      />
    </TimelineContext>
  );
}
