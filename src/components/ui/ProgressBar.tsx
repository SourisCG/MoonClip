import clsx from "clsx";

type Tone = "brand" | "link" | "ok" | "warn";

const fills: Record<Tone, string> = {
  brand: "bg-brand",
  link: "bg-link",
  ok: "bg-ok",
  warn: "bg-warn",
};

export function ProgressBar({
  percent,
  tone = "brand",
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
        "h-1.5 w-full overflow-hidden rounded-full bg-raised",
        className,
      )}
      role="progressbar"
      aria-valuenow={clamped ?? undefined}
      aria-valuemin={0}
      aria-valuemax={100}
    >
      <div
        className={clsx("h-full rounded-full transition-[width] duration-200", fills[tone])}
        style={{ width: `${clamped ?? 5}%` }}
      />
    </div>
  );
}
