import type { ReactNode } from "react";
import clsx from "clsx";

/** Segmented control (Medal-like pill row). */
export function Tabs({
  className,
  children,
}: {
  className?: string;
  children: ReactNode;
}) {
  return (
    <div
      role="tablist"
      className={clsx(
        "inline-flex flex-wrap items-center gap-1 rounded-control border border-line bg-raised/60 p-1",
        className,
      )}
    >
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
        "inline-flex items-center gap-2 rounded-[6px] px-2.5 py-1.5 text-xs font-medium transition-colors",
        active
          ? "bg-[#2c2c30] text-ink shadow-[inset_0_1px_0_rgba(255,255,255,0.06)]"
          : "text-ink-muted hover:bg-raised hover:text-ink",
        className,
      )}
    >
      {num && (
        <span
          className={clsx(
            "rounded-[4px] px-1 py-px font-mono text-[10px]",
            active ? "bg-brand/20 text-brand-bright" : "bg-raised text-ink-faint",
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
