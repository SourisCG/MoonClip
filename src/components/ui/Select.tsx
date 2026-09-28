import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown } from "lucide-react";
import clsx from "clsx";

export interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

/**
 * Custom select (button + popover). Native `<select>` renders differently in
 * WebKitGTK and WebView2 (dropdown styling, hover colors), so every choice in
 * the app uses this component instead.
 */
export function Select({
  value,
  options,
  onChange,
  placeholder,
  disabled,
  className,
  buttonClassName,
  ariaLabel,
}: {
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
  className?: string;
  buttonClassName?: string;
  ariaLabel?: string;
}) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const current = options.find((option) => option.value === value);

  useEffect(() => {
    if (!open) return;
    const onPointer = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onPointer);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onPointer);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  useEffect(() => {
    if (open) {
      const index = options.findIndex((option) => option.value === value);
      setActive(index >= 0 ? index : 0);
    }
  }, [open, options, value]);

  const pick = (index: number) => {
    const option = options[index];
    if (!option || option.disabled) return;
    onChange(option.value);
    setOpen(false);
  };

  const move = (delta: number) => {
    if (!options.length) return;
    let next = active;
    for (let i = 0; i < options.length; i += 1) {
      next = (next + delta + options.length) % options.length;
      if (!options[next]?.disabled) break;
    }
    setActive(next);
  };

  return (
    <div ref={rootRef} className={clsx("relative", className)}>
      <button
        type="button"
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((v) => !v)}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            if (!open) setOpen(true);
            else move(event.key === "ArrowDown" ? 1 : -1);
          } else if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            if (open) pick(active);
            else setOpen(true);
          }
        }}
        className={clsx(
          "flex w-full items-center gap-2 rounded-control border border-line bg-black/60 px-2.5 py-1.5 text-left text-sm text-ink transition-colors",
          "hover:border-line-strong focus:border-link disabled:opacity-50",
          buttonClassName,
        )}
      >
        <span className={clsx("min-w-0 flex-1 truncate", !current && "text-ink-faint")}>
          {current?.label ?? placeholder ?? ""}
        </span>
        <ChevronDown size={14} className="shrink-0 text-ink-faint" />
      </button>
      {open && (
        <ul
          role="listbox"
          className="absolute z-30 mt-1 max-h-60 w-full overflow-y-auto rounded-control border border-line bg-[#1f1f20] py-1 shadow-pop"
        >
          {options.map((option, index) => (
            <li key={option.value}>
              <button
                type="button"
                role="option"
                aria-selected={option.value === value}
                disabled={option.disabled}
                onMouseEnter={() => setActive(index)}
                onClick={() => pick(index)}
                className={clsx(
                  "flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-sm transition-colors",
                  index === active ? "bg-[#2c2c30] text-ink" : "text-ink-soft",
                  option.disabled && "cursor-not-allowed opacity-40",
                )}
              >
                <span className="min-w-0 flex-1 truncate">{option.label}</span>
                {option.value === value && <Check size={13} className="shrink-0 text-link-bright" />}
              </button>
            </li>
          ))}
          {!options.length && (
            <li className="px-2.5 py-1.5 text-xs text-ink-faint">—</li>
          )}
        </ul>
      )}
    </div>
  );
}
