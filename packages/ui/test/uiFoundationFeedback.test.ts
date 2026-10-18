/**
 * M4 P1.1 Tabs/Tooltip/Toast 类名契约（spec: docs/specs/m4-ui-foundation.md §2.5/§2.6）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import {
  tabClassName,
  tabListClassName,
  toastClassName,
  tooltipClassName,
  tooltipShortcutClassName,
} from "../src/ui-foundation/feedbackStyles.js";
import {
  dialogClassName,
  menuClassName,
  popoverClassName,
} from "../src/ui-foundation/surfaceStyles.js";

test("Tabs：选中用 selected 表面，未选中中性；不使用 brand 填充", () => {
  assert.match(tabListClassName(), /inline-flex/);
  const active = tabClassName({ selected: true });
  assert.match(active, /bg-selected/);
  assert.doesNotMatch(active, /bg-primary/);
  const inactive = tabClassName({ selected: false });
  assert.match(inactive, /text-foreground-subtle/);
  assert.match(inactive, /hover:bg-hover/);
});

test("Tooltip：tooltip token、text-ui-sm、快捷键 text-ui-xs", () => {
  const tooltip = tooltipClassName();
  assert.match(tooltip, /bg-tooltip/);
  assert.match(tooltip, /text-tooltip-foreground/);
  assert.match(tooltip, /text-ui-sm/);
  assert.match(tooltip, /z-40/);
  assert.match(tooltipShortcutClassName(), /bg-tooltip-tag/);
  assert.match(tooltipShortcutClassName(), /text-ui-xs/);
});

test("交互浮层高于 Tooltip；Toast 用 toast token 与 2xl", () => {
  for (const classes of [menuClassName(), popoverClassName(), dialogClassName()]) {
    assert.match(classes, /z-50/);
  }
  const toast = toastClassName();
  assert.match(toast, /bg-toast/);
  assert.match(toast, /rounded-2xl/);
  assert.match(toast, /shadow-lg/);
  assert.match(toastClassName({ destructive: true }), /border-destructive/);
});

test("Tabs/Tooltip/Toast 类名通过排版守卫", () => {
  for (const classes of [
    tabListClassName(),
    tabClassName({ selected: true }),
    tabClassName({ selected: false }),
    tooltipClassName(),
    tooltipShortcutClassName(),
    toastClassName(),
  ]) {
    assert.deepEqual(findUiTypographyViolations([classes]), []);
  }
});
