import type { ButtonHTMLAttributes, ReactNode } from "react";
import clsx from "clsx";

type Variant = "primary" | "paper" | "ghost" | "subtle" | "danger";
type Size = "sm" | "md" | "icon" | "icon-sm";

interface Props extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
  children?: ReactNode;
}

const variants: Record<Variant, string> = {
  // Blood stamp: the primary action (matches the logo red).
  primary:
    "border-blood bg-blood text-paper hover:bg-blood-bright hover:border-ink hover:-translate-x-px hover:-translate-y-px hover:shadow-stamp",
  paper:
    "border-ink bg-paper text-ink hover:bg-paper-soft hover:-translate-x-px hover:-translate-y-px hover:shadow-stamp",
  ghost:
    "border-dashed border-ink-faint/70 bg-transparent text-ink-muted hover:bg-paper/5 hover:text-ink",
  subtle:
    "border-line bg-raised/70 text-ink-soft hover:border-gold/40 hover:text-ink",
  danger:
    "border-blood/50 bg-blood/10 text-blood-bright hover:bg-blood/20 hover:border-blood",
};

const sizes: Record<Size, string> = {
  sm: "px-2.5 py-1 text-xs gap-1.5",
  md: "px-3.5 py-1.5 text-sm gap-2",
  icon: "p-2",
  "icon-sm": "p-1.5",
};

export function Button({
  variant = "subtle",
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
        "inline-flex items-center justify-center rounded-stamp border font-medium tracking-wide transition-all duration-150",
        "disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:translate-x-0 disabled:hover:translate-y-0 disabled:hover:shadow-none",
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
