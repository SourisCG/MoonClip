import type { ReactNode } from "react";
import clsx from "clsx";

type Tone = "neutral" | "gold" | "blood" | "jade" | "aether" | "lava" | "sky" | "paper";

const tones: Record<Tone, string> = {
  neutral: "border-ink-faint/50 bg-void/60 text-ink-muted",
  gold: "border-gold/60 bg-gold/10 text-gold-bright",
  blood: "border-blood/60 bg-blood/10 text-blood-bright",
  jade: "border-jade/60 bg-jade/10 text-jade-bright",
  aether: "border-aether/60 bg-aether/10 text-aether-bright",
  lava: "border-lava/60 bg-lava/10 text-lava-bright",
  sky: "border-sky/60 bg-sky/10 text-sky-bright",
  paper: "border-ink bg-paper text-ink",
};

/** Stamped micro-label (uppercase mono), the fanzine badge. */
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
        "inline-flex items-center gap-1 rounded-stamp border px-1.5 py-0.5 font-mono text-[10px] uppercase leading-none tracking-[0.16em]",
        tones[tone],
        className,
      )}
    >
      {children}
    </span>
  );
}
