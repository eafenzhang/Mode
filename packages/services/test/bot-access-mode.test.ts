import assert from "node:assert/strict";
import test from "node:test";

// 访问控制统一双模式（docs/specs/bot-access-mode.md R1/R5）：
// 1) 显式 accessMode 永远优先；
// 2) 无 accessMode 时按存量 privateChatMode 推导（all_users→free、bound_users→whitelist）；
// 3) 两者皆缺省时沿用旧 provider 缺省（微信 free、其余 whitelist）——存量不锁死；
// 4) 严格 schema 必须登记新字段，否则保存抛 ZodError。

test("resolveBotAccessMode：显式 accessMode 优先于 privateChatMode", async () => {
  const { resolveBotAccessMode } = await import("@mode/shared");
  assert.equal(
    resolveBotAccessMode({
      provider: "weixin",
      accessMode: "whitelist",
      privateChatMode: "all_users",
    }),
    "whitelist",
  );
  assert.equal(
    resolveBotAccessMode({ provider: "wecom", accessMode: "free", privateChatMode: "bound_users" }),
    "free",
  );
});

test("resolveBotAccessMode：无 accessMode 时从存量 privateChatMode 推导", async () => {
  const { resolveBotAccessMode } = await import("@mode/shared");
  assert.equal(resolveBotAccessMode({ provider: "wecom", privateChatMode: "all_users" }), "free");
  assert.equal(
    resolveBotAccessMode({ provider: "weixin", privateChatMode: "bound_users" }),
    "whitelist",
  );
});

test("resolveBotAccessMode：两者皆缺省沿用旧 provider 缺省（存量微信不锁死）", async () => {
  const { resolveBotAccessMode } = await import("@mode/shared");
  assert.equal(resolveBotAccessMode({ provider: "weixin" }), "free");
  assert.equal(resolveBotAccessMode({ provider: "wecom" }), "whitelist");
  assert.equal(resolveBotAccessMode({ provider: "feishu" }), "whitelist");
  assert.equal(resolveBotAccessMode({ provider: "dingtalk" }), "whitelist");
});

test("botConfigSchema：新增字段能严格解析并往返", async () => {
  const { botsConfigFileSchema } = await import("@mode/shared");
  const parsed = botsConfigFileSchema.parse({
    version: 3,
    bots: [
      {
        id: "bot-1",
        name: "b",
        provider: "wecom",
        enabled: true,
        accessMode: "free",
        blockedUsers: ["u-black"],
        blockedGroups: [
          { groupId: "g-black", groupName: "Bad group", platform: "wecom", blockedAt: 1 },
        ],
        userRegistry: [{ providerUserId: "u1", displayName: "Alice", discoveredAt: 2 }],
        allowedWorkspaces: ["*"],
        // 简报原样写的是 {}，但 botAllowedCommandsSchema 有 6 个必填布尔
        // （status/new/workspace/model/thoughtLevel/reply），缺一会先于本任务字段报错；
        // 这里补齐以聚焦本次新增的 4 个字段能否通过 strict 校验。
        allowedCommands: {
          status: true,
          new: true,
          workspace: true,
          model: true,
          thoughtLevel: true,
          reply: true,
        },
        currentOptions: {},
        replyMode: "assistant_changes",
      },
    ],
  });
  assert.equal(parsed.bots[0]?.accessMode, "free");
  assert.equal(parsed.bots[0]?.blockedUsers?.[0], "u-black");
  assert.equal(parsed.bots[0]?.blockedGroups?.[0]?.groupId, "g-black");
  assert.equal(parsed.bots[0]?.userRegistry?.[0]?.providerUserId, "u1");
});

test("findBoundUser：free 模式任何私聊用户放行，whitelist 按绑定收紧", async () => {
  const { findBoundUser } = await import("../src/bots/botConfigHelpers.js");
  const free = {
    id: "b1",
    provider: "wecom",
    providerUserId: "owner",
    allowedUsers: ["t1"],
    accessMode: "free",
  } as never;
  const whitelist = { ...free, accessMode: "whitelist" } as never;
  const actor = (providerUserId: string) =>
    ({ provider: "wecom", botId: "b1", providerUserId, chatType: "private" }) as never;
  assert.ok(findBoundUser(free, actor("stranger")), "free 模式陌生私聊必须放行");
  assert.equal(findBoundUser(whitelist, actor("stranger")), null, "whitelist 必须拒绝陌生私聊");
  assert.ok(findBoundUser(whitelist, actor("t1")), "whitelist 放行 allowedUsers");
});

test("黑名单纯函数：按 bot 配置精确命中，缺席不误伤", async () => {
  const { isUserBlocked, isGroupBlocked } = await import("../src/bots/botConfigHelpers.js");
  const bot = {
    id: "b1",
    provider: "wecom",
    blockedUsers: ["u-black"],
    blockedGroups: [{ groupId: "g-black", platform: "wecom", blockedAt: 1 }],
  } as never;
  assert.equal(isUserBlocked(bot, "u-black"), true);
  assert.equal(isUserBlocked(bot, "u-ok"), false);
  assert.equal(isGroupBlocked(bot, "g-black"), true);
  assert.equal(isGroupBlocked(bot, "g-ok"), false);
  assert.equal(
    isUserBlocked({ id: "b1", provider: "wecom" } as never, "u-any"),
    false,
    "无字段视为未拉黑",
  );
});

test("normalizeBotConfig：回填 accessMode（存量推导不锁死，显式值原样保留）", async () => {
  const { normalizeBotConfig } = await import("../src/bots/botConfigHelpers.js");
  const base = {
    id: "b1",
    name: "b",
    provider: "weixin",
    enabled: true,
    allowedWorkspaces: ["*"],
    // 简报原样写的是 {}，但 botAllowedCommandsSchema 有 6 个必填布尔
    // （status/new/workspace/model/thoughtLevel/reply）；此处补齐以与上方 schema 用例一致。
    allowedCommands: {
      status: true,
      new: true,
      workspace: true,
      model: true,
      thoughtLevel: true,
      reply: true,
    },
    currentOptions: {},
    replyMode: "assistant_changes",
  } as never;
  assert.equal(normalizeBotConfig(base).accessMode, "free", "存量微信无字段推导 free");
  assert.equal(
    normalizeBotConfig({ ...base, accessMode: "whitelist" } as never).accessMode,
    "whitelist",
    "显式值原样保留",
  );
  assert.equal(
    normalizeBotConfig({ ...base, provider: "wecom", privateChatMode: "all_users" } as never)
      .accessMode,
    "free",
    "存量 all_users 推导 free",
  );
});
