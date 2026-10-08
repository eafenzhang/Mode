import assert from "node:assert/strict";
import test from "node:test";

// 对话级会话隔离：私聊按用户、群聊按群；一个 bot 可同时服务多个对话（各持一个桌面会话）。
// 这里覆盖键推导、v3→v4 状态迁移、以及服务层"每对话一个槽位"的源码契约。

test("对话键：私聊按用户、群聊按群，缺 chatId 时退回发送者", async () => {
  const {
    makeBotConversationKey,
    parseBotConversationKey,
    getBotActorConversation,
  } = await import("@mode/shared");

  assert.equal(makeBotConversationKey("private", "u1"), "private:u1");
  assert.equal(makeBotConversationKey("group", "oc_1"), "group:oc_1");
  assert.deepEqual(parseBotConversationKey("private:u1"), { kind: "private", id: "u1" });
  assert.deepEqual(parseBotConversationKey("group:oc_1"), { kind: "group", id: "oc_1" });
  assert.equal(parseBotConversationKey("legacy"), null);
  assert.equal(parseBotConversationKey("private:"), null);
  assert.equal(parseBotConversationKey("weird:u1"), null);

  assert.deepEqual(
    getBotActorConversation({ chatType: "private", providerUserId: "u1" }),
    { key: "private:u1", kind: "private", id: "u1" },
  );
  assert.deepEqual(
    getBotActorConversation({ chatType: "group", providerUserId: "u1", chatId: "oc_1" }),
    { key: "group:oc_1", kind: "group", id: "oc_1" },
  );
  // 群聊缺 chatId（部分平台首帧）时退回发送者，保证键稳定非空
  assert.deepEqual(
    getBotActorConversation({ chatType: "group", providerUserId: "u1" }),
    { key: "group:u1", kind: "group", id: "u1" },
  );
});

test("v3 → v4 迁移：上下文挂到可判定的对话上，通道级字段留在 bot 记录上", async () => {
  const { migrateBotsStateFileV3, BOTS_STATE_FILE_VERSION } = await import("@mode/shared");

  const v3 = {
    version: 3,
    bots: {
      "bot-with-private": {
        botId: "bot-with-private",
        workspacePath: "D:\\ws",
        workspaceIdentity: "D:\\ws",
        workspaceId: "D:\\ws",
        mode: "task",
        activeTaskId: "sess_1",
        lastPrivateUserId: "u9",
        telegramOffset: 42,
        lastHeartbeatAt: 111,
        updatedAt: 1000,
      },
      "bot-with-group": {
        botId: "bot-with-group",
        workspacePath: "D:\\ws2",
        mode: "draft",
        activeTaskId: null,
        queuedMessages: [{ text: "hi", receivedAt: 5, chatId: "oc_group" }],
        updatedAt: 1001,
      },
      "bot-no-hint": {
        botId: "bot-no-hint",
        workspacePath: "D:\\ws3",
        mode: "draft",
        activeTaskId: null,
        updatedAt: 1002,
      },
    },
  };

  const migrated = migrateBotsStateFileV3(v3);
  assert.equal(migrated.version, BOTS_STATE_FILE_VERSION);
  const withPrivate = migrated.bots["bot-with-private"];
  // 通道级字段保留在 bot 记录
  assert.equal(withPrivate.telegramOffset, 42);
  assert.equal(withPrivate.lastHeartbeatAt, 111);
  // 上下文挂到最近私聊用户的对话
  const privateContext = withPrivate.conversations["private:u9"];
  assert.equal(privateContext.activeTaskId, "sess_1");
  assert.equal(privateContext.workspacePath, "D:\\ws");
  assert.equal(privateContext.conversationKind, "private");

  const withGroup = migrated.bots["bot-with-group"];
  assert.equal(withGroup.conversations["group:oc_group"].conversationId, "oc_group");

  const noHint = migrated.bots["bot-no-hint"];
  assert.equal(noHint.conversations.legacy.conversationId, "legacy");
  assert.equal(noHint.conversations.legacy.workspacePath, "D:\\ws3");
});

test("schema：v3 文件可被解析（自动迁移），v4 原样通过，未知版本拒绝", async () => {
  const { botsStateFileSchema } = await import("@mode/shared");

  const v3 = {
    version: 3,
    bots: {
      bot1: {
        botId: "bot1",
        workspacePath: "D:\\ws",
        mode: "draft",
        activeTaskId: null,
        lastPrivateUserId: "u1",
        updatedAt: 1,
      },
    },
  };
  const parsedV3 = botsStateFileSchema.parse(v3);
  assert.equal(parsedV3.version, 4);
  assert.ok(parsedV3.bots.bot1.conversations["private:u1"]);

  const v4 = {
    version: 4,
    bots: {
      bot1: {
        botId: "bot1",
        conversations: {
          "group:oc": {
            botId: "bot1",
            conversationKey: "group:oc",
            conversationKind: "group",
            conversationId: "oc",
            workspacePath: "D:\\ws",
            mode: "draft",
            activeTaskId: null,
            updatedAt: 2,
          },
        },
        updatedAt: 2,
      },
    },
  };
  const parsedV4 = botsStateFileSchema.parse(v4);
  assert.equal(parsedV4.bots.bot1.conversations["group:oc"].conversationKey, "group:oc");

  assert.throws(() => botsStateFileSchema.parse({ version: 2, bots: {} }));
});

