import type { ReactNode } from "react";
import clsx from "clsx";

/**
 * Medal-like shell: fixed icon rail (sections) + contextual rail (filters or
 * sub-navigation) + header + scrollable main. No nested cards: every region
 * is edge-to-edge with a single hairline border.
 */

export interface RailItem {
  id: string;
  icon: ReactNode;
  label: string;
}

export function IconRail({
  brand,
  items,
  active,
  onSelect,
  children,
}: {
  brand: ReactNode;
  items: RailItem[];
  active: string;
  onSelect: (id: string) => void;
  /** Bottom block: record state, start/stop, language. */
  children?: ReactNode;
}) {
  return (
    <nav className="flex w-[72px] shrink-0 flex-col items-center gap-1 border-r border-line bg-surface py-3">
      <div className="mb-2 flex h-10 w-10 items-center justify-center">{brand}</div>
      {items.map((item) => (
        <button
          key={item.id}
          onClick={() => onSelect(item.id)}
          title={item.label}
          className={clsx(
            "group relative flex h-12 w-12 flex-col items-center justify-center gap-0.5 rounded-control text-[10px] font-medium transition-colors",
            active === item.id
              ? "bg-raised text-ink"
              : "text-ink-muted hover:bg-raised/60 hover:text-ink",
          )}
        >
          {active === item.id && (
            <span className="absolute -left-2 top-1/2 h-5 w-0.5 -translate-y-1/2 rounded-full bg-brand" />
          )}
          {item.icon}
          <span className="max-w-full truncate px-0.5">{item.label}</span>
        </button>
      ))}
      <div className="mt-auto flex flex-col items-center gap-2 pt-2">{children}</div>
    </nav>
  );
}

export function ContextRail({
  title,
  children,
  className,
}: {
  title?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <aside
      className={clsx(
        "flex w-[224px] shrink-0 flex-col overflow-y-auto border-r border-line bg-surface px-2 py-3",
        className,
      )}
    >
      {title && (
        <p className="mb-2 px-2 font-mono text-[10px] uppercase tracking-[0.18em] text-ink-faint">
          {title}
        </p>
      )}
      <div className="space-y-0.5">{children}</div>
    </aside>
  );
}

export function RailButton({
  active,
  icon,
  label,
  count,
  danger,
  onClick,
  className,
}: {
  active?: boolean;
  icon?: ReactNode;
  label: ReactNode;
  count?: number;
  danger?: boolean;
  onClick?: () => void;
  className?: string;
}) {
  return (
    <button
      onClick={onClick}
      className={clsx(
        "flex w-full items-center gap-2 rounded-control px-2 py-1.5 text-left text-[13px] transition-colors",
        active
          ? "bg-raised text-ink"
          : danger
            ? "text-brand-bright hover:bg-raised/60"
            : "text-ink-muted hover:bg-raised/60 hover:text-ink",
        className,
      )}
    >
      {icon && <span className="shrink-0 text-ink-faint">{icon}</span>}
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {count !== undefined && (
        <span className="shrink-0 font-mono text-[10px] text-ink-faint">{count}</span>
      )}
    </button>
  );
}

export function ShellHeader({
  children,
  actions,
}: {
  children?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <header className="flex h-12 shrink-0 items-center gap-3 border-b border-line bg-base px-4">
      <div className="flex min-w-0 flex-1 items-center gap-3">{children}</div>
      {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
    </header>
  );
}

export function ShellMain({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <main className={clsx("min-h-0 flex-1 overflow-y-auto", className)}>{children}</main>
  );
}
