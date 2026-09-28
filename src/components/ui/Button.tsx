import type { ButtonHTMLAttributes, ReactNode } from "react";
import clsx from "clsx";

type Variant = "primary" | "secondary" | "ghost" | "danger" | "paper";
type Size = "sm" | "md" | "icon" | "icon-sm";

interface Props extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
  children?: ReactNode;
}

const variants: Record<Variant, string> = {
  // Red = the logo action color.
  primary:
    "border-transparent bg-brand text-white hover:bg-brand-bright active:bg-brand",
  secondary:
    "border-line bg-raised text-ink-soft hover:border-line-strong hover:bg-[#26262a] hover:text-ink",
  ghost:
    "border-transparent bg-transparent text-ink-muted hover:bg-raised hover:text-ink",
  danger:
    "border-brand/50 bg-transparent text-brand-bright hover:bg-brand/15 hover:border-brand",
  // Compatibility alias (old panels): same as secondary.
  paper:
    "border-line bg-raised text-ink-soft hover:border-line-strong hover:bg-[#26262a] hover:text-ink",
};

const sizes: Record<Size, string> = {
  sm: "h-8 px-3 text-xs gap-1.5",
  md: "h-9 px-3.5 text-sm gap-2",
  icon: "h-9 w-9",
  "icon-sm": "h-7 w-7",
};

export function Button({
  variant = "secondary",
  size = "sm",
  className,
  children,
  type = "button",
  ...rest
}: Props) {
  return (
    <button
      type={type}
      className={clsx(
        "inline-flex items-center justify-center rounded-control font-medium transition-colors duration-150",
        "disabled:cursor-not-allowed disabled:opacity-40",
        variants[variant],
        sizes[size],
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
}
