import { cn } from "@/components/lib/utils.js";

// P1.7 快捷键清单的类名契约（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.7）。

export function shortcutListClassName(className?: string): string {
  return cn(["flex flex-col gap-2", className]);
}

export function shortcutRowClassName(className?: string): string {
  return cn(["flex items-center justify-between gap-3 text-ui-sm", className]);
}

export function shortcutLabelClassName(className?: string): string {
  return cn(["min-w-0 truncate text-foreground", className]);
}

export function shortcutKeysClassName(className?: string): string {
  return cn(["flex shrink-0 items-center gap-1", className]);
}

export function shortcutKeyClassName(className?: string): string {
  return cn([
    "rounded-sm border border-border bg-surface px-1 font-mono text-ui-xs text-foreground-subtle",
    className,
  ]);
}
