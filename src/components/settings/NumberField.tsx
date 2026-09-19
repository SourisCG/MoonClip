import { useEffect, useRef, useState } from "react";

interface NumberFieldProps {
  /** Committed value from the parent (`undefined` = empty). */
  value: number | undefined;
  min: number;
  max: number;
  step?: number;
  /** Normalize down to even on commit (output resolutions). */
  even?: boolean;
  disabled?: boolean;
  className?: string;
  ariaLabel?: string;
  placeholder?: string;
  onCommit: (v: number) => void;
}

/**
 * Numeric input you can actually type in: while focused it keeps a local
 * string draft (empty/partial values allowed, nothing committed), and on
 * blur/Enter it parses, clamps and normalizes, reverting on garbage.
 * External value changes while not editing reset the draft.
 */
export function NumberField({
  value,
  min,
  max,
  step = 1,
  even = false,
  disabled,
  className,
  ariaLabel,
  placeholder,
  onCommit,
}: NumberFieldProps) {
  const [draft, setDraft] = useState<string | null>(null);
  const focused = useRef(false);

  useEffect(() => {
    if (!focused.current) setDraft(null);
  }, [value]);

  const shown = draft ?? (value === undefined ? "" : String(value));

  const commit = (raw: string) => {
    const n = Number(raw);
    if (raw.trim() === "" || !Number.isFinite(n)) {
      setDraft(null);
      return;
    }
    let v = Math.round(n);
    v = Math.min(max, Math.max(min, v));
    if (even) v -= ((v % 2) + 2) % 2 === 0 ? 0 : 1;
    onCommit(v);
    setDraft(null);
  };

  return (
    <input
      type="number"
      min={min}
      max={max}
      step={step}
      value={shown}
      disabled={disabled}
      aria-label={ariaLabel}
      placeholder={placeholder}
      className={className}
      onFocus={() => {
        focused.current = true;
        setDraft(value === undefined ? "" : String(value));
      }}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={(e) => {
        focused.current = false;
        commit(e.target.value);
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
      }}
    />
  );
}
