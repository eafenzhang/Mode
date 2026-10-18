/**
 * M4 P1.5 设置页/插件卡片类名契约（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.5）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import {
  pluginAvailabilityBadgeClassName,
  pluginCardActionsClassName,
  pluginCardClassName,
  pluginCardDescriptionClassName,
  pluginCardTitleClassName,
  pluginDetailHintClassName,
  pluginIconClassName,
  settingsContentClassName,
  settingsLayoutClassName,
  settingsNavClassName,
  settingsNavItemClassName,
  settingsSectionDescriptionClassName,
  settingsSectionTitleClassName,
} from "../src/ui-foundation/settingsStyles.js";

test("设置布局：左侧导航固定宽、右侧内容可滚动", () => {
  assert.match(settingsLayoutClassName(), /flex/);
  assert.match(settingsNavClassName(), /w-56/);
  assert.match(settingsNavClassName(), /border-e/);
  assert.match(settingsNavClassName(), /overflow-y-auto/);
  assert.match(settingsContentClassName(), /overflow-y-auto/);
  assert.match(settingsNavItemClassName({ selected: true }), /bg-selected/);
  assert.match(settingsNavItemClassName(), /focus-visible:ring-2/);
});

test("设置分区：标题 base、说明 sm", () => {
  assert.match(settingsSectionTitleClassName(), /text-ui-base/);
  assert.match(settingsSectionTitleClassName(), /font-medium/);
  assert.match(settingsSectionDescriptionClassName(), /text-ui-sm/);
  assert.match(settingsSectionDescriptionClassName(), /text-foreground-subtle/);
});

test("插件卡片：card 表面、品牌图标背板 2xl、说明两行截断", () => {
  const card = pluginCardClassName();
  assert.match(card, /bg-card/);
  assert.match(card, /border-card-border/);
  assert.match(card, /rounded-xl/);
  assert.match(pluginIconClassName(), /rounded-2xl/);
  assert.match(pluginCardTitleClassName(), /text-ui-base/);
  assert.match(pluginCardDescriptionClassName(), /line-clamp-2/);
  assert.match(pluginCardActionsClassName(), /items-center/);
});

test("不可安装：卡片只出 pill，详情给固定说明（不提供重试）", () => {
  const badge = pluginAvailabilityBadgeClassName();
  assert.match(badge, /rounded-full/);
  assert.match(badge, /text-ui-xs/);
  assert.match(badge, /border-border/);
  const hint = pluginDetailHintClassName();
  assert.match(hint, /bg-surface/);
  assert.match(hint, /rounded-xl/);
  assert.doesNotMatch(hint, /retry/i);
});

test("设置/插件类名通过排版守卫", () => {
  for (const classes of [
    settingsLayoutClassName(),
    settingsNavClassName(),
    settingsNavItemClassName({ selected: true }),
    settingsContentClassName(),
    settingsSectionTitleClassName(),
    settingsSectionDescriptionClassName(),
    pluginCardClassName(),
    pluginIconClassName(),
    pluginCardTitleClassName(),
    pluginCardDescriptionClassName(),
    pluginAvailabilityBadgeClassName(),
    pluginDetailHintClassName(),
  ]) {
    assert.deepEqual(findUiTypographyViolations([classes]), []);
  }
});
