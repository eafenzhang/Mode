import assert from "node:assert/strict";
import test from "node:test";
import { botsConfigFileSchema, type BotConfig } from "@zcode/shared";

// 回归背景：BotConfig 新增字段（heartbeat / groupChat / wecomBotId …）时只加 TS 类型、
// 忘了登记 botConfigSchema，而 schema 是 .strict()——保存会直接抛
// ZodError("Unrecognized key")，UI 上表现为"开关点了没有任何效果"。
// 这个测试把"每个可选字段都能通过 strict schema 往返"固定下来。

test("botConfigSchema：包含全部可选字段的配置能通过严格校验且字段保真", () => {
  const fullBot: BotConfig = {
    id: "bot-full",
    name: "Full feature bot",
    provider: "wecom",
    enabled: true,
    credentialRef: "bot:cred:1",
    webhookSecretRef: "bot:whsec:1",
    webhookUrl: "https://example.com/hook",
    webhookAuthHeaderName: "x-zcode-bot-secret",
    feishuAppId: "cli_xxx",
    providerUserId: "ZhangCongCong",
    privateChatMode: "all_users",
    displayName: "Zhang",
    wecomBotId: "aibeaRfcjzJT",
    allowedWorkspaces: ["*"],
    allowedCommands: {
      status: true,
      new: true,
      workspace: true,
      model: true,
      mode: true,
      thoughtLevel: true,
      reply: true,
    },
    currentOptions: {},
    replyMode: "summary_changes",
    // 本次事故字段：漏登记即整次保存失败
    heartbeat: {
      enabled: true,
      intervalMinutes: 60,
      activeHours: { start: "09:00", end: "21:00" },
    },
    groupChat: { activation: "mention" },
  };

  const parsed = botsConfigFileSchema.parse({ version: 3, bots: [fullBot] });
  const roundTripped = parsed.bots[0]!;
  assert.deepEqual(roundTripped.heartbeat, fullBot.heartbeat);
  assert.deepEqual(roundTripped.groupChat, fullBot.groupChat);
  assert.equal(roundTripped.wecomBotId, "aibeaRfcjzJT");
  assert.equal(roundTripped.credentialRef, "bot:cred:1");
});

test("botConfigSchema：未知字段仍然被严格拒绝（strict 语义不回退）", () => {
  const result = botsConfigFileSchema.safeParse({
    version: 3,
    bots: [
      {
        id: "bot-x",
        name: "",
        provider: "telegram",
        enabled: true,
        allowedWorkspaces: ["*"],
        allowedCommands: {
          status: true,
          new: true,
          workspace: true,
          model: true,
          mode: true,
          thoughtLevel: true,
          reply: true,
        },
        currentOptions: {},
        replyMode: "assistant_changes",
        unknownFutureField: 1,
      },
    ],
  });
  assert.equal(result.success, false);
});
