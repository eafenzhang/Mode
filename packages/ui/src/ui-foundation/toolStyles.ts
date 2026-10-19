import { cn } from "@/components/lib/utils.js";

// P1.3 工具调用与审批块的类名契约
// （plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.3；事件状态见 tauri-agent-runtime spec）。

export type ToolBlockState = "scheduled" | "started" | "progress" | "result" | "error";

const TOOL_STATE_CLASSES: Record<ToolBlockState, string> = {
  scheduled: "border-dashed border-border bg-surface text-foreground-subtle",
  started: "border-border bg-surface text-foreground",
  progress: "border-border bg-surface text-foreground",
  result: "border-border bg-surface text-foreground",
  error: "border-destructive bg-surface text-destructive",
};

export function toolBlockClassName(state: ToolBlockState, className?: string): string {
  return cn(["rounded-xl border px-3 py-2 text-ui-sm", TOOL_STATE_CLASSES[state], className]);
}

export function toolHeaderClassName(className?: string): string {
  return cn(["flex min-w-0 items-center gap-2", className]);
}

export function toolNameClassName(className?: string): string {
  return cn(["min-w-0 truncate font-mono text-ui-sm text-foreground", className]);
}

export function toolStatusClassName(state: ToolBlockState, className?: string): string {
  return cn([
    "shrink-0 text-ui-xs",
    state === "error" ? "text-destructive" : "text-foreground-subtlest",
    className,
  ]);
}

/** 工具正文：技术内容用 mono；结果可能很长，保留换行并允许断词。 */
export function toolBodyClassName(className?: string): string {
  return cn([
    "mt-2 whitespace-pre-wrap break-words font-mono text-ui-sm text-foreground",
    className,
  ]);
}

export type ApprovalStatus = "pending" | "allowed" | "denied";

const APPROVAL_STATUS_CLASSES: Record<ApprovalStatus, string> = {
  pending: "text-interaction-confirmation-foreground",
  allowed: "text-success",
  denied: "text-destructive",
};

/** 等待徽标统一用 confirmation 绿色（DESIGN.md「Blocking interaction colors」）。 */
export function approvalBlockClassName(className?: string): string {
  return cn([
    "rounded-xl border border-border bg-interaction-confirmation-surface px-3 py-2.5",
    className,
  ]);
}

export function approvalTitleClassName(className?: string): string {
  return cn(["text-ui-base font-medium text-foreground", className]);
}

export function approvalDescriptionClassName(className?: string): string {
  return cn(["mt-0.5 text-ui-sm text-foreground-subtle", className]);
}

export function approvalStatusClassName(status: ApprovalStatus, className?: string): string {
  return cn(["text-ui-xs", APPROVAL_STATUS_CLASSES[status], className]);
}

export function approvalActionsClassName(className?: string): string {
  return cn(["mt-2 flex items-center justify-end gap-2", className]);
}
