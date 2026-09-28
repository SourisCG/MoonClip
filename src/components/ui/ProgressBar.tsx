import clsx from "clsx";

type Tone = "blood" | "gold" | "jade" | "sky";

const fills: Record<Tone, string> = {
  blood: "bg-[repeating-linear-gradient(45deg,#c92a2a_0_6px,#e04040_6px_12px)]",
  gold: "bg-[repeating-linear-gradient(45deg,#b58a3c_0_6px,#e8bc63_6px_12px)]",
  jade: "bg-jade",
  sky: "bg-sky",
};

/** Fanzine progress: ink-framed track with a hatched fill. */
export function ProgressBar({
  percent,
  tone = "blood",
  className,
}: {
  percent: number | null;
  tone?: Tone;
  className?: string;
}) {
  const clamped = percent === null ? null : Math.max(0, Math.min(100, percent));
  return (
    <div
      className={clsx(
        "h-2 w-full overflow-hidden rounded-[2px] border border-ink-faint/50 bg-void/70",
        className,
      )}
      role="progressbar"
      aria-valuenow={clamped ?? undefined}
      aria-valuemin={0}
      aria-valuemax={100}
    >
      <div
        className={clsx("h-full transition-[width] duration-200", fills[tone])}
        style={{ width: `${clamped ?? 5}%` }}
      />
    </div>
  );
}
