import assert from "node:assert/strict";
import test from "node:test";

// 群聊历史缓冲：未触发消息按会话暂存，作为下次被 @ 时的上下文；带上限与 LRU 淘汰。

test("append/drain：按会话隔离，drain 后清空", async () => {
  const { createGroupHistoryBuffer } = await import("../src/bots/groupHistory.js");
  const buffer = createGroupHistoryBuffer();
  buffer.append("bot-1", "chat-A", { senderName: "Zhang", text: "先跑一下构建" });
  buffer.append("bot-1", "chat-A", { text: "   " });
  buffer.append("bot-1", "chat-B", { text: "另一个群" });
  buffer.append("bot-2", "chat-A", { text: "另一个 bot" });

  const drained = buffer.drain("bot-1", "chat-A");
  assert.equal(drained.length, 1);
  assert.equal(drained[0].senderName, "Zhang");
  assert.equal(drained[0].text, "先跑一下构建");
  // drain 后为空；其他键不受影响
  assert.deepEqual(buffer.drain("bot-1", "chat-A"), []);
  assert.equal(buffer.drain("bot-1", "chat-B").length, 1);
  assert.equal(buffer.drain("bot-2", "chat-A").length, 1);
});

test("单群上限 30 条，超出丢弃最旧；长消息截断", async () => {
  const { createGroupHistoryBuffer, GROUP_HISTORY_MAX_PER_GROUP } = await import(
    "../src/bots/groupHistory.js"
  );
  const buffer = createGroupHistoryBuffer();
  for (let index = 0; index < GROUP_HISTORY_MAX_PER_GROUP + 5; index += 1) {
    buffer.append("bot-1", "chat-A", { text: `msg-${index}` });
  }
  const drained = buffer.drain("bot-1", "chat-A");
  assert.equal(drained.length, GROUP_HISTORY_MAX_PER_GROUP);
  assert.equal(drained[0].text, "msg-5");

  buffer.append("bot-1", "chat-A", { text: "x".repeat(500) });
  const [long] = buffer.drain("bot-1", "chat-A");
  assert.ok(long.text.length <= 201);
  assert.ok(long.text.endsWith("…"));
});

test("clear/clearBot：分别清空单群与该 bot 的全部群", async () => {
  const { createGroupHistoryBuffer } = await import("../src/bots/groupHistory.js");
  const buffer = createGroupHistoryBuffer();
  buffer.append("bot-1", "chat-A", { text: "a" });
  buffer.append("bot-1", "chat-B", { text: "b" });
  buffer.append("bot-2", "chat-A", { text: "c" });

  buffer.clear("bot-1", "chat-A");
  assert.equal(buffer.drain("bot-1", "chat-A").length, 0);
  assert.equal(buffer.drain("bot-1", "chat-B").length, 1);

  buffer.clearBot("bot-1");
  assert.equal(buffer.drain("bot-1", "chat-B").length, 0);
  assert.equal(buffer.drain("bot-2", "chat-A").length, 1);
});

test("formatGroupHistoryContext：无历史返回 null；中英渲染带首尾标记与 [from: 名字 时间]", async () => {
  const { formatGroupHistoryContext } = await import("../src/bots/groupHistory.js");
  assert.equal(formatGroupHistoryContext([], undefined), null);

  const at = new Date(2026, 0, 2, 3, 4, 5).getTime();
  const entries = [
    { senderName: "Zhang", text: "改一下登录逻辑", at },
    { text: "收到", at },
  ];
  const zh = formatGroupHistoryContext(entries, "zh-CN");
  assert.ok(zh?.includes("群聊记录"));
  assert.ok(zh?.includes("[from: Zhang 2026-01-02 03:04:05] 改一下登录逻辑"));
  assert.ok(zh?.includes("[from: 未知成员 2026-01-02 03:04:05] 收到"), "缺昵称要有兜底");
  assert.ok(zh?.includes("当前消息"));

  const en = formatGroupHistoryContext(entries, "en-US");
  assert.ok(en?.includes("group chat history"));
  assert.ok(en?.includes("[from: Zhang 2026-01-02 03:04:05] 改一下登录逻辑"));
  assert.ok(en?.includes("[from: unknown member "));
});

