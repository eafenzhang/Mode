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
