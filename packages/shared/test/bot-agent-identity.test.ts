import assert from "node:assert/strict";
import test from "node:test";

// A+ 迁移默认：一个 Bot 对应一个 Agent；v4 落地前 UI 用该派生值展示归属。
test("resolveBotAgentId：按迁移默认一 Bot 一 Agent 派生稳定 id", async () => {
  const { resolveBotAgentId } = await import("../src/bots.js");
  assert.equal(resolveBotAgentId("bot-1"), "agent-bot-1");
  assert.equal(resolveBotAgentId("bot-1"), resolveBotAgentId("bot-1"));
  assert.notEqual(resolveBotAgentId("bot-1"), resolveBotAgentId("bot-2"));
});
