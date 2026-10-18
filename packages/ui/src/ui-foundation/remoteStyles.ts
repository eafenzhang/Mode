// P1.6 远程与开发者面板的类名契约
// （plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.6）。
// 只做静态容器；媒体流/重连/剪贴板契约沿用既有 remote-desktop spec。

export function remoteLayoutClassName(className?: string): string {
  return ["flex min-h-0 flex-1 flex-col gap-2", className].filter(Boolean).join(" ");
}

export function remoteSurfaceClassName(className?: string): string {
  return [
    "relative flex min-h-0 flex-1 items-center justify-center overflow-hidden rounded-xl bg-background",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function remoteCanvasClassName(className?: string): string {
  return ["relative max-h-full max-w-full", className].filter(Boolean).join(" ");
}

export function remoteToolbarClassName(className?: string): string {
  return [
    "flex shrink-0 items-center gap-1 rounded-xl border border-border bg-panel px-2 py-1",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export interface RemoteToolbarButtonClassNameOptions {
  active?: boolean;
  className?: string;
}

export function remoteToolbarButtonClassName(
  options: RemoteToolbarButtonClassNameOptions = {},
): string {
  return [
    "inline-flex size-7 items-center justify-center rounded-md text-foreground-subtle",
    "hover:bg-hover hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary",
    "disabled:pointer-events-none disabled:opacity-50",
    options.active && "bg-selected text-foreground",
    options.className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function remoteStatusStripClassName(className?: string): string {
  return [
    "flex shrink-0 items-center gap-3 rounded-xl border border-border bg-panel px-3 py-1 text-ui-xs text-foreground-subtle",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function remoteStatusItemClassName(className?: string): string {
  return ["inline-flex items-center gap-1", className].filter(Boolean).join(" ");
}

/** 断流覆盖：不透出底层画面，也不引入无 token 的透明黑。 */
export function remoteOfflineOverlayClassName(className?: string): string {
  return [
    "absolute inset-0 flex flex-col items-center justify-center gap-2 bg-background text-center text-ui-sm text-foreground-subtle",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function terminalPaneClassName(className?: string): string {
  return [
    "flex min-h-0 flex-1 flex-col overflow-y-auto bg-panel px-3 py-2 font-mono text-ui-sm text-foreground",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function previewPaneClassName(className?: string): string {
  return [
    "flex min-h-0 flex-1 flex-col overflow-hidden rounded-xl border border-border bg-surface",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function previewHeaderClassName(className?: string): string {
  return [
    "flex h-8 shrink-0 items-center gap-2 border-b border-border px-2 text-ui-sm text-foreground-subtle",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}
