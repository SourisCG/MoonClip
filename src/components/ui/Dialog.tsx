import { useEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";
import type { ReactNode } from "react";
import clsx from "clsx";

import { pushOverlay } from "../../lib/overlay";
import { Button } from "./Button";

/**
 * Full dialog shell: portaled to the body (see Modal.tsx for why), Escape to
 * close, backdrop click to close, initial focus + focus restore.
 */
export function Dialog({
  onClose,
  title,
  subtitle,
  actions,
  children,
  className,
  panelClassName,
}: {
  onClose: () => void;
  title: ReactNode;
  subtitle?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
  panelClassName?: string;
}) {
  const panelRef = useRef<HTMLDivElement>(null);

  useEffect(() => pushOverlay(), []);

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    panelRef.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      previous?.focus?.();
    };
  }, [onClose]);

  return createPortal(
    <div
      className={clsx(
        "fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4 backdrop-blur-sm",
        className,
      )}
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={panelRef}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        className={clsx(
          "max-h-[calc(100vh-2rem)] w-full max-w-2xl overflow-y-auto rounded-card border border-line bg-surface shadow-panel outline-none",
          panelClassName,
        )}
      >
        <header className="flex items-center gap-3 border-b border-line bg-[#191919] px-4 py-3">
          <div className="min-w-0">
            <h2 className="truncate text-sm font-semibold text-ink">{title}</h2>
            {subtitle && <p className="mt-0.5 truncate text-xs text-ink-muted">{subtitle}</p>}
          </div>
          <div className="ml-auto flex shrink-0 items-center gap-2">
            {actions}
            <Button variant="ghost" size="icon-sm" onClick={onClose} aria-label="Close">
              <X size={15} />
            </Button>
          </div>
        </header>
        <div className="px-4 py-4">{children}</div>
      </div>
    </div>,
    document.body,
  );
}
