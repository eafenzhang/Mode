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

test("Bot 详情设置使用 stacked 左对齐布局", () => {
  const settingsRow = read("../src/settings/SettingsPageParts.tsx");
  assert.match(settingsRow, /layout === "stacked"/);
  assert.match(
    settingsRow,
    /justify-start gap-2/,
    "stacked 布局的控件必须左对齐，不再是右侧 192px 列",
  );

  for (const file of [
    "../src/BotsDialog/BotHeartbeatCard.tsx",
    "../src/BotsDialog/BotDeliveryModeCard.tsx",
    "../src/BotsDialog/BotPrivateChatCard.tsx",
    "../src/BotsDialog/BotGroupChatCard.tsx",
    "../src/BotsDialog/WorkspaceManagementCard.tsx",
    "../src/BotsDialog/BotDingtalkCardConfigCard.tsx",
  ]) {
    assert.match(read(file), /layout="stacked"/, `${file} 必须使用 stacked 布局`);
  }
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
