import { useEffect, useRef } from "react";

/** Draws precomputed min/max peaks on a canvas (no media element, no own
 *  progress: the timeline playhead is the only position indicator).
 *  `from`/`to` are fractions of the source: a segment draws only its own
 *  [inMs, outMs] window, so each clip gets a bounded canvas. `gain` scales
 *  the drawn amplitude (clip gain x master) with a -1..1 clamp. */
export function WaveCanvas({
  peaks,
  color = "#0891b2",
  from = 0,
  to = 1,
  gain = 1,
}: {
  peaks: number[][] | undefined;
  color?: string;
  from?: number;
  to?: number;
  gain?: number;
}) {
  const ref = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const parent = canvas.parentElement;
    const draw = () => {
      const dpr = window.devicePixelRatio || 1;
      const width = Math.max(1, parent?.clientWidth ?? canvas.clientWidth);
      const height = Math.max(1, parent?.clientHeight ?? canvas.clientHeight);
      canvas.width = Math.floor(width * dpr);
      canvas.height = Math.floor(height * dpr);
      canvas.style.width = "100%";
      canvas.style.height = "100%";
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, width, height);
      if (!peaks || peaks.length === 0 || peaks[0].length === 0) return;
      const chans = peaks.length;
      const per = peaks[0].length / 2;
      // Exact window: each bar aggregates the buckets under its own fraction
      // of [from, to]. NEVER draw more bars than pixels: with a 1px floor the
      // waveform became `count` px wide instead of `width` and got clipped,
      // so zoomed-out clips showed their peaks at ~2x their real position.
      const a = Math.max(0, Math.min(1, from)) * per;
      const b = Math.max(a, Math.min(1, to) * per);
      const count = Math.max(
        1,
        Math.min(Math.max(1, Math.floor(width)), Math.round(b - a)),
      );
      const barW = width / count;
      const volume = Math.max(0, gain);
      const band = height / chans;
      ctx.fillStyle = color;
      for (let c = 0; c < chans; c++) {
        const center = (c + 0.5) * band;
        const half = band / 2 - 1;
        for (let p = 0; p < count; p++) {
          const start = a + (p * (b - a)) / count;
          const end = a + ((p + 1) * (b - a)) / count;
          const s = Math.max(0, Math.floor(start));
          const e = Math.min(per, Math.max(s + 1, Math.ceil(end)));
          let max = -1;
          let min = 1;
          for (let i = s; i < e; i++) {
            const hi = peaks[c][i * 2] ?? 0;
            const lo = peaks[c][i * 2 + 1] ?? 0;
            if (hi > max) max = hi;
            if (lo < min) min = lo;
          }
          if (max < min) {
            max = 0;
            min = 0;
          }
          const top = center - Math.min(1, max * volume) * half;
          const bottom = center - Math.max(-1, min * volume) * half;
          ctx.fillRect(
            p * barW,
            Math.min(top, bottom),
            Math.max(1, barW * 0.8),
            Math.max(1, Math.abs(bottom - top)),
          );
        }
      }
    };
    draw();
    const ro = new ResizeObserver(draw);
    if (parent) ro.observe(parent);
    return () => ro.disconnect();
  }, [peaks, color, from, to, gain]);

  return <canvas ref={ref} className="block h-full w-full" />;
}
