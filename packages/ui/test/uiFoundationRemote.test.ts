/**
 * M4 P1.6 远程/开发者面板类名契约（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.6）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import {
  previewHeaderClassName,
  previewPaneClassName,
  remoteLayoutClassName,
  remoteOfflineOverlayClassName,
  remoteStatusItemClassName,
  remoteStatusStripClassName,
  remoteSurfaceClassName,
  remoteToolbarButtonClassName,
  remoteToolbarClassName,
  terminalPaneClassName,
} from "../src/ui-foundation/remoteStyles.js";

test("远程画布：toolbar/画布/状态条三段，画布可裁剪", () => {
  assert.match(remoteLayoutClassName(), /flex-col/);
  const surface = remoteSurfaceClassName();
  assert.match(surface, /overflow-hidden/);
  assert.match(surface, /rounded-xl/);
  assert.match(surface, /bg-background/);
  assert.match(remoteToolbarClassName(), /bg-panel/);
  assert.match(remoteStatusStripClassName(), /text-ui-xs/);
  assert.match(remoteStatusItemClassName(), /inline-flex/);
});

test("工具栏按钮：图标按钮方形、选中态与焦点环", () => {
  const button = remoteToolbarButtonClassName();
  assert.match(button, /size-7/);
  assert.match(button, /rounded-md/);
  assert.match(button, /focus-visible:ring-2/);
  const active = remoteToolbarButtonClassName({ active: true });
  assert.match(active, /bg-selected/);
  assert.match(active, /text-foreground/);
});

test("断流覆盖使用不透明背景（没有无 token 的透明黑）", () => {
  const overlay = remoteOfflineOverlayClassName();
  assert.match(overlay, /absolute/);
  assert.match(overlay, /inset-0/);
  assert.match(overlay, /bg-background/);
  assert.doesNotMatch(overlay, /black\//);
  assert.match(overlay, /text-ui-sm/);
});

test("终端与预览面板：mono/surface 与 header 分隔", () => {
  const terminal = terminalPaneClassName();
  assert.match(terminal, /font-mono/);
  assert.match(terminal, /bg-panel/);
  assert.match(terminal, /overflow-y-auto/);

  const preview = previewPaneClassName();
  assert.match(preview, /bg-surface/);
  assert.match(preview, /rounded-xl/);
  assert.match(preview, /overflow-hidden/);
  assert.match(previewHeaderClassName(), /h-8/);
  assert.match(previewHeaderClassName(), /border-b/);
});

test("远程面板类名通过排版守卫", () => {
  for (const classes of [
    remoteLayoutClassName(),
    remoteSurfaceClassName(),
    remoteToolbarClassName(),
    remoteToolbarButtonClassName(),
    remoteStatusStripClassName(),
    remoteStatusItemClassName(),
    remoteOfflineOverlayClassName(),
    terminalPaneClassName(),
    previewPaneClassName(),
    previewHeaderClassName(),
  ]) {
    assert.deepEqual(findUiTypographyViolations([classes]), []);
  }
});
