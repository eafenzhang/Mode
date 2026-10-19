import assert from "node:assert/strict";
import test from "node:test";
import type { BotConfig, BotsStateFile } from "@mode/shared";

// 微信 providerUserId 误写拆分（docs/specs/weixin-provider-self-id.md）：
// 扫码注册把上游 ilink_bot_id（机器人自身 id）写进了语义为"绑定用户"的 providerUserId，
// 连锁产出幻影私聊预判、心跳发给自己、会话绑到幻影对话。修复 = 新字段 providerSelfId
// + 启动迁移（挪字段 + 删幻影对话）+ 出站兼容回退。

function makeWeixinBot(overrides: Partial<BotConfig> = {}): BotConfig {
  return {
    id: "bot-1",
    name: "",
    provider: "weixin",
    enabled: true,
    allowedWorkspaces: ["D:\\ws"],
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
    ...overrides,
  } as BotConfig;
}

function makeState(conversations: Record<string, Record<string, unknown>>): BotsStateFile {
  return {
    version: 4,
    bots: {
      "bot-1": {
        botId: "bot-1",
        conversations: conversations as BotsStateFile["bots"][string]["conversations"],
        updatedAt: 1,
      },
    },
  } as BotsStateFile;
}

test("schema：botConfigSchema 接受 providerSelfId（strict 不再抛 Unrecognized key）", async () => {
  const { botConfigSchema } = await import("@mode/shared");
  const parsed = botConfigSchema.parse(makeWeixinBot({ providerSelfId: "8b43889be073@im.bot" }));
  assert.equal(parsed.providerSelfId, "8b43889be073@im.bot");
});

test("迁移规划：注册双写（providerUserId === displayName）挪进 providerSelfId 并删幻影对话", async () => {
  const { planWeixinSelfIdMigration } = await import("../src/bots/weixinSelfId.js");
  const bot = makeWeixinBot({
    providerUserId: "8b43889be073@im.bot",
    displayName: "8b43889be073@im.bot",
  });
  const state = makeState({
    "private:8b43889be073@im.bot": {
      botId: "bot-1",
      conversationKey: "private:8b43889be073@im.bot",
      conversationKind: "private",
      conversationId: "8b43889be073@im.bot",
      workspacePath: "D:\\ws",
      mode: "task",
      activeTaskId: "sess_1",
      updatedAt: 1,
    },
    "private:o9cq80yCfHT3uR9T3VNctmmH5Ldc@im.wechat": {
      botId: "bot-1",
      conversationKey: "private:o9cq80yCfHT3uR9T3VNctmmH5Ldc@im.wechat",
      conversationKind: "private",
      conversationId: "o9cq80yCfHT3uR9T3VNctmmH5Ldc@im.wechat",
      workspacePath: "D:\\ws",
      mode: "task",
      activeTaskId: "sess_2",
      updatedAt: 2,
    },
  });

  const plan = planWeixinSelfIdMigration([bot], state);

  assert.equal(plan.changed, true);
  const migrated = plan.bots.find((item) => item.id === "bot-1")!;
  assert.equal(migrated.providerSelfId, "8b43889be073@im.bot");
  assert.equal(migrated.providerUserId, undefined);
  // 幻影对话（自身 id）进删除清单，真实对话绝不出现
  assert.deepEqual(plan.removedConversations, [
    { botId: "bot-1", conversationKey: "private:8b43889be073@im.bot" },
  ]);
});

test("迁移规划：@im.bot 后缀判别（displayName 不同）同样迁移", async () => {
  const { planWeixinSelfIdMigration } = await import("../src/bots/weixinSelfId.js");
  const bot = makeWeixinBot({
    providerUserId: "8b43889be073@im.bot",
    displayName: "我的机器人",
  });
  const plan = planWeixinSelfIdMigration([bot], makeState({}));
  assert.equal(plan.changed, true);
  assert.equal(plan.bots[0]!.providerSelfId, "8b43889be073@im.bot");
  assert.equal(plan.bots[0]!.providerUserId, undefined);
});

test("迁移规划：/bind 写入的真实用户 id 不迁移，微信无 providerUserId 不迁移，幂等无操作", async () => {
  const { planWeixinSelfIdMigration } = await import("../src/bots/weixinSelfId.js");
  // 真实用户（两个判别都不命中）：保留绑定用户语义
  const bound = makeWeixinBot({
    providerUserId: "o9cq80yCfHT3uR9T3VNctmmH5Ldc@im.wechat",
    displayName: "张三",
  });
  const boundPlan = planWeixinSelfIdMigration([bound], makeState({}));
  assert.equal(boundPlan.changed, false);
  assert.equal(boundPlan.bots[0]!.providerUserId, "o9cq80yCfHT3uR9T3VNctmmH5Ldc@im.wechat");
  assert.deepEqual(boundPlan.removedConversations, []);

  // 无 providerUserId：无事可做
  const empty = makeWeixinBot();
  const emptyPlan = planWeixinSelfIdMigration([empty], makeState({}));
  assert.equal(emptyPlan.changed, false);

  // 已迁移（providerSelfId 已有值）：幂等
  const migrated = makeWeixinBot({
    providerSelfId: "8b43889be073@im.bot",
    displayName: "8b43889be073@im.bot",
  });
  const migratedPlan = planWeixinSelfIdMigration([migrated], makeState({}));
  assert.equal(migratedPlan.changed, false);
  assert.deepEqual(migratedPlan.removedConversations, []);
});

