// P1.1 Card / Menu / Popover / Dialog 的类名纯函数
// （spec: docs/specs/m4-ui-foundation.md §2.3/§2.4）。只使用 DESIGN.md 的语义 token。

import { resolveNestedRadius, type RadiusLevel } from "../lib/uiFoundation.js";

const NESTED_RADIUS_CLASSES: Record<RadiusLevel, string> = {
  none: "rounded-xl",
  sm: "rounded-sm",
  md: "rounded-md",
  lg: "rounded-lg",
  xl: "rounded-xl",
  "2xl": "rounded-xl",
};

/** 容器嵌套半径：首层 xl；Dialog shell（2xl）不计层，内容重新从 xl 起。 */
export function containerRadiusClass(parentRadius: RadiusLevel = "none"): string {
  return NESTED_RADIUS_CLASSES[resolveNestedRadius(parentRadius)];
}

export interface CardClassNameOptions {
  parentRadius?: RadiusLevel;
  /** 低强调容器用 bg-surface（默认 bg-card）。 */
  lowEmphasis?: boolean;
  selected?: boolean;
  /** 主内容卡默认 16px 内边距；紧凑卡可关掉。 */
  padded?: boolean;
  className?: string;
}

export function cardClassName(options: CardClassNameOptions = {}): string {
  return [
    "border border-card-border",
    options.lowEmphasis ? "bg-surface" : "bg-card",
    options.selected && "bg-card-selected",
    containerRadiusClass(options.parentRadius),
    options.padded === false ? null : "p-4",
    options.className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function cardHeaderClassName(className?: string): string {
  return ["flex items-start justify-between gap-2", className].filter(Boolean).join(" ");
}

export function cardTitleClassName(className?: string): string {
  return ["text-ui-base font-medium text-foreground", className].filter(Boolean).join(" ");
}

export function cardDescriptionClassName(className?: string): string {
  return ["text-ui-sm text-foreground-subtle", className].filter(Boolean).join(" ");
}

export function cardContentClassName(className?: string): string {
  return ["min-w-0", className].filter(Boolean).join(" ");
}

export function cardFooterClassName(className?: string): string {
  return ["flex items-center gap-2", className].filter(Boolean).join(" ");
}

export function cardActionClassName(className?: string): string {
  return ["shrink-0", className].filter(Boolean).join(" ");
}

/** 菜单 shell：bg-menu + rounded-lg + shadow-md + p-1；交互浮层层级高于 Tooltip。 */
export function menuClassName(className?: string): string {
  return [
    "z-50 bg-menu border border-popover-border rounded-lg shadow-md p-1",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export interface MenuItemClassNameOptions {
  destructive?: boolean;
  className?: string;
}

export function menuItemClassName(options: MenuItemClassNameOptions = {}): string {
  return [
    "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-ui-base",
    options.destructive ? "text-destructive" : "text-foreground",
    "hover:bg-menu-hover focus:bg-menu-hover focus:outline-none",
    "disabled:pointer-events-none disabled:opacity-50",
    options.className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function menuGroupClassName(className?: string): string {
  return ["flex flex-col gap-0.5", className].filter(Boolean).join(" ");
}

/** 独立浮层：bg-popover + 从 xl 起的嵌套半径；位于 Tooltip 之上。 */
export function popoverClassName(parentRadius: RadiusLevel = "none", className?: string): string {
  return [
    "z-50 bg-popover border border-popover-border shadow-md",
    containerRadiusClass(parentRadius),
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

/** Dialog shell 固定 rounded-2xl；内部内容重新从 xl 起；位于 Tooltip 之上。 */
export function dialogClassName(className?: string): string {
  return [
    "z-50 bg-popover border border-popover-border rounded-2xl shadow-md",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function dialogHeaderClassName(className?: string): string {
  return ["flex flex-col gap-1.5", className].filter(Boolean).join(" ");
}

export function dialogTitleClassName(className?: string): string {
  return ["text-ui-lg font-medium text-foreground", className].filter(Boolean).join(" ");
}

export function dialogDescriptionClassName(className?: string): string {
  return ["text-ui-sm text-foreground-subtle", className].filter(Boolean).join(" ");
}

export function dialogFooterClassName(className?: string): string {
  return ["flex items-center justify-end gap-2 pt-2", className].filter(Boolean).join(" ");
}
