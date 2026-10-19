import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (relativePath: string): string =>
  readFileSync(new URL(relativePath, import.meta.url), "utf8");

test("BotsDialog 展示 Agent 归属只读投影", () => {
  const dialog = read("../src/BotsDialog.tsx");
  assert.ok(dialog.includes("<BotAgentOwnershipCard"), "BotsDialog 缺少 Agent 归属卡片");
  const card = read("../src/BotsDialog/BotAgentOwnershipCard.tsx");
  assert.ok(card.includes("resolveBotAgentId(bot.id)"), "Agent 归属必须使用统一派生 id");
  assert.ok(card.includes("bots.agentOwnership.unbound"), "未绑定工作区需要明确文案");
});
