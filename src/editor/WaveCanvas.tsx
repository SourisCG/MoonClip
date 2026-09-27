import { useEffect, useRef } from "react";

/** Draws precomputed min/max peaks on a canvas (no media element, no own
 *  progress: the timeline playhead is the only position indicator). */
export function WaveCanvas({
  peaks,
  color = "#0891b2",
}: {
  peaks: number[][] | undefined;
  color?: string;
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
      const barW = Math.max(1, width / per);
      const band = height / chans;
      ctx.fillStyle = color;
      for (let c = 0; c < chans; c++) {
        const center = (c + 0.5) * band;
        const half = band / 2 - 1;
        for (let p = 0; p < per; p++) {
          const max = peaks[c][p * 2] ?? 0;
          const min = peaks[c][p * 2 + 1] ?? 0;
          const top = center - max * half;
          const bottom = center - min * half;
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
  }, [peaks, color]);

  return <canvas ref={ref} className="block h-full w-full" />;
}
