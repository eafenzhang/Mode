import assert from "node:assert/strict";
import test from "node:test";

// 电脑控制设置分区可见性（docs/specs/computer-use-enablement.md 验收场景）：
// 分区从 HIDDEN_SETTINGS_SECTIONS 放开后，桌面配置必须含 computerUse，
// Web 配置保持排除；跳转解析不再把 computerUse 回退到 general。
const { createSettingsPageConfig } = await import("../src/settings/settingsPageConfig.js");
const { isSettingsSectionEnabled, resolveSettingsSection } =
  await import("../src/lib/settingsNavigation.js");

test("computerUse 不再是隐藏设置分区", () => {
  assert.equal(isSettingsSectionEnabled("computerUse"), true);
});

test("跳转解析保留 computerUse（不再回退到 general）", () => {
  assert.equal(resolveSettingsSection("computerUse"), "computerUse");
});

test("桌面（Windows）配置包含 computerUse 且排在浏览器之后的基础设置组", () => {
  const { settingsSections } = createSettingsPageConfig({
    isDesktop: true,
    isWindowsDesktop: true,
  });
  const ids = settingsSections.map((section) => section.id);
  assert.ok(ids.includes("computerUse"), "桌面配置应包含 computerUse 分区");
  const computerUseSection = settingsSections.find((section) => section.id === "computerUse");
  assert.equal(computerUseSection?.groupId, "basics");
  assert.ok(ids.indexOf("computerUse") > ids.indexOf("browser"), "电脑控制应排在浏览器之后");
});

test("Web 配置不包含 computerUse", () => {
  const { settingsSections } = createSettingsPageConfig({});
  assert.ok(
    !settingsSections.some((section) => section.id === "computerUse"),
    "Web 视图不应出现 computerUse 分区",
  );
});