test("迁移规划：非微信 provider 的 providerUserId 永不触碰", async () => {
  const { planWeixinSelfIdMigration } = await import("../src/bots/weixinSelfId.js");
  const dingtalk = makeWeixinBot({
    provider: "dingtalk",
    providerUserId: "u1",
    displayName: "u1",
  });
  const plan = planWeixinSelfIdMigration([dingtalk], makeState({}));
  assert.equal(plan.changed, false);
  assert.equal(plan.bots[0]!.providerUserId, "u1");
});

test("出站兼容：微信 send 的 from_user_id 用 providerSelfId 并回退 providerUserId", async () => {
  const source = await import("node:fs/promises").then((fs) =>
    fs.readFile(new URL("../src/bots/providers/weixinProvider.ts", import.meta.url), "utf8"),
  );
  const fromLine = source.split("\n").find((line) => line.includes("from_user_id: bot"));
  assert.ok(fromLine, "weixin send 缺少 from_user_id 赋值");
  assert.ok(
    fromLine.includes("providerSelfId"),
    "from_user_id 必须优先用 providerSelfId（存量未迁移配置回退 providerUserId）",
  );
  assert.ok(
    fromLine.includes("providerUserId"),
    "from_user_id 必须保留 providerUserId 回退（未迁移配置不断流）",
  );
});

test("心跳目标：resolveBotProactiveUserId 私聊对话回落早于 privateChatMode 判断", async () => {
  const source = await import("node:fs/promises").then((fs) =>
    fs.readFile(new URL("../src/bots/botsService.ts", import.meta.url), "utf8"),
  );
  const lines = source.split("\n");
  const fnStart = lines.findIndex((line) => line.includes("function resolveBotProactiveUserId"));
  assert.ok(fnStart >= 0, "未找到 resolveBotProactiveUserId");
  const fnBody = lines.slice(fnStart, fnStart + 40).join("\n");

  const privateFallbackIndex = fnBody.indexOf("conversation.conversationId");
  const modeIndex = fnBody.indexOf("privateChatMode");
  assert.ok(privateFallbackIndex >= 0, "缺少私聊对话回落");
  assert.ok(modeIndex >= 0, "缺少 privateChatMode 分支");
  assert.ok(
    privateFallbackIndex < modeIndex,
    "私聊回落必须在 privateChatMode 判断之前（迁移后微信 boundUserId 为空仍能拿到真实对端）",
  );
});

test("重复校验：validateBotConfig 对 providerSelfId 做同 provider 去重", async () => {
  const source = await import("node:fs/promises").then((fs) =>
    fs.readFile(new URL("../src/bots/botsService.ts", import.meta.url), "utf8"),
  );
  const lines = source.split("\n");
  const fnStart = lines.findIndex((line) => line.includes("function validateBotConfig"));
  assert.ok(fnStart >= 0, "未找到 validateBotConfig");
  const fnBody = lines.slice(fnStart, fnStart + 30).join("\n");
  assert.ok(
    fnBody.includes("providerSelfId"),
    "去重校验必须覆盖 providerSelfId（迁移后 providerUserId 为空，保护不能丢）",
  );
});

test("媒体投递：deliverBotMedia 私聊目标链 chatId → providerUserId → 对话 id", async () => {
  const source = await import("node:fs/promises").then((fs) =>
    fs.readFile(new URL("../src/bots/botsService.ts", import.meta.url), "utf8"),
  );
  const lines = source.split("\n");
  const targetIndex = lines.findIndex((line) =>
    line.includes("params.chatId?.trim() || params.bot.providerUserId"),
  );
  assert.ok(targetIndex >= 0, "未找到 deliverBotMedia 目标选择");
  // 目标链可能跨行（prettier 折行），在邻近窗口内断言回落
  const targetWindow = lines.slice(targetIndex, targetIndex + 3).join("\n");
  assert.ok(
    targetWindow.includes("conversation?.conversationId"),
    "目标链必须回落到对话 id（迁移后微信 providerUserId 为空，否则 /send 抛 no bound chat）",
  );
});
