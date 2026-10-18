// P1.2 WorkspaceHeader 的类名契约（spec: docs/specs/m4-ui-foundation.md §3）。

export function workspaceHeaderClassName(className?: string): string {
  return ["flex min-h-9 shrink-0 items-center gap-2 border-b border-border px-3 py-1.5", className]
    .filter(Boolean)
    .join(" ");
}

export function workspaceHeaderLeadingClassName(className?: string): string {
  return ["flex shrink-0 items-center gap-2", className].filter(Boolean).join(" ");
}

export function workspaceHeaderTitleClassName(className?: string): string {
  return ["min-w-0 truncate text-ui-base font-medium text-foreground", className]
    .filter(Boolean)
    .join(" ");
}

export function workspaceHeaderSubtitleClassName(className?: string): string {
  return ["min-w-0 truncate text-ui-sm text-foreground-subtle", className]
    .filter(Boolean)
    .join(" ");
}

export function workspaceHeaderActionsClassName(className?: string): string {
  return ["ms-auto flex shrink-0 items-center gap-1", className].filter(Boolean).join(" ");
}
