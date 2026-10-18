// P1.1 Tabs / Tooltip / Toast 的类名纯函数
// （spec: docs/specs/m4-ui-foundation.md §2.5/§2.6）。

export function tabListClassName(className?: string): string {
  return ["inline-flex items-center gap-0.5", className].filter(Boolean).join(" ");
}

export interface TabClassNameOptions {
  selected: boolean;
  className?: string;
}

export function tabClassName(options: TabClassNameOptions): string {
  return [
    "inline-flex h-7 items-center gap-1.5 rounded-md px-2.5 text-ui-sm font-medium transition-colors",
    "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary",
    options.selected
      ? "bg-selected text-foreground"
      : "text-foreground-subtle hover:bg-hover hover:text-foreground",
    options.className,
  ]
    .filter(Boolean)
    .join(" ");
}

/** Tooltip 在交互浮层（menu/popover/dialog = z-50）之下。 */
export function tooltipClassName(className?: string): string {
  return [
    "z-40 inline-flex items-center gap-1.5 pointer-events-none max-w-xs rounded-md border border-popover-border",
    "bg-tooltip px-2 py-1 text-ui-sm text-tooltip-foreground shadow-md",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function tooltipShortcutClassName(className?: string): string {
  return ["rounded-sm bg-tooltip-tag px-1 text-ui-xs text-tooltip-tag-foreground", className]
    .filter(Boolean)
    .join(" ");
}

export interface ToastClassNameOptions {
  destructive?: boolean;
  className?: string;
}

export function toastClassName(options: ToastClassNameOptions = {}): string {
  return [
    "z-50 rounded-2xl bg-toast px-4 py-3 text-ui-sm text-foreground shadow-lg",
    options.destructive && "border border-destructive",
    options.className,
  ]
    .filter(Boolean)
    .join(" ");
}
