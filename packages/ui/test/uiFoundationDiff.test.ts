/**
 * M4 P1.3 diff 与排队输入类名契约（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.3）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import {
  diffBlockClassName,
  diffHeaderClassName,
  diffLineClassName,
  diffMarker,
  queuedInputActionsClassName,
  queuedInputItemClassName,
  queuedInputListClassName,
  queuedInputTextClassName,
} from "../src/ui-foundation/diffStyles.js";

test("Diff 使用专用语义色，绝不借用 success/destructive", () => {
  const added = diffLineClassName("added");
  assert.match(added, /bg-diff-added/);
  assert.match(added, /text-diff-added-foreground/);
  assert.doesNotMatch(added, /success/);

  const removed = diffLineClassName("removed");
  assert.match(removed, /bg-diff-removed/);
  assert.match(removed, /text-diff-removed-foreground/);
  assert.doesNotMatch(removed, /destructive/);

  assert.match(diffLineClassName("context"), /text-foreground/);
});

test("Diff 容器与 header：surface、mono、分隔线", () => {
  const block = diffBlockClassName();
  assert.match(block, /bg-surface/);
  assert.match(block, /font-mono/);
  assert.match(block, /rounded-xl/);
  assert.match(diffHeaderClassName(), /border-b/);
  assert.equal(diffMarker("added"), "+");
  assert.equal(diffMarker("removed"), "-");
  assert.equal(diffMarker("context"), " ");
});

test("排队输入：紧凑行、可截断文本、尾部操作槽", () => {
  assert.match(queuedInputListClassName(), /flex-col/);
  assert.match(queuedInputListClassName(), /gap-1\.5/);
  const item = queuedInputItemClassName();
  assert.match(item, /rounded-lg/);
  assert.match(item, /border-border/);
  assert.match(item, /bg-surface/);
  assert.match(queuedInputTextClassName(), /truncate/);
  assert.match(queuedInputTextClassName(), /min-w-0/);
  assert.match(queuedInputActionsClassName(), /ms-auto/);
});

test("diff/排队输入类名通过排版守卫", () => {
  for (const classes of [
    diffBlockClassName(),
    diffHeaderClassName(),
    diffLineClassName("added"),
    diffLineClassName("removed"),
    diffLineClassName("context"),
    queuedInputListClassName(),
    queuedInputItemClassName(),
    queuedInputTextClassName(),
    queuedInputActionsClassName(),
  ]) {
    assert.deepEqual(findUiTypographyViolations([classes]), []);
  }
});
