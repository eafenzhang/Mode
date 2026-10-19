import assert from "node:assert/strict";
import test from "node:test";

// A+ 私聊准入：微信与企微/飞书/钉钉共用同一套规则。
// 1) findAuthorizedBot 只按 botId + provider 定位，不再按发送者提前拒绝；
// 2) findBoundUser 不再给微信无条件放行；
// 3) allowedUsers、绑定用户、all_users 三种放行方式对所有平台一致。

test("findAuthorizedBot：按 botId/provider 定位，未绑定发送者也能进入后续闸门", async () => {
  const { findAuthorizedBot } = await import("../src/bots/botConfigHelpers.js");
  const config = {
    version: 3,
    bots: [
      { id: "weixin-1", provider: "weixin", enabled: true, name: "wx" },
      { id: "wecom-1", provider: "wecom", enabled: true, name: "wecom" },
    ],
  } as never;

  for (const provider of ["weixin", "wecom"] as const) {
    const bot = findAuthorizedBot(config, {
      provider,
      botId: `${provider}-1`,
      providerUserId: "stranger",
      chatType: "private",
    });
    assert.ok(bot, `${provider} 未绑定发送者也必须能定位到 bot`);
  }
});

test("findBoundUser：微信不再绕过绑定/白名单", async () => {
  const { findBoundUser } = await import("../src/bots/botConfigHelpers.js");
  const bot = {
    id: "weixin-1",
    provider: "weixin",
    providerUserId: "owner-1",
    allowedUsers: ["teammate-1"],
    privateChatMode: "bound_users",
  } as never;
  const actor = (providerUserId: string, chatType: "private" | "group" = "private") =>
    ({
      provider: "weixin",
      botId: "weixin-1",
      providerUserId,
      chatType,
    }) as never;

  assert.ok(findBoundUser(bot, actor("owner-1")), "绑定用户必须放行");
  assert.ok(findBoundUser(bot, actor("teammate-1")), "allowedUsers 必须放行");
  assert.equal(findBoundUser(bot, actor("stranger-9")), null, "陌生用户必须拒绝");
});

test("findBoundUser：all_users 只对私聊放行，群聊仍由群授权闸门决定", async () => {
  const { findBoundUser } = await import("../src/bots/botConfigHelpers.js");
  const bot = {
    id: "wecom-1",
    provider: "wecom",
    privateChatMode: "all_users",
  } as never;
  const actor = (chatType: "private" | "group") =>
    ({
      provider: "wecom",
      botId: "wecom-1",
      providerUserId: "anyone",
      chatType,
    }) as never;

  assert.ok(findBoundUser(bot, actor("private")), "all_users 私聊放行");
  assert.equal(findBoundUser(bot, actor("group")), null, "群聊不能借 all_users 跳过群授权");
});
