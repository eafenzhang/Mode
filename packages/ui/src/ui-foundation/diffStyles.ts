// P1.3 diff 与排队输入的类名契约
// （plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.3）。
// Diff 必须使用 diff 专用语义色，不能借用 success/destructive（DESIGN.md）。

export type DiffLineKind = "added" | "removed" | "context";

export function diffBlockClassName(className?: string): string {
  return [
    "overflow-hidden rounded-xl border border-border bg-surface font-mono text-ui-sm",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function diffHeaderClassName(className?: string): string {
  return [
    "flex items-center gap-2 border-b border-border px-3 py-1.5 text-ui-sm text-foreground-subtle",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function diffFilePathClassName(className?: string): string {
  return ["min-w-0 truncate font-mono text-ui-sm text-foreground", className]
    .filter(Boolean)
    .join(" ");
}

export function diffStatsClassName(className?: string): string {
  return ["ms-auto flex shrink-0 items-center gap-2 text-ui-xs", className]
    .filter(Boolean)
    .join(" ");
}

const DIFF_LINE_CLASSES: Record<DiffLineKind, string> = {
  added: "bg-diff-added text-diff-added-foreground",
  removed: "bg-diff-removed text-diff-removed-foreground",
  context: "text-foreground",
};

export function diffLineClassName(kind: DiffLineKind, className?: string): string {
  return ["flex gap-2 px-3 py-0.5", DIFF_LINE_CLASSES[kind], className].filter(Boolean).join(" ");
}

/** 行首标记：+ / − / 空白；由 aria-hidden 渲染，读屏依赖行语义而非符号。 */
export function diffMarker(kind: DiffLineKind): string {
  return kind === "added" ? "+" : kind === "removed" ? "-" : " ";
}

export function queuedInputListClassName(className?: string): string {
  return ["flex flex-col gap-1.5", className].filter(Boolean).join(" ");
}

export function queuedInputItemClassName(className?: string): string {
  return [
    "flex items-start gap-2 rounded-lg border border-border bg-surface px-2.5 py-1.5 text-ui-sm text-foreground-subtle",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function queuedInputTextClassName(className?: string): string {
  return ["min-w-0 flex-1 truncate", className].filter(Boolean).join(" ");
}

export function queuedInputActionsClassName(className?: string): string {
  return ["ms-auto flex shrink-0 items-center gap-1", className].filter(Boolean).join(" ");
}
