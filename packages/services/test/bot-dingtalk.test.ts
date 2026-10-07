import assert from "node:assert/strict";
import test from "node:test";

// 钉钉适配器（对齐 MyAgents 的 dingtalk）：回调解析、会话 id 约定与发送目标判定。
// 这些是消息流转链路的契约点——字段读错会表现为"消息收不到/回错会话"。

test("parseDingtalkBotMessage：单聊文本消息（conversationType=1）", async () => {
  const { parseDingtalkBotMessage } = await import("../src/bots/providers/dingtalkProvider.js");
  const message = parseDingtalkBotMessage("bot-1", {
    msgId: "msg-1",
    msgtype: "text",
    text: { content: "  你好  " },
    senderStaffId: "staff-1",
    senderNick: "张三",
    conversationType: "1",
  });
  assert.ok(message);
  assert.equal(message.text, "你好");
  assert.equal(message.actor.provider, "dingtalk");
  assert.equal(message.actor.chatType, "private");
  assert.equal(message.actor.providerUserId, "staff-1");
  assert.equal(message.actor.chatId, "staff-1");
  assert.equal(message.actor.displayName, "张三");
  assert.equal(message.actor.isMention, true);
});

test("parseDingtalkBotMessage：群聊消息带 group: 前缀与 isInAtList（bool / 字符串两种形态）", async () => {
  const { parseDingtalkBotMessage } = await import("../src/bots/providers/dingtalkProvider.js");
  const group = parseDingtalkBotMessage("bot-1", {
    msgId: "msg-2",
    msgtype: "text",
    text: { content: "@机器人 跑一下测试" },
    senderStaffId: "staff-2",
    conversationType: "2",
    conversationId: "cid-abc",
    isInAtList: "true",
  });
  assert.ok(group);
  assert.equal(group.actor.chatType, "group");
  // 会话 id 约定与 MyAgents 一致：群聊 = group:{openConversationId}
  assert.equal(group.actor.chatId, "group:cid-abc");
  // providerUserId 保留发送者身份（授权用），回复目标由 chatId 表达
  assert.equal(group.actor.providerUserId, "staff-2");
  assert.equal(group.actor.isMention, true);

  const notMentioned = parseDingtalkBotMessage("bot-1", {
    msgId: "msg-3",
    msgtype: "text",
    text: { content: "没 @ 机器人" },
    senderStaffId: "staff-2",
    conversationType: "2",
    conversationId: "cid-abc",
    isInAtList: false,
  });
  assert.equal(notMentioned?.actor.isMention, false);
});

test("parseDingtalkBotMessage：richText 取纯文本；未知类型给占位；空内容丢弃", async () => {
  const { parseDingtalkBotMessage } = await import("../src/bots/providers/dingtalkProvider.js");
  const rich = parseDingtalkBotMessage("bot-1", {
    msgId: "msg-4",
    msgtype: "richText",
    content: { richText: [{ text: "前" }, { text: "后" }] },
    senderStaffId: "staff-1",
    conversationType: "1",
  });
  assert.equal(rich?.text, "前后");

  const unknown = parseDingtalkBotMessage("bot-1", {
    msgId: "msg-5",
    msgtype: "picture",
    senderStaffId: "staff-1",
    conversationType: "1",
  });
  assert.equal(unknown?.text, "[不支持的消息类型: picture]");

  assert.equal(
    parseDingtalkBotMessage("bot-1", {
      msgId: "msg-6",
      msgtype: "text",
      text: { content: "   " },
      senderStaffId: "staff-1",
      conversationType: "1",
    }),
    null,
  );
  assert.equal(parseDingtalkBotMessage("bot-1", { msgtype: "text" }), null);
});

test("resolveDingtalkSendTarget：group: 前缀 → 群 API 目标；其余按单聊 staffId", async () => {
  const { resolveDingtalkSendTarget } = await import(
    "../src/bots/providers/dingtalkProvider.js"
  );
  assert.deepEqual(resolveDingtalkSendTarget("group:cid-abc"), {
    kind: "group",
    target: "cid-abc",
  });
  assert.deepEqual(resolveDingtalkSendTarget("staff-1"), {
    kind: "private",
    target: "staff-1",
  });
});

test("钉钉消息分片：超过 20000 字符按边界分片", async () => {
  const { DINGTALK_MESSAGE_LIMIT } = await import("../src/bots/providers/dingtalkProvider.js");
  const { splitBotText } = await import("../src/bots/botText.js");
  assert.equal(DINGTALK_MESSAGE_LIMIT, 20_000);
  const long = Array.from({ length: 30 }, () => "y".repeat(1_000)).join("\n\n");
  const chunks = splitBotText(long, DINGTALK_MESSAGE_LIMIT);
  assert.ok(chunks.length >= 2);
  assert.ok(chunks.every((chunk) => chunk.length <= DINGTALK_MESSAGE_LIMIT));
});

// 对齐 MyAgents 的三项优化：
// 1) allowedUsers 白名单（额外授权用户可驱动机器人）
// 2) 钉钉 AI 卡片门槛（开关 + 模板 ID 同时具备才走卡片流式）
test("isDingtalkAiCardEnabled：开关与模板 ID 缺一不可", async () => {
  const { isDingtalkAiCardEnabled } = await import(
    "../src/bots/providers/dingtalkProvider.js"
  );
  assert.equal(isDingtalkAiCardEnabled({}), false);
  assert.equal(isDingtalkAiCardEnabled({ dingtalkUseAiCard: true }), false);
  assert.equal(isDingtalkAiCardEnabled({ dingtalkCardTemplateId: "tpl-1" }), false);
  assert.equal(isDingtalkAiCardEnabled({ dingtalkUseAiCard: false, dingtalkCardTemplateId: "tpl-1" }), false);
  assert.equal(isDingtalkAiCardEnabled({ dingtalkUseAiCard: true, dingtalkCardTemplateId: "  " }), false);
  assert.equal(isDingtalkAiCardEnabled({ dingtalkUseAiCard: true, dingtalkCardTemplateId: "tpl-1" }), true);
});

test("findBoundUser：绑定用户与 allowedUsers 白名单都放行，未授权用户拒绝", async () => {
  const { findBoundUser } = await import("../src/bots/botConfigHelpers.js");
  const bot = {
    id: "bot-1",
    provider: "dingtalk",
    providerUserId: "owner-1",
    allowedUsers: ["teammate-2", "teammate-3"],
  } as Parameters<typeof findBoundUser>[0];
  const actor = (providerUserId: string) =>
    ({ provider: "dingtalk", botId: "bot-1", providerUserId, chatType: "private" }) as Parameters<
      typeof findBoundUser
    >[1];

  assert.ok(findBoundUser(bot, actor("owner-1")), "绑定用户必须放行");
  assert.ok(findBoundUser(bot, actor("teammate-2")), "白名单用户必须放行");
  assert.equal(findBoundUser(bot, actor("stranger-9")), null, "未授权用户必须拒绝");

  // 无白名单时保持既有语义
  const soloBot = { ...bot, allowedUsers: undefined } as Parameters<typeof findBoundUser>[0];
  assert.ok(findBoundUser(soloBot, actor("owner-1")));
  assert.equal(findBoundUser(soloBot, actor("teammate-2")), null);
});
