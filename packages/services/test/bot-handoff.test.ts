import assert from "node:assert/strict";
import test from "node:test";

// 跨聊天交接协议 schema（docs/specs/bot-cross-chat-handoff.md R1/R5）：
// strict 拒绝多余键；状态枚举与 toolCallId 必填，保证幂等键始终存在。

test("modeBotHandoffParamsSchema：合法参数通过，多余键被拒绝", async () => {
  const { modeBotHandoffParamsSchema } = await import("@mode/shared");
  const base = {
    target: { provider: "wecom", botId: "b1", providerUserId: "u1", chatType: "private" },
    conversationKey: "private:u2",
    message: "请转达",
    toolCallId: "call-1",
  };
  assert.deepEqual(modeBotHandoffParamsSchema.parse(base), base);
  assert.throws(() => modeBotHandoffParamsSchema.parse({ ...base, extra: 1 }), /unrecognized/i);
  assert.throws(() => modeBotHandoffParamsSchema.parse({ ...base, toolCallId: "" }), /./);
  assert.throws(() => modeBotHandoffParamsSchema.parse({ ...base, message: "" }), /./);
});

test("modeBotHandoffResultSchema：三态 + 目标回显", async () => {
  const { modeBotHandoffResultSchema } = await import("@mode/shared");
  for (const status of ["submitted", "queued", "duplicate"] as const) {
    assert.equal(
      modeBotHandoffResultSchema.parse({ status, conversationKey: "private:u2" }).status,
      status,
    );
  }
  assert.throws(() => modeBotHandoffResultSchema.parse({ status: "lost", conversationKey: "x" }));
});
