// P1.2 工作区外壳的类名契约（spec: docs/specs/m4-ui-foundation.md §3）。
// 只使用 DESIGN.md 的语义 token；布局 frame 不计入内容半径层级。

export function workspaceShellClassName(className?: string): string {
  return ["flex h-full min-h-0 w-full gap-1 bg-background", className].filter(Boolean).join(" ");
}

export function conversationColumnClassName(className?: string): string {
  return ["flex min-h-0 min-w-0 flex-1 flex-col gap-1", className].filter(Boolean).join(" ");
}

export function conversationFrameClassName(className?: string): string {
  return [
    "flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-xl border border-border bg-panel",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function terminalFrameClassName(className?: string): string {
  return [
    "flex h-56 min-h-0 flex-col overflow-hidden rounded-xl border border-border bg-panel",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function sidePaneFrameClassName(className?: string): string {
  return [
    "flex min-h-0 w-80 shrink-0 flex-col overflow-hidden rounded-xl border border-border bg-panel",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export type ResizeOrientation = "vertical" | "horizontal";

/**
 * 4px 透明命中区；hover/focus/drag 时显示 2px `foreground-subtlest/50` 线，
 * 线在两端按 frame 圆角内缩（DESIGN.md「Workspace layout」）。
 */
export function resizeHandleClassName(orientation: ResizeOrientation, className?: string): string {
  const axis =
    orientation === "vertical"
      ? "w-1 cursor-col-resize after:inset-y-2 after:left-1/2 after:w-0.5 after:-translate-x-1/2"
      : "h-1 cursor-row-resize after:inset-x-2 after:top-1/2 after:h-0.5 after:-translate-y-1/2";
  return [
    "relative shrink-0 bg-transparent outline-none",
    "after:absolute after:rounded-full after:bg-foreground-subtlest/50 after:opacity-0",
    "hover:after:opacity-100 focus-visible:after:opacity-100 data-[dragging=true]:after:opacity-100",
    axis,
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

/** 手机远控：单列 + drawer；宽度/高度由调用方约束。 */
export function mobileDrawerClassName(className?: string): string {
  return [
    "fixed inset-x-0 bottom-0 z-50 overflow-y-auto rounded-t-2xl border-t border-border bg-popover",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}
