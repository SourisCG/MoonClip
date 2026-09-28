import type { ReactNode } from "react";
import clsx from "clsx";
import { Tag } from "./Tag";

/** Panel surface for the app: warm dark, hairline border, soft shadow. */
export function Card({
  className,
  children,
}: {
  className?: string;
  children: ReactNode;
}) {
  return (
    <div
      className={clsx(
        "rounded-card border border-line bg-panel/70 shadow-panel backdrop-blur-xl",
        className,
      )}
    >
      {children}
    </div>
  );
}

/** Section header with an optional stamped page tag. */
export function CardHeader({
  tag,
  title,
  actions,
  className,
}: {
  tag?: ReactNode;
  title: ReactNode;
  actions?: ReactNode;
  className?: string;
}) {
  return (
    <div className={clsx("flex items-center justify-between gap-3", className)}>
      <div className="min-w-0">
        {tag && <Tag tone="gold">{tag}</Tag>}
        <h3 className="truncate font-display text-base font-semibold text-ink">{title}</h3>
      </div>
      {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
    </div>
  );
}
