import type {
  InputHTMLAttributes,
  ReactNode,
  SelectHTMLAttributes,
} from "react";
import clsx from "clsx";

export const fieldClass =
  "w-full rounded-control border border-line bg-black/60 px-2.5 py-1.5 text-sm text-ink placeholder:text-ink-faint outline-none transition-colors focus:border-link";

export function Input({
  className,
  ...rest
}: InputHTMLAttributes<HTMLInputElement>) {
  return <input className={clsx(fieldClass, className)} {...rest} />;
}

/** Native fallback select (custom Select lives in ./Select.tsx). */
export function Select({
  className,
  children,
  ...rest
}: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select className={clsx(fieldClass, "cursor-pointer", className)} {...rest}>
      {children}
    </select>
  );
}

/** Label + control wrapper used across Settings. */
export function Field({
  label,
  hint,
  children,
  className,
}: {
  label: ReactNode;
  hint?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <label className={clsx("block", className)}>
      <span className="mb-1.5 block text-xs font-medium text-ink-muted">{label}</span>
      {children}
      {hint && <span className="mt-1 block text-[11px] text-ink-faint">{hint}</span>}
    </label>
  );
}

/** Checkbox drawn by us so both webviews match. */
export function Checkbox({
  className,
  ...rest
}: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      type="checkbox"
      className={clsx(
        "mt-0.5 h-4 w-4 shrink-0 cursor-pointer appearance-none rounded-[4px] border border-line-strong bg-black/60 transition-colors",
        "checked:border-link checked:bg-link checked:bg-[url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 12 12'%3E%3Cpath d='M2.5 6.2 5 8.6 9.5 3.6' fill='none' stroke='white' stroke-width='1.8' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E\")] checked:bg-[length:10px_10px] checked:bg-center checked:bg-no-repeat",
        className,
      )}
      {...rest}
    />
  );
}
