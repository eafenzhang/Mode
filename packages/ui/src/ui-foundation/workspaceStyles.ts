// P1.4 工作区文件树与任务列表的类名契约
// （plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.4）。
// 只负责展示；workspaceIdentity/path 解析仍归既有服务层（AGENTS.md）。

export const FILE_TREE_INDENT_BASE_PX = 8;
export const FILE_TREE_INDENT_STEP_PX = 12;

export function fileTreeClassName(className?: string): string {
  return ["flex min-h-0 flex-1 flex-col overflow-y-auto py-1", className]
    .filter(Boolean)
    .join(" ");
}

export interface FileTreeItemClassNameOptions {
  selected?: boolean;
  className?: string;
}

export function fileTreeItemClassName(options: FileTreeItemClassNameOptions = {}): string {
  return [
    "flex h-7 items-center gap-1.5 rounded-md pe-2 text-ui-sm text-foreground",
    "hover:bg-hover focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary",
    options.selected && "bg-selected",
    options.className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function fileTreeIconClassName(className?: string): string {
  return ["size-4 shrink-0 text-foreground-subtle", className].filter(Boolean).join(" ");
}

export function fileTreeLabelClassName(className?: string): string {
  return ["min-w-0 truncate", className].filter(Boolean).join(" ");
}

export function taskListClassName(className?: string): string {
  return ["flex min-h-0 flex-col gap-1 overflow-y-auto", className].filter(Boolean).join(" ");
}

export interface TaskRowClassNameOptions {
  selected?: boolean;
  className?: string;
}

export function taskRowClassName(options: TaskRowClassNameOptions = {}): string {
  return [
    "flex min-h-8 items-center gap-2 rounded-md px-2 py-1",
    "hover:bg-hover focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary",
    options.selected && "bg-selected",
    options.className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function taskRowTitleClassName(className?: string): string {
  return ["min-w-0 truncate text-ui-sm font-medium text-foreground", className]
    .filter(Boolean)
    .join(" ");
}

export function taskRowMetaClassName(className?: string): string {
  return ["shrink-0 text-ui-xs text-foreground-subtlest", className].filter(Boolean).join(" ");
}

export type TaskStatus = "queued" | "running" | "completed" | "cancelled" | "failed";

const TASK_STATUS_TEXT: Record<TaskStatus, string> = {
  queued: "text-foreground-subtlest",
  running: "text-warning",
  completed: "text-success",
  cancelled: "text-foreground-subtle",
  failed: "text-destructive",
};

const TASK_STATUS_DOT: Record<TaskStatus, string> = {
  queued: "bg-foreground-subtlest",
  running: "bg-warning",
  completed: "bg-success",
  cancelled: "bg-foreground-subtle",
  failed: "bg-destructive",
};

export function taskStatusBadgeClassName(status: TaskStatus, className?: string): string {
  return ["inline-flex shrink-0 items-center gap-1 text-ui-xs", TASK_STATUS_TEXT[status], className]
    .filter(Boolean)
    .join(" ");
}

export function taskStatusDotClassName(status: TaskStatus, className?: string): string {
  return ["size-1.5 shrink-0 rounded-full", TASK_STATUS_DOT[status], className]
    .filter(Boolean)
    .join(" ");
}
