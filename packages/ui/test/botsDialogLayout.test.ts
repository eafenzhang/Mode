import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (relativePath: string): string =>
  readFileSync(new URL(relativePath, import.meta.url), "utf8");

test("SettingsRow 控制列允许换行，避免固定列宽时向右溢出", () => {
  const source = read("../src/settings/SettingsPageParts.tsx");
  assert.match(
    source,
    /flex w-full min-w-0 flex-wrap items-center justify-end gap-2/,
    "inline 控制列必须有 min-w-0 + flex-wrap（BotsDialog 右栏横向溢出的根因）",
  );
});

test("BotsDialog 右栏禁止横向滚动，纵向滚动保持", () => {
  const source = read("../src/BotsDialog.tsx");
  assert.match(source, /overflow-x-hidden overflow-y-auto/);
});

test("Bot 详情设置使用 inline 布局，控件在右列", () => {
  const settingsRow = read("../src/settings/SettingsPageParts.tsx");
  assert.ok(!settingsRow.includes('layout === "stacked"'), "SettingsRow 不再保留 stacked 分支");
  assert.match(
    settingsRow,
    /justify-end gap-2/,
    "inline 布局的控制列必须右对齐（标题/说明在左、控件在右）",
  );

  for (const file of [
    "../src/BotsDialog/BotHeartbeatCard.tsx",
    "../src/BotsDialog/BotDeliveryModeCard.tsx",
    "../src/BotsDialog/BotPrivateChatCard.tsx",
    "../src/BotsDialog/BotGroupChatCard.tsx",
    "../src/BotsDialog/WorkspaceManagementCard.tsx",
    "../src/BotsDialog/BotDingtalkCardConfigCard.tsx",
    "../src/BotsDialog/BotSummaryCard.tsx",
    "../src/BotsDialog/ProviderSettingsCard.tsx",
  ]) {
    assert.ok(!read(file).includes('layout="stacked"'), `${file} 不应再使用 stacked 布局`);
  }
});

test("BotsDialog 内容区有统一内边距，次要文字局部提亮", () => {
  const source = read("../src/BotsDialog.tsx");
  assert.match(
    source,
    /flex-col gap-3 overflow-hidden rounded-2xl p-4/,
    "弹窗 shell 必须有 p-4 + gap-3，避免内容贴着圆角边缘",
  );
  assert.match(source, /--color-foreground-subtle/, "次要文字对比度应在弹窗范围内覆盖");
});

test("Bot 详情把设置合并为三组，并移除独立 Agent 卡", () => {
  const source = read("../src/BotsDialog.tsx");
  assert.equal(
    (source.match(/<SettingsGroupCard>/gu) ?? []).length >= 3,
    true,
    "至少按自动回复 / 聊天接入 / 工作区三组收口",
  );
  assert.ok(!source.includes("<BotAgentOwnershipCard"), "Agent 归属已并入工作区管理");
});
