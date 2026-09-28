import clsx from "clsx";
import { levelToDb, levelToPercent, meterTone } from "../../lib/audio";

const tones = {
  ok: "bg-jade",
  warn: "bg-gold",
  hot: "bg-blood-bright",
} as const;

/**
 * OBS-like horizontal level meter with zones. `level` is the linear
 * amplitude (1.0 = full scale); it renders the current level plus a peak
 * tick so quiet signals still read visually.
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
  const tone = meterTone(percent);
  return (
    <div className={clsx("flex items-center gap-2", className)}>
      <div className="relative h-2.5 flex-1 overflow-hidden rounded-[2px] border border-ink-faint/40 bg-void/80">
        {/* Zone guides at 75% and 92%. */}
        <span aria-hidden className="absolute inset-y-0 left-[75%] w-px bg-paper/10" />
        <span aria-hidden className="absolute inset-y-0 left-[92%] w-px bg-paper/10" />
        <div
          className={clsx(
            "h-full rounded-[1px] transition-[width] duration-75",
            tones[tone],
          )}
          style={{ width: `${percent}%` }}
        />
        <span
          aria-hidden
          className="absolute inset-y-0 w-[2px] bg-paper/80"
          style={{ left: `calc(${peakPercent}% - 1px)` }}
        />
      </div>
      {showDb && (
        <span className="w-12 shrink-0 text-right font-mono text-[10px] text-ink-muted">
          {levelToDb(level)} dB
        </span>
      )}
    </div>
  );
}
