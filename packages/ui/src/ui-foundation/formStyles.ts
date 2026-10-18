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

export function radioGroupClassName(className?: string): string {
  return ["flex flex-col gap-2", className].filter(Boolean).join(" ");
}

export function radioRowClassName(className?: string): string {
  return ["flex items-center gap-2 text-ui-sm text-foreground", className]
    .filter(Boolean)
    .join(" ");
}

export function radioClassName(className?: string): string {
  return [
    "size-4 shrink-0 rounded-full border border-input-border bg-input",
    "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary",
    "disabled:cursor-not-allowed disabled:opacity-50",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function progressTrackClassName(className?: string): string {
  return ["h-1.5 w-full overflow-hidden rounded-full bg-surface", className]
    .filter(Boolean)
    .join(" ");
}

export function progressIndicatorClassName(className?: string): string {
  return ["h-full rounded-full bg-primary transition-all", className]
    .filter(Boolean)
    .join(" ");
}

/** 进度百分比：非法值归 0，超界 clamp，避免除零。 */
export function progressPercent(value: number, max = 100): number {
  if (!Number.isFinite(value) || !Number.isFinite(max) || max <= 0) {
    return 0;
  }
  return Math.min(100, Math.max(0, (value / max) * 100));
}
