/**
 * M4 P1.4 工作区文件树/任务列表类名契约（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.4）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
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
} from "../src/ui-foundation/workspaceStyles.js";

test("文件树：可滚动、紧凑行、选中与焦点态、缩进步长", () => {
  assert.match(fileTreeClassName(), /overflow-y-auto/);
  assert.match(fileTreeClassName(), /flex-1/);
  assert.equal(FILE_TREE_INDENT_BASE_PX, 8);
  assert.equal(FILE_TREE_INDENT_STEP_PX, 12);

  const item = fileTreeItemClassName({ selected: true });
  assert.match(item, /h-7/);
  assert.match(item, /bg-selected/);
  assert.match(item, /hover:bg-hover/);
  assert.match(item, /focus-visible:ring-2/);
  assert.match(fileTreeIconClassName(), /size-4/);
  assert.match(fileTreeLabelClassName(), /truncate/);
});

test("任务列表：行高、选中态、标题截断与弱化元数据", () => {
  assert.match(taskListClassName(), /gap-1/);
  const row = taskRowClassName({ selected: true });
  assert.match(row, /min-h-8/);
  assert.match(row, /bg-selected/);
  assert.match(taskRowTitleClassName(), /text-ui-sm/);
  assert.match(taskRowTitleClassName(), /font-medium/);
  assert.match(taskRowMetaClassName(), /text-ui-xs/);
  assert.match(taskRowMetaClassName(), /text-foreground-subtlest/);
});

test("任务状态使用语义色：running warning、completed success、failed destructive", () => {
  const expected: Record<TaskStatus, RegExp> = {
    queued: /text-foreground-subtlest/,
    running: /text-warning/,
    completed: /text-success/,
    cancelled: /text-foreground-subtle/,
    failed: /text-destructive/,
  };
  for (const [status, pattern] of Object.entries(expected)) {
    assert.match(taskStatusBadgeClassName(status as TaskStatus), pattern);
  }
  assert.match(taskStatusDotClassName("running"), /bg-warning/);
  assert.match(taskStatusDotClassName("completed"), /bg-success/);
  assert.match(taskStatusDotClassName("failed"), /bg-destructive/);
});

test("工作区类名通过排版守卫", () => {
  for (const classes of [
    fileTreeClassName(),
    fileTreeItemClassName(),
    fileTreeIconClassName(),
    fileTreeLabelClassName(),
    taskListClassName(),
    taskRowClassName(),
    taskRowTitleClassName(),
    taskRowMetaClassName(),
    taskStatusBadgeClassName("running"),
  ]) {
    assert.deepEqual(findUiTypographyViolations([classes]), []);
  }
});
