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
        "rounded-[5px] border border-line bg-raised px-1.5 py-0.5 font-mono text-[10px] text-ink-soft",
        className,
      )}
    >
      {children}
    </kbd>
  );
}
