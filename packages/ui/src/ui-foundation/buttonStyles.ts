// P1.1 Button 的类名纯函数（spec: docs/specs/m4-ui-foundation.md §2.1）。
// 只从 DESIGN.md 的语义 token 与尺寸表出发；不引用旧 button 实现。

import type { RadiusLevel } from "../lib/uiFoundation.js";
import { controlRadiusClass } from "./radius.js";

export type ButtonVariant =
  | "primary"
  | "outline"
  | "secondary"
  | "ghost"
  | "destructive"
  | "link";

export type ButtonSize = "xs" | "sm" | "default" | "lg" | "icon-xs" | "icon-sm" | "icon" | "icon-lg";

const VARIANT_CLASSES: Record<ButtonVariant, string> = {
  primary: "bg-primary text-primary-foreground hover:bg-primary/90",
  outline: "border border-border bg-transparent text-foreground hover:bg-hover",
  secondary: "bg-secondary text-foreground hover:bg-hover",
  ghost: "bg-transparent text-foreground hover:bg-hover",
  destructive: "bg-destructive text-destructive-foreground hover:bg-destructive/90",
  link: "bg-transparent text-primary underline-offset-4 hover:underline",
};

const SIZE_CLASSES: Record<ButtonSize, string> = {
  xs: "h-6 px-2 text-ui-xs gap-1",
  sm: "h-7 px-2.5 text-ui-sm gap-1.5",
  default: "h-8 px-3 text-ui-base gap-2",
  lg: "h-9 px-4 text-ui-base gap-2",
  "icon-xs": "size-6 text-ui-xs",
  "icon-sm": "size-7 text-ui-sm",
  icon: "size-8 text-ui-base",
  "icon-lg": "size-9 text-ui-base",
};

export interface ButtonClassNameOptions {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** 最近圆角父容器；决定控件半径（spec 的控件表）。 */
  parentRadius?: RadiusLevel;
  className?: string;
}

export function buttonClassName(options: ButtonClassNameOptions = {}): string {
  const variant = options.variant ?? "primary";
  const size = options.size ?? "default";
  const radius = controlRadiusClass(options.parentRadius);
  return [
    "inline-flex shrink-0 items-center justify-center whitespace-nowrap font-medium",
    "transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary",
    "disabled:pointer-events-none disabled:opacity-50",
    VARIANT_CLASSES[variant],
    SIZE_CLASSES[size],
    radius,
    options.className,
  ]
    .filter(Boolean)
    .join(" ");
}