test("服务层契约：上下文按对话槽位读写，绑定/投影/镜像都带对话键", async () => {
  const source = await import("node:fs/promises").then((fs) =>
    fs.readFile(new URL("../src/bots/botsService.ts", import.meta.url), "utf8"),
  );

  // 1) readContext 按 actor 推导对话键
  assert.ok(
    source.includes("const conversation = getBotActorConversation(actor);") &&
      source.includes("state.bots[bot.id]?.conversations[conversation.key]"),
    "readContext 必须按对话键取上下文",
  );
  // 2) writeContext 只写该对话槽位，且旧会话订阅只在同对话内撤
  assert.ok(
    source.includes("const previous = channel.conversations[context.conversationKey];") &&
      source.includes("channel.conversations[context.conversationKey] = { ...context"),
    "writeContext 必须写对话槽位（其他对话不受影响）",
  );
  assert.ok(
    source.includes("ensureBotChannelState(state, context.botId)"),
    "writeContext 必须走通道状态 + 对话表结构",
  );
  // 3) 通道级状态不再伪造上下文
  assert.ok(
    source.includes("async function patchBotChannelState(") &&
      !/state\.bots\[botId\] = \{\s*botId:\s*botId,\s*workspacePath/u.test(source),
    "通道级写入不得再伪造对话上下文",
  );
  // 4) 会话绑定/解绑/投影按对话粒度
  assert.ok(source.includes("conversationKey?: string;"), "bindBotToTask 必须接受对话键");
  assert.ok(
    source.includes("listConversationsByTask(state, params.botId, params.taskId)"),
    "unbindBotFromTask 必须按对话解绑",
  );
  assert.ok(
    source.includes("async listBotConversations("),
    "必须提供候选对话列表供绑定菜单选择",
  );
  // 4.5) 桌面端发起回合也必须武装助手回复的镜像订阅：
  // 订阅在每个回合终态都会释放，只在 IM 入站路径武装会让 UI 发起的回复永远推不到 IM。
  const armBlock = source.slice(
    source.indexOf("async armConversationReplyMirror(params: {"),
    source.indexOf("async armConversationReplyMirror(params: {") + 2000,
  );
  assert.ok(
    armBlock.includes("await ensureContextStreamWatch(bot, context);"),
    "armConversationReplyMirror 必须按对话武装 ensureContextStreamWatch",
  );
  const notifyBlock = source.slice(
    source.indexOf("async notifyDesktopUserMessage(params: {"),
    source.indexOf("async notifyDesktopUserMessage(params: {") + 1400,
  );
  assert.ok(
    notifyBlock.includes("await this.armConversationReplyMirror(params);"),
    "提问回显路径也必须幂等武装同一订阅",
  );
  // 订阅只从武装那一刻起收事件：UI 必须在提示进入 Agent 之前 await 武装。
  const sessionPane = await import("node:fs/promises").then((fs) =>
    fs.readFile(new URL("../../../packages/ui/src/v4/SessionPane.tsx", import.meta.url), "utf8"),
  );
  const armIndex = sessionPane.indexOf("await botsService.armConversationReplyMirror({");
  assert.ok(armIndex > 0, "UI 发送路径缺少 armConversationReplyMirror");
  // 武装之后紧接着才是同一段里的 sendText 派发（同一函数体内）。
  const dispatchIndex = sessionPane.indexOf("dispatchSubmissionCommand(", armIndex);
  assert.ok(
    dispatchIndex > armIndex && dispatchIndex - armIndex < 800,
    "武装必须紧邻且早于 sendText 派发：晚于 ACK 会整轮漏收事件（短回合直接静默）",
  );

  // 5) 桌面镜像：谁持有会话就往谁的对话发
  assert.ok(
    source.includes("const mirrorUserId =") &&
      source.includes('context.conversationKind === "private"') &&
      source.includes("? context.conversationId"),
    "镜像必须按持有会话的对话投递",
  );
  // 6) 心跳目标：优先绑定会话的对话，其次最近私聊/群
  assert.ok(source.includes("function pickHeartbeatConversation("), "缺少心跳目标对话选择");
  // 7) 迁移细化：legacy 对话按绑定用户重映射，保住既有绑定
  assert.ok(
    source.includes("BOT_LEGACY_CONVERSATION_KEY") &&
      source.includes("async function rekeyLegacyConversations("),
    "缺少 legacy 对话重映射",
  );
});