// 源码契约：群聊门控的三个安全不变量（行为测试需要整套服务图，成本过高）。
//   1. 未绑定/非绑定成员的消息必须静默忽略（而不是回"仅支持私聊"）；
//   2. mention 模式未被 @ 时静默忽略；
//   3. 群聊限制管理类命令。
const botsServiceSource = await import("node:fs/promises").then((fs) =>
  fs.readFile(new URL("../src/bots/botsService.ts", import.meta.url), "utf8").catch(() => null),
);

test("群聊门控不变量：静默忽略非绑定/未 @，并限制管理命令", {
  skip: !botsServiceSource && "无法读取 botsService 源码",
}, () => {
  const source = botsServiceSource;
  if (!source) return;

  const gateIndex = source.indexOf("const activation = bot.groupChat?.activation");
  assert.ok(gateIndex >= 0, "未找到群聊激活门控");
  const gate = source.slice(gateIndex, gateIndex + 1600);
  assert.ok(gate.includes("isBoundSender"), "缺少绑定成员校验");
  assert.ok(gate.includes('reply: []'), "非绑定成员必须静默忽略");
  assert.ok(
    gate.includes('activation === "mention" && message.actor.isMention !== true'),
    "缺少 mention 模式判定",
  );
  // 历史缓冲与门控在同一分支内（未触发消息进入上下文）
  assert.ok(gate.includes("groupHistory.append"), "未触发消息必须记入群历史");

  const restrictIndex = source.indexOf('command.type !== "message" &&');
  assert.ok(restrictIndex >= 0, "缺少群聊命令限制");
  const restrict = source.slice(restrictIndex, restrictIndex + 900);
  assert.ok(restrict.includes('"groupCommandUnsupported"'), "群聊管理命令必须有引导文案");
});
// 群聊方式的选项由平台能力矩阵决定（与 MyAgents 的 groupActivation 对齐）：
//   能识别 @ → 给「@提及」；能收到未 @ 的消息 → 给「全部消息」；
//   企微只在被 @ 时下发群回调，因此只有 @提及（不能假装支持「全部消息」）。
test("群聊方式：@提及 / 全部消息 选项按平台能力矩阵提供", async () => {
  const { resolveBotGroupChatCapabilities, botProviderSupportsGroupMention } = await import(
    "@mode/shared"
  );
  for (const provider of ["telegram", "feishu", "lark", "dingtalk"]) {
    assert.deepEqual(
      resolveBotGroupChatCapabilities(provider),
      { mention: true, always: true },
      provider + " 应同时提供 @提及 与 全部消息",
    );
  }
  // 企微：平台只在被 @ 时下发回调 → 只有 mention，没有 always
  assert.deepEqual(resolveBotGroupChatCapabilities("wecom"), { mention: true, always: false });
  assert.equal(botProviderSupportsGroupMention("wecom"), true);
  for (const provider of ["weixin", "webhook", "discord"]) {
    assert.deepEqual(
      resolveBotGroupChatCapabilities(provider),
      { mention: false, always: true },
      provider + " 无法识别 @，只能「全部消息」",
    );
  }
  // AstrBot 桥接已下线：字面量只为解析历史配置保留，不提供任何群聊模式
  assert.deepEqual(resolveBotGroupChatCapabilities("astrbot"), {
    mention: false,
    always: false,
  });
  // 兼容入口仍然只表达"能否识别 @"
  assert.equal(botProviderSupportsGroupMention("telegram"), true);
  assert.equal(botProviderSupportsGroupMention("weixin"), false);

  const card = await import("node:fs/promises").then((fs) =>
    fs.readFile(new URL("../../../packages/ui/src/BotsDialog/BotGroupChatCard.tsx", import.meta.url), "utf8"),
  );
  assert.ok(
    card.includes("capabilities.mention ? (") && card.includes('value="mention"'),
    "卡片必须按平台能力决定 mention 选项",
  );
  assert.ok(
    card.includes("capabilities.always ? (") && card.includes('value="always"'),
    "卡片必须按平台能力决定「全部消息」选项",
  );
  assert.ok(
    card.includes('configured === "always" ? "mention" : configured'),
    "平台无 always 语义时显示层必须收敛到 mention（企微存量配置）",
  );
});

