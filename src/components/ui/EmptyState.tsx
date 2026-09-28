import type { ReactNode } from "react";
import clsx from "clsx";

/** Paper "cutout" for empty/blank states, fanzine style. */
export function EmptyState({
  icon,
  title,
  hint,
  action,
  className,
}: {
  icon?: ReactNode;
  title: ReactNode;
  hint?: ReactNode;
  action?: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={clsx(
        "relative rounded-card border-2 border-ink bg-paper px-6 py-8 text-center text-ink shadow-stamp",
        className,
      )}
    >
      <span
        aria-hidden
        className="pointer-events-none absolute inset-1 rounded-[6px] border border-dashed border-ink/30"
      />
      {icon && <div className="mb-3 flex justify-center text-blood">{icon}</div>}
      <p className="font-display text-base font-semibold">{title}</p>
      {hint && <p className="mt-1 text-xs text-ink/70">{hint}</p>}
      {action && <div className="mt-4 flex justify-center">{action}</div>}
    </div>
  );
}
