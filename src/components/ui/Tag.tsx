import type { ReactNode } from "react";
import clsx from "clsx";

type Tone = "neutral" | "brand" | "link" | "ok" | "warn" | "aqua" | "edit" | "paper";

const tones: Record<Tone, string> = {
  neutral: "border-line bg-raised text-ink-soft",
  brand: "border-brand/50 bg-brand/15 text-brand-bright",
  link: "border-link/50 bg-link/15 text-link-bright",
  ok: "border-ok/50 bg-ok/15 text-ok-bright",
  warn: "border-warn/50 bg-warn/15 text-warn-bright",
  aqua: "border-aqua/50 bg-aqua/15 text-aqua-bright",
  edit: "border-edit/50 bg-edit/15 text-edit-bright",
  paper: "border-line bg-raised text-ink-soft",
};

/** Compact uppercase mono chip; the only "stamp" detail that stays. */
export function Tag({
  tone = "neutral",
  className,
  children,
}: {
  tone?: Tone;
  className?: string;
  children: ReactNode;
}) {
  return (
    <span
      className={clsx(
        "inline-flex items-center gap-1 rounded-[5px] border px-1.5 py-0.5 font-mono text-[10px] uppercase leading-none tracking-[0.14em]",
        tones[tone],
        className,
      )}
    >
      {children}
    </span>
  );
}
