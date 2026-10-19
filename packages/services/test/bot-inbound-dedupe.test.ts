import assert from "node:assert/strict";
import test from "node:test";
import type { BotInboundMessage } from "@mode/shared";

// 入站消息幂等：平台重连/重投时同一 provider message id 不能触发第二轮 AI。
// 去重键必须带 bot + provider + 对话身份，避免不同群/私聊互相误伤。

function message(
  overrides: {
    botId?: string;
    provider?: "weixin" | "wecom" | "feishu" | "dingtalk";
    providerUserId?: string;
    chatId?: string;
    providerMessageId?: string;
  } = {},
): BotInboundMessage {
  const {
    botId = "bot-1",
    provider = "wecom",
    providerUserId = "user-1",
    chatId,
    providerMessageId = "msg-1",
  } = overrides;
  return {
    botId,
    text: "hello",
    actor: {
      botId,
      provider,
      providerUserId,
      chatType: chatId ? "group" : "private",
      ...(chatId ? { chatId } : {}),
      ...(providerMessageId ? { providerMessageId } : {}),
    },
  };
}

test("buildInboundDeliveryKey：bot + provider + 对话 + message id；无 id 返回 null", async () => {
  const { buildInboundDeliveryKey } = await import("../src/bots/inboundDedupe.js");
  assert.equal(buildInboundDeliveryKey(message()), "bot-1::wecom::user-1::msg-1");
  assert.equal(
    buildInboundDeliveryKey(message({ chatId: "group-1" })),
    "bot-1::wecom::group-1::msg-1",
  );
  assert.equal(buildInboundDeliveryKey(message({ providerMessageId: "" })), null);
  // provider message id 两侧空白不能产生第二个键
  assert.equal(
    buildInboundDeliveryKey(message({ providerMessageId: "  msg-1  " })),
    buildInboundDeliveryKey(message()),
  );
});

test("mark：同一 bot/provider/对话的同一 message id 只放行一次", async () => {
  const { createInboundDeliveryDedupe } = await import("../src/bots/inboundDedupe.js");
  const dedupe = createInboundDeliveryDedupe();
  assert.equal(dedupe.mark(message(), 1000), true);
  assert.equal(dedupe.mark(message(), 1001), false);
});

test("mark：不同 message id / 对话 / bot / provider 互不影响", async () => {
  const { createInboundDeliveryDedupe } = await import("../src/bots/inboundDedupe.js");
  const dedupe = createInboundDeliveryDedupe();
  assert.equal(dedupe.mark(message(), 1000), true);
  assert.equal(dedupe.mark(message({ providerMessageId: "msg-2" }), 1000), true);
  assert.equal(dedupe.mark(message({ chatId: "group-1" }), 1000), true);
  assert.equal(dedupe.mark(message({ botId: "bot-2" }), 1000), true);
  assert.equal(dedupe.mark(message({ provider: "feishu" }), 1000), true);
  // 原键仍然被视为重复
  assert.equal(dedupe.mark(message(), 1001), false);
});

test("mark：平台未提供 message id 时不去重", async () => {
  const { createInboundDeliveryDedupe } = await import("../src/bots/inboundDedupe.js");
  const dedupe = createInboundDeliveryDedupe();
  const noId = message({ providerMessageId: "" });
  assert.equal(dedupe.mark(noId, 1000), true);
  assert.equal(dedupe.mark(noId, 1001), true);
});

test("mark：TTL 到期后允许重投，窗口内仍拒绝", async () => {
  const { createInboundDeliveryDedupe } = await import("../src/bots/inboundDedupe.js");
  const dedupe = createInboundDeliveryDedupe(1_000);
  assert.equal(dedupe.mark(message(), 2_000), true);
  assert.equal(dedupe.mark(message(), 2_999), false);
  assert.equal(dedupe.mark(message(), 3_000), true);
});

test("release：业务处理失败后释放键，平台重投可以重新进入", async () => {
  const { createInboundDeliveryDedupe } = await import("../src/bots/inboundDedupe.js");
  const dedupe = createInboundDeliveryDedupe();
  assert.equal(dedupe.mark(message(), 1000), true);
  dedupe.release(message());
  assert.equal(dedupe.mark(message(), 1001), true);
});

test("clear：dispose 后所有键失效", async () => {
  const { createInboundDeliveryDedupe } = await import("../src/bots/inboundDedupe.js");
  const dedupe = createInboundDeliveryDedupe();
  assert.equal(dedupe.mark(message(), 1000), true);
  dedupe.clear();
  assert.equal(dedupe.mark(message(), 1001), true);
});
