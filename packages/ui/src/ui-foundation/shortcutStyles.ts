// P1.7 快捷键清单的类名契约（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.7）。

export function shortcutListClassName(className?: string): string {
  return ["flex flex-col gap-2", className].filter(Boolean).join(" ");
}

export function shortcutRowClassName(className?: string): string {
  return ["flex items-center justify-between gap-3 text-ui-sm", className]
    .filter(Boolean)
    .join(" ");
}

export function shortcutLabelClassName(className?: string): string {
  return ["min-w-0 truncate text-foreground", className].filter(Boolean).join(" ");
}

export function shortcutKeysClassName(className?: string): string {
  return ["flex shrink-0 items-center gap-1", className].filter(Boolean).join(" ");
}

export function shortcutKeyClassName(className?: string): string {
  return [
    "rounded-sm border border-border bg-surface px-1 font-mono text-ui-xs text-foreground-subtle",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}
