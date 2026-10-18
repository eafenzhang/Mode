/**
 * M4 P1.3 工具/审批块类名契约（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.3）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import {
  approvalActionsClassName,
  approvalBlockClassName,
  approvalStatusClassName,
  toolBlockClassName,
  toolBodyClassName,
  toolNameClassName,
  toolStatusClassName,
} from "../src/ui-foundation/toolStyles.js";

test("工具块五态：scheduled 虚线、error 语义红，其余常规边框", () => {
  assert.match(toolBlockClassName("scheduled"), /border-dashed/);
  assert.match(toolBlockClassName("scheduled"), /text-foreground-subtle/);
  assert.match(toolBlockClassName("error"), /border-destructive/);
  assert.match(toolBlockClassName("error"), /text-destructive/);
  for (const state of ["started", "progress", "result"] as const) {
    assert.match(toolBlockClassName(state), /border-border/);
  }
});

test("工具名与正文使用 mono，正文保留换行", () => {
  assert.match(toolNameClassName(), /font-mono/);
  assert.match(toolNameClassName(), /text-ui-sm/);
  assert.match(toolBodyClassName(), /font-mono/);
  assert.match(toolBodyClassName(), /whitespace-pre-wrap/);
  assert.match(toolBodyClassName(), /break-words/);
});

test("工具状态色：error 红，其余弱化", () => {
  assert.match(toolStatusClassName("error"), /text-destructive/);
  assert.match(toolStatusClassName("progress"), /text-foreground-subtlest/);
});

test("审批块：等待用 confirmation 绿色，允许/拒绝用语义色", () => {
  assert.match(approvalBlockClassName(), /bg-interaction-confirmation-surface/);
  assert.match(approvalStatusClassName("pending"), /text-interaction-confirmation-foreground/);
  assert.match(approvalStatusClassName("allowed"), /text-success/);
  assert.match(approvalStatusClassName("denied"), /text-destructive/);
  assert.match(approvalActionsClassName(), /justify-end/);
});

test("工具/审批类名通过排版守卫", () => {
  for (const classes of [
    toolBlockClassName("scheduled"),
    toolBlockClassName("error"),
    toolNameClassName(),
    toolBodyClassName(),
    toolStatusClassName("error"),
    approvalBlockClassName(),
    approvalStatusClassName("pending"),
    approvalActionsClassName(),
  ]) {
    assert.deepEqual(findUiTypographyViolations([classes]), []);
  }
});
