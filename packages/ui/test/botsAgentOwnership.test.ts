import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

// Agent 归属已合并进「工作区管理」（只读 Agent id + 绑定工作区同一行），
// 独立卡片不再渲染，避免同一块信息占两行。

const dialog = await readFile(new URL("../src/BotsDialog.tsx", import.meta.url), "utf8");
const workspaceCard = await readFile(
  new URL("../src/BotsDialog/WorkspaceManagementCard.tsx", import.meta.url),
  "utf8",
);

test("Agent 归属合并进工作区管理卡", () => {
  assert.ok(
    !dialog.includes("<BotAgentOwnershipCard"),
    "BotsDialog 不再渲染独立的 Agent 归属卡片",
  );
  assert.ok(
    workspaceCard.includes("resolveBotAgentId(bot.id)"),
    "工作区管理卡必须展示统一派生的 Agent id",
  );
  assert.ok(
    workspaceCard.includes("SettingsRow"),
    "Agent 归属继续通过工作区管理的设置行展示",
  );
});
