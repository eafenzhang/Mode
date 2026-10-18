/**
 * M4 P1.7 引导/quickpick/快捷键类名契约（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.7）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import {
  onboardingActionsClassName,
  onboardingCardClassName,
  onboardingContainerClassName,
  onboardingDescriptionClassName,
  onboardingStepDotClassName,
  onboardingTitleClassName,
} from "../src/ui-foundation/onboardingStyles.js";
import {
  quickpickEmptyClassName,
  quickpickInputClassName,
  quickpickItemClassName,
  quickpickListClassName,
  quickpickPanelClassName,
} from "../src/ui-foundation/quickpickStyles.js";
import {
  shortcutKeyClassName,
  shortcutKeysClassName,
  shortcutLabelClassName,
  shortcutRowClassName,
} from "../src/ui-foundation/shortcutStyles.js";

test("引导：居中容器、卡片、xl 标题与步骤点状态", () => {
  assert.match(onboardingContainerClassName(), /items-center/);
  assert.match(onboardingContainerClassName(), /justify-center/);
  assert.match(onboardingCardClassName(), /max-w-lg/);
  assert.match(onboardingCardClassName(), /bg-card/);
  assert.match(onboardingTitleClassName(), /text-ui-xl/);
  assert.match(onboardingDescriptionClassName(), /text-ui-sm/);
  assert.match(onboardingActionsClassName(), /justify-end/);

  assert.match(onboardingStepDotClassName("completed"), /bg-success/);
  assert.match(onboardingStepDotClassName("active"), /bg-primary/);
  assert.match(onboardingStepDotClassName("pending"), /bg-foreground-subtlest/);
});

test("quickpick：popover 面板、紧凑行、选中态与空态", () => {
  const panel = quickpickPanelClassName();
  assert.match(panel, /w-80/);
  assert.match(panel, /bg-popover/);
  assert.match(panel, /shadow-md/);
  assert.match(quickpickInputClassName(), /text-ui-base/);
  assert.match(quickpickInputClassName(), /placeholder:text-foreground-subtlest/);
  assert.match(quickpickListClassName(), /max-h-80/);
  assert.match(quickpickItemClassName({ selected: true }), /bg-selected/);
  assert.match(quickpickItemClassName(), /hover:bg-menu-hover/);
  assert.match(quickpickEmptyClassName(), /text-center/);
});

test("快捷键：行两端对齐、标签可截断、kbd 用 mono", () => {
  assert.match(shortcutRowClassName(), /justify-between/);
  assert.match(shortcutLabelClassName(), /truncate/);
  assert.match(shortcutKeysClassName(), /shrink-0/);
  const key = shortcutKeyClassName();
  assert.match(key, /font-mono/);
  assert.match(key, /text-ui-xs/);
  assert.match(key, /border-border/);
});

test("P1.7 类名通过排版守卫", () => {
  for (const classes of [
    onboardingContainerClassName(),
    onboardingCardClassName(),
    onboardingTitleClassName(),
    onboardingDescriptionClassName(),
    onboardingActionsClassName(),
    onboardingStepDotClassName("active"),
    quickpickPanelClassName(),
    quickpickInputClassName(),
    quickpickListClassName(),
    quickpickItemClassName(),
    quickpickEmptyClassName(),
    shortcutRowClassName(),
    shortcutLabelClassName(),
    shortcutKeysClassName(),
    shortcutKeyClassName(),
  ]) {
    assert.deepEqual(findUiTypographyViolations([classes]), []);
  }
});
