/**
 * M4 P1.2 工作区外壳类名契约（spec: docs/specs/m4-ui-foundation.md §3）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import {
  conversationColumnClassName,
  conversationFrameClassName,
  mobileDrawerClassName,
  resizeHandleClassName,
  sidePaneFrameClassName,
  terminalFrameClassName,
  workspaceShellClassName,
} from "../src/ui-foundation/shellStyles.js";

test("外壳与三个 frame 使用 4px 间隔、独立背景和边框", () => {
  const shell = workspaceShellClassName();
  assert.match(shell, /flex/);
  assert.match(shell, /gap-1/);
  assert.match(shell, /min-h-0/);
  assert.match(shell, /bg-background/);

  const conversation = conversationFrameClassName();
  assert.match(conversation, /bg-panel/);
  assert.match(conversation, /border-border/);
  assert.match(conversation, /rounded-xl/);
  assert.match(conversation, /overflow-hidden/);

  assert.match(terminalFrameClassName(), /h-56/);
  assert.match(sidePaneFrameClassName(), /w-80/);
  assert.match(sidePaneFrameClassName(), /shrink-0/);
  assert.match(conversationColumnClassName(), /flex-1/);
});

test("分隔条 4px 命中区，hover/focus/drag 显示 2px 线", () => {
  const vertical = resizeHandleClassName("vertical");
  assert.match(vertical, /w-1/);
  assert.match(vertical, /cursor-col-resize/);
  assert.match(vertical, /after:w-0\.5/);
  assert.match(vertical, /hover:after:opacity-100/);
  assert.match(vertical, /focus-visible:after:opacity-100/);
  assert.match(vertical, /data-\[dragging=true\]:after:opacity-100/);

  const horizontal = resizeHandleClassName("horizontal");
  assert.match(horizontal, /h-1/);
  assert.match(horizontal, /cursor-row-resize/);
  assert.match(horizontal, /after:h-0\.5/);
});

test("手机 drawer 固定底部并位于浮层层级", () => {
  const drawer = mobileDrawerClassName();
  assert.match(drawer, /fixed/);
  assert.match(drawer, /bottom-0/);
  assert.match(drawer, /z-50/);
});

test("外壳类名通过排版守卫", () => {
  for (const classes of [
    workspaceShellClassName(),
    conversationColumnClassName(),
    conversationFrameClassName(),
    terminalFrameClassName(),
    sidePaneFrameClassName(),
    resizeHandleClassName("vertical"),
    resizeHandleClassName("horizontal"),
    mobileDrawerClassName(),
  ]) {
    assert.deepEqual(findUiTypographyViolations([classes]), []);
  }
});
