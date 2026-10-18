/**
 * M4 P1.2 WorkspaceHeader 类名契约（spec: docs/specs/m4-ui-foundation.md §3）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import {
  workspaceHeaderActionsClassName,
  workspaceHeaderClassName,
  workspaceHeaderSubtitleClassName,
  workspaceHeaderTitleClassName,
} from "../src/ui-foundation/headerStyles.js";

test("Header 使用紧凑高度、分割线与水平内边距", () => {
  const header = workspaceHeaderClassName();
  assert.match(header, /min-h-9/);
  assert.match(header, /border-b/);
  assert.match(header, /px-3/);
  assert.match(header, /shrink-0/);
});

test("标题与副标题按角色使用 text-ui-* 并可截断", () => {
  const title = workspaceHeaderTitleClassName();
  assert.match(title, /text-ui-base/);
  assert.match(title, /font-medium/);
  assert.match(title, /truncate/);
  assert.match(title, /min-w-0/);

  const subtitle = workspaceHeaderSubtitleClassName();
  assert.match(subtitle, /text-ui-sm/);
  assert.match(subtitle, /text-foreground-subtle/);
});

test("操作槽位靠尾部、不参与收缩", () => {
  const actions = workspaceHeaderActionsClassName();
  assert.match(actions, /ms-auto/);
  assert.match(actions, /shrink-0/);
});

test("Header 类名通过排版守卫", () => {
  for (const classes of [
    workspaceHeaderClassName(),
    workspaceHeaderTitleClassName(),
    workspaceHeaderSubtitleClassName(),
    workspaceHeaderActionsClassName(),
  ]) {
    assert.deepEqual(findUiTypographyViolations([classes]), []);
  }
});
