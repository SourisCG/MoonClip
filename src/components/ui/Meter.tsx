import clsx from "clsx";
import { levelToDb, levelToPercent } from "../../lib/audio";

/** OBS-like fixed zones: green to 75 %, amber to 92 %, red at the top. */
const ZONES =
  "linear-gradient(to right, #3fb950 0 75%, #d29922 75% 92%, #ef4444 92% 100%)";

/**
 * OBS-style horizontal level meter. The green/amber/red scale is always
 * visible; the current level reveals it at full brightness while the unused
 * part stays dimmed, plus a peak tick so quiet signals still read.
 */
export function Meter({
  level,
  peak,
  showDb,
  className,
}: {
  level: number;
  peak?: number;
  showDb?: boolean;
  className?: string;
}) {
  const percent = levelToPercent(level);
  const peakPercent = Math.max(percent, levelToPercent(peak ?? 0));
  return (
    <div className={clsx("flex items-center gap-2", className)}>
      <div className="relative h-2.5 flex-1 overflow-hidden rounded-[3px] border border-line bg-black/70">
        {/* The fixed scale. */}
        <span aria-hidden className="absolute inset-0 opacity-30" style={{ background: ZONES }} />
        {/* Revealed (loud) part. */}
        <span
          aria-hidden
          className="absolute inset-y-0 left-0 transition-[width] duration-75"
          style={{ width: `${percent}%`, background: ZONES }}
        />
        {/* Peak tick. */}
        <span
          aria-hidden
          className="absolute inset-y-0 w-[2px] bg-white/85"
          style={{ left: `calc(${peakPercent}% - 1px)` }}
        />
        {/* Zone separators. */}
        <span aria-hidden className="absolute inset-y-0 left-[75%] w-px bg-black/50" />
        <span aria-hidden className="absolute inset-y-0 left-[92%] w-px bg-black/50" />
      </div>
      {showDb && (
        <span className="w-12 shrink-0 text-right font-mono text-[10px] text-ink-muted">
          {levelToDb(level)} dB
        </span>
      )}
    </div>
  );
}
