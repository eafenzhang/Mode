// P1.4 文件树与任务列表静态结构（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.4）。
// 只做展示与 ARIA；文件/任务数据由调用方注入。

import { forwardRef, type HTMLAttributes, type LiHTMLAttributes, type ReactNode } from "react";

import {
  FILE_TREE_INDENT_BASE_PX,
  FILE_TREE_INDENT_STEP_PX,
  fileTreeClassName,
  fileTreeIconClassName,
  fileTreeItemClassName,
  fileTreeLabelClassName,
  taskListClassName,
  taskRowClassName,
  taskRowMetaClassName,
  taskRowTitleClassName,
  taskStatusBadgeClassName,
  taskStatusDotClassName,
  type TaskStatus,
} from "./workspaceStyles.js";

export interface FileTreeProps extends HTMLAttributes<HTMLUListElement> {
  label: string;
}

export const FileTree = forwardRef<HTMLUListElement, FileTreeProps>(function FileTree(
  { label, className, ...rest },
  ref,
) {
  return (
    <ul
      {...rest}
      ref={ref}
      role="tree"
      aria-label={label}
      className={fileTreeClassName(className)}
    />
  );
});

export interface FileTreeItemProps extends LiHTMLAttributes<HTMLLIElement> {
  depth: number;
  kind: "file" | "folder";
  label: ReactNode;
  icon?: ReactNode;
  selected?: boolean;
  expanded?: boolean;
  children?: ReactNode;
}

export const FileTreeItem = forwardRef<HTMLLIElement, FileTreeItemProps>(function FileTreeItem(
  { depth, kind, label, icon, selected, expanded, className, children, style, ...rest },
  ref,
) {
  return (
    <li
      {...rest}
      ref={ref}
      role="treeitem"
      aria-level={depth + 1}
      aria-selected={selected}
      aria-expanded={kind === "folder" ? Boolean(expanded) : undefined}
      style={{
        ...style,
        paddingInlineStart: FILE_TREE_INDENT_BASE_PX + depth * FILE_TREE_INDENT_STEP_PX,
      }}
      className={fileTreeItemClassName({ selected, className })}
    >
      {icon ? <span className={fileTreeIconClassName()}>{icon}</span> : null}
      <span className={fileTreeLabelClassName()}>{label}</span>
      {children ? <ul role="group">{children}</ul> : null}
    </li>
  );
});

export interface TaskListProps extends HTMLAttributes<HTMLUListElement> {
  label: string;
}

export const TaskList = forwardRef<HTMLUListElement, TaskListProps>(function TaskList(
  { label, className, ...rest },
  ref,
) {
  return <ul {...rest} ref={ref} aria-label={label} className={taskListClassName(className)} />;
});

export interface TaskStatusBadgeProps extends HTMLAttributes<HTMLSpanElement> {
  status: TaskStatus;
  /** 状态文案（i18n 由调用方提供）。 */
  label: string;
}

export function TaskStatusBadge({
  status,
  label,
  className,
  ...rest
}: TaskStatusBadgeProps): ReactNode {
  return (
    <span {...rest} className={taskStatusBadgeClassName(status, className)}>
      <span aria-hidden="true" className={taskStatusDotClassName(status)} />
      {label}
    </span>
  );
}

export interface TaskRowProps extends Omit<LiHTMLAttributes<HTMLLIElement>, "title"> {
  title: ReactNode;
  meta?: ReactNode;
  status?: ReactNode;
  actions?: ReactNode;
  selected?: boolean;
}

export const TaskRow = forwardRef<HTMLLIElement, TaskRowProps>(function TaskRow(
  { title, meta, status, actions, selected, className, ...rest },
  ref,
) {
  return (
    <li
      {...rest}
      ref={ref}
      aria-current={selected ? "true" : undefined}
      className={taskRowClassName({ selected, className })}
    >
      <span className={taskRowTitleClassName()}>{title}</span>
      {meta ? <span className={taskRowMetaClassName()}>{meta}</span> : null}
      {status}
      {actions ? <span className="ms-auto flex shrink-0 items-center gap-1">{actions}</span> : null}
    </li>
  );
});
