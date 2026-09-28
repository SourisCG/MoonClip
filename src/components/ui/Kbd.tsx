import type { ReactNode } from "react";
import clsx from "clsx";

export function Kbd({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <kbd
      className={clsx(
        "rounded-[3px] border border-ink-faint/60 bg-void/80 px-1.5 py-0.5 font-mono text-[10px] text-ink-soft shadow-[0_1px_0_rgba(0,0,0,0.6)]",
        className,
      )}
    >
      {children}
    </kbd>
  );
}
