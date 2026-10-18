// P1.1 基础件补漏：Checkbox / Switch 的类名契约（adapter map §1 缺口）。

export function checkboxClassName(className?: string): string {
  return [
    "size-4 shrink-0 rounded-sm border border-input-border bg-input",
    "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary",
    "disabled:cursor-not-allowed disabled:opacity-50",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function checkboxRowClassName(className?: string): string {
  return ["flex items-center gap-2 text-ui-sm text-foreground", className]
    .filter(Boolean)
    .join(" ");
}

export function switchTrackClassName(checked: boolean, className?: string): string {
  return [
    "relative inline-flex h-5 w-9 shrink-0 items-center rounded-full border border-border transition-colors",
    checked ? "bg-primary" : "bg-input",
    "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary",
    "disabled:cursor-not-allowed disabled:opacity-50",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function switchThumbClassName(checked: boolean, className?: string): string {
  return [
    "size-4 rounded-full bg-background shadow-sm transition-transform",
    checked ? "translate-x-4" : "translate-x-0.5",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}
