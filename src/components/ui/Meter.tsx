import clsx from "clsx";
import { levelToDb, levelToPercent } from "../../lib/audio";

/** OBS-like fixed zones on the -60..0 dB scale: green to -18 dB (70 %),
 *  amber to -6 dB (90 %), red above. */
const ZONES =
  "linear-gradient(to right, #3fb950 0 70%, #d29922 70% 90%, #ef4444 90% 100%)";

/**
 * OBS-style horizontal level meter. The green/amber/red scale is always
 * visible; the current level reveals it at full brightness while the unused
 * part stays dimmed, plus a peak tick so quiet signals still read.
 */
export function Meter({
  level,
  peak,
  showDb,
  showScale,
  className,
}: {
  level: number;
  peak?: number;
  showDb?: boolean;
  showScale?: boolean;
  className?: string;
}) {
  const percent = levelToPercent(level);
  const peakPercent = Math.max(percent, levelToPercent(peak ?? 0));
  return (
    <div className={clsx("flex flex-col gap-0.5", className)}>
      <div className="relative h-2.5 w-full overflow-hidden rounded-[3px] border border-line bg-black/70">
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
        <span aria-hidden className="absolute inset-y-0 left-[70%] w-px bg-black/50" />
        <span aria-hidden className="absolute inset-y-0 left-[90%] w-px bg-black/50" />
      </div>
      {showScale && (
        <div className="flex justify-between font-mono text-[9px] leading-none text-ink-faint">
          <span>-60</span>
          <span>-24</span>
          <span>-6</span>
          <span>0</span>
        </div>
      )}
      {showDb && (
        <span className="text-right font-mono text-[10px] text-ink-muted">
          {levelToDb(level)} dB
        </span>
      )}
    </div>
  );
}
