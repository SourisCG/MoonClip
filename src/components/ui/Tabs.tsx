import type { ReactNode } from "react";
import clsx from "clsx";

/** Fanzine tab row: numbered stamps, one active. */
export function Tabs({
  className,
  children,
}: {
  className?: string;
  children: ReactNode;
}) {
  return (
    <div role="tablist" className={clsx("flex flex-wrap items-center gap-1.5", className)}>
      {children}
    </div>
  );
}

export function Tab({
  active,
  num,
  icon,
  onClick,
  children,
  className,
}: {
  active?: boolean;
  num?: string;
  icon?: ReactNode;
  onClick?: () => void;
  children: ReactNode;
  className?: string;
}) {
  return (
    <button
      role="tab"
      aria-selected={active}
      type="button"
      onClick={onClick}
      className={clsx(
        "inline-flex items-center gap-2 rounded-stamp border px-2.5 py-1.5 font-mono text-[11px] font-semibold uppercase tracking-[0.1em] transition-all duration-150",
        active
          ? "-translate-y-0.5 border-ink bg-paper text-ink shadow-stamp-blood"
          : "border-line bg-raised/60 text-ink-muted hover:border-jade/50 hover:text-ink",
        className,
      )}
    >
      {num && (
        <span
          className={clsx(
            "rounded-[2px] px-1 py-px text-[10px]",
            active ? "bg-blood text-paper" : "bg-void text-ink-faint",
          )}
        >
          {num}
        </span>
      )}
      {icon}
      {children}
    </button>
  );
}
