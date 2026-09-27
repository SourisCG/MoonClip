import { useEffect, useState } from "react";
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
import { projectDurationMs, segmentDurationMs, type EditorAudioTrack, type Segment } from "./types";

const SIDEBAR = 88;

function fmt(ms: number) {
  const s = Math.max(0, ms) / 1000;
  const m = Math.floor(s / 60);
  return `${m}:${(s % 60).toFixed(1).padStart(4, "0")}`;
}

/** One draggable/resizable video segment (dnd-timeline handles both edges). */
function SegmentItem({ segment }: { segment: Segment }) {
  const duration = segmentDurationMs(segment);
  const span: Span = {
    start: segment.timelineStartMs,
    end: segment.timelineStartMs + Math.max(100, duration),
  };
  const { setNodeRef, attributes, listeners, itemStyle, itemContentStyle } = useItem({
    id: segment.id,
    span,
  });
  const selected = useEditorStore((s) => s.selectedSegmentId === segment.id);
  return (
    <div
      ref={setNodeRef}
      style={itemStyle}
      {...listeners}
      {...attributes}
      onPointerDown={() => useEditorStore.getState().selectSegment(segment.id)}
    >
      <div style={itemContentStyle}>
        <div
          className={`h-full w-full cursor-grab overflow-hidden rounded-md border px-2 text-[11px] leading-[32px] ${
            selected
              ? "border-cyan-300/80 bg-cyan-500/25 text-cyan-50"
              : "border-white/15 bg-white/10 text-slate-200 hover:bg-white/15"
          }`}
          title={`${fmt(segment.inMs)} – ${fmt(segment.outMs)}`}
        >
          <span className="pointer-events-none select-none truncate">{fmt(duration)}</span>
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
  onSeekAt,
}: {
  row: RowDefinition;
  label: string;
  height?: number;
  children?: React.ReactNode;
  onSeekAt: (clientX: number, rectLeft: number) => void;
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
        style={{ ...rowStyle, cursor: "crosshair" }}
        className="border-b border-white/5 bg-black/20"
        onPointerDown={(e) => {
          // Only empty-track clicks seek; item/handle drags are untouched.
          if (e.target !== e.currentTarget) return;
          const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
          onSeekAt(e.clientX, rect.left);
        }}
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
  laneRefs,
  visibleEnd,
  onSeek,
}: {
  rows: RowDefinition[];
  laneRefs: React.MutableRefObject<(HTMLDivElement | null)[]>;
  visibleEnd: number;
  onSeek: (ms: number) => void;
}) {
  const { setTimelineRef, style, pixelsToValue, range } = useTimelineContext();
  const segments = useEditorStore((s) => s.project?.segments ?? []);
  const seekAt = (clientX: number, rectLeft: number) =>
    onSeek(Math.max(0, pixelsToValue(clientX - rectLeft) + range.start));
  return (
    <div ref={setTimelineRef} style={style} className="relative select-none">
      <Ruler visibleEnd={visibleEnd} />
      {rows.map((row, idx) => (
        <Lane key={row.id} row={row} label={row.id} onSeekAt={seekAt}>
          {idx === 0 &&
            segments.map((seg) => <SegmentItem key={seg.id} segment={seg} />)}
          {idx > 0 && (
            <div
              ref={(el) => {
                laneRefs.current[idx - 1] = el;
              }}
              className="h-full w-full"
            />
          )}
        </Lane>
      ))}
      <Playhead />
    </div>
  );
}

/** Timeline: one video lane (draggable/trimable segments) plus one lane per
 *  capture audio track (wavesurfer canvases injected by the parent). */
export function TimelineView({
  audioTracks,
  laneRefs,
  visibleEnd,
  onSeek,
}: {
  audioTracks: EditorAudioTrack[];
  laneRefs: React.MutableRefObject<(HTMLDivElement | null)[]>;
  visibleEnd: number;
  onSeek: (ms: number) => void;
}) {
  const project = useEditorStore((s) => s.project);
  const total = project ? projectDurationMs(project) : 0;
  const [range, setRange] = useState<Range>({ start: 0, end: Math.max(5000, visibleEnd) });

  useEffect(() => {
    setRange({ start: 0, end: Math.max(5000, visibleEnd) });
  }, [visibleEnd]);

  // Keep the fixed window big enough while the timeline grows.
  useEffect(() => {
    setRange((r) => (total + 1000 > r.end ? { start: 0, end: total + 2000 } : r));
  }, [total]);

  const rows: RowDefinition[] = [
    { id: "Video" },
    ...audioTracks.map((t) => ({ id: t.label })),
  ];

  const onDragEnd = (event: DragEndEvent) => {
    const span = event.active.data.current?.getSpanFromDragEvent?.(event) as Range | null;
    if (span) useEditorStore.getState().moveSegment(String(event.active.id), span.start);
  };
  const onResizeEnd = (event: ResizeEndEvent) => {
    const id = String(event.active.id);
    const before = useEditorStore.getState().project?.segments.find((s) => s.id === id);
    const span = event.active.data.current?.getSpanFromResizeEvent?.(event) as Range | null;
    if (!before || !span) return;
    if (Math.abs(span.start - before.timelineStartMs) > 1) {
      useEditorStore.getState().trimSegment(id, "start", span.start);
    } else {
      useEditorStore.getState().trimSegment(id, "end", span.end);
    }
  };

  return (
    <TimelineContext
      range={range}
      sidebarWidth={SIDEBAR}
      onRangeChanged={(update) => setRange((prev) => update(prev))}
      onDragEnd={onDragEnd}
      onResizeEnd={onResizeEnd}
      resizeHandleWidth={14}
    >
      <Inner rows={rows} laneRefs={laneRefs} visibleEnd={visibleEnd} onSeek={onSeek} />
    </TimelineContext>
  );
}
