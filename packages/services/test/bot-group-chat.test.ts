import assert from "node:assert/strict";
import test from "node:test";

// 群聊历史缓冲：未触发消息按会话暂存，作为下次被 @ 时的上下文；带上限与 LRU 淘汰。

test("append/snapshot：按会话隔离，读取后不清空（旧 drain 语义已废止）", async () => {
  const { createGroupHistoryBuffer } = await import("../src/bots/groupHistory.js");
  const buffer = createGroupHistoryBuffer();
  buffer.append("bot-1", "chat-A", { senderName: "Zhang", text: "先跑一下构建" });
  buffer.append("bot-1", "chat-A", { text: "   " });
  buffer.append("bot-1", "chat-B", { text: "另一个群" });
  buffer.append("bot-2", "chat-A", { text: "另一个 bot" });

  const first = buffer.snapshot("bot-1", "chat-A");
  assert.equal(first.length, 1);
  assert.equal(first[0].senderName, "Zhang");
  assert.equal(first[0].text, "先跑一下构建");
  // G7 的核心变更：快照不清空，同一群下次触发仍能看到之前的内容
  assert.equal(buffer.snapshot("bot-1", "chat-A").length, 1);
  assert.equal(buffer.snapshot("bot-1", "chat-B").length, 1);
  assert.equal(buffer.snapshot("bot-2", "chat-A").length, 1);
});

test("单群上限 30 条，超出丢弃最旧；长消息截断", async () => {
  const { createGroupHistoryBuffer, GROUP_HISTORY_MAX_PER_GROUP } =
    await import("../src/bots/groupHistory.js");
  const buffer = createGroupHistoryBuffer();
  for (let index = 0; index < GROUP_HISTORY_MAX_PER_GROUP + 5; index += 1) {
    buffer.append("bot-1", "chat-A", { text: `msg-${index}` });
  }
  const entries = buffer.snapshot("bot-1", "chat-A");
  assert.equal(entries.length, GROUP_HISTORY_MAX_PER_GROUP);
  assert.equal(entries[0].text, "msg-5");

  buffer.append("bot-1", "chat-A", { text: "x".repeat(500) });
  const [long] = buffer.snapshot("bot-1", "chat-A").slice(-1);
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
  assert.equal(buffer.snapshot("bot-1", "chat-A").length, 0);
  assert.equal(buffer.snapshot("bot-1", "chat-B").length, 1);

  buffer.clearBot("bot-1");
  assert.equal(buffer.snapshot("bot-1", "chat-B").length, 0);
  assert.equal(buffer.snapshot("bot-2", "chat-A").length, 1);
});

test("hydrate/serialize：跨重启恢复，幂等，非法快照按没有历史处理", async () => {
  const { createGroupHistoryBuffer } = await import("../src/bots/groupHistory.js");
  const source = createGroupHistoryBuffer();
  source.append("bot-1", "chat-A", { senderName: "Zhang", text: "重启前的群聊" });
  const snapshot = source.serialize();
  assert.equal(snapshot.version, 1);
  assert.equal(snapshot.bots["bot-1"]?.["chat-A"]?.length, 1);

  const restored = createGroupHistoryBuffer();
  restored.hydrate(snapshot);
  assert.equal(restored.snapshot("bot-1", "chat-A")[0]?.text, "重启前的群聊");

  // 幂等：第二次 hydrate 被忽略（不会覆盖恢复后新写入的内容）
  restored.append("bot-1", "chat-A", { text: "恢复之后的新消息" });
  restored.hydrate(snapshot);
  assert.equal(restored.snapshot("bot-1", "chat-A").length, 2);

  // 非法快照：版本不符 / 条目缺字段 / null 都不应凭空造出历史
  const invalid = createGroupHistoryBuffer();
  invalid.hydrate({ version: 2, bots: { "bot-1": { "chat-A": [{ text: "x", at: 1 }] } } } as never);
  invalid.hydrate(null);
  assert.equal(invalid.size(), 0);

  const partiallyBad = createGroupHistoryBuffer();
  partiallyBad.hydrate({
    version: 1,
    bots: { "bot-1": { "chat-A": [{ text: "有效", at: 1 }, { text: "缺时间戳" }, null] } },
  } as never);
  const kept = partiallyBad.snapshot("bot-1", "chat-A");
  assert.equal(kept.length, 1);
  assert.equal(kept[0]?.text, "有效");
  // 恢复时同样套用条数上限
  const overflow = createGroupHistoryBuffer();
  overflow.hydrate({
    version: 1,
    bots: {
      "bot-1": {
        "chat-A": Array.from({ length: 40 }, (_, index) => ({ text: `m-${index}`, at: index })),
      },
    },
  });
  assert.equal(overflow.snapshot("bot-1", "chat-A").length, 30);
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

test(
  "群聊门控不变量：静默忽略非绑定/未 @，并限制管理命令",
  {
    skip: !botsServiceSource && "无法读取 botsService 源码",
  },
  () => {
    const source = botsServiceSource;
    if (!source) return;

    const gateIndex = source.indexOf("const activation = bot.groupChat?.activation");
    assert.ok(gateIndex >= 0, "未找到群聊激活门控");
    // 窗口只需覆盖「群门控分支」内的判定标记：Task 4 在同一分支内插入了
    // free 模式免批登记（spec bot-access-mode R3，约 1000 字符），
    // 把 mention 判定推到 2400 之外，窗口随之放宽，断言语义不变。
    const gate = source.slice(gateIndex, gateIndex + 4000);
    assert.ok(gate.includes("approved"), "缺少群授权判定");
    assert.ok(gate.includes("isAllowedSender"), "缺少白名单发送者判定");
    assert.ok(gate.includes("reply: []"), "未授权群消息必须静默忽略");
    assert.ok(
      gate.includes('activation === "mention" && message.actor.isMention !== true'),
      "缺少 mention 模式判定",
    );
    // 历史缓冲与门控在同一分支内（未触发消息进入上下文）
    assert.ok(gate.includes("groupHistory.append"), "未触发消息必须记入群历史");

    const restrictIndex = source.indexOf('command.type !== "message" &&');
    assert.ok(restrictIndex >= 0, "缺少群聊命令限制");
    const restrict = source.slice(restrictIndex, restrictIndex + 900);
    assert.ok(restrict.includes("return [];"), "群聊管理命令必须静默丢弃");
    assert.ok(!restrict.includes("groupCommandUnsupported"), "群聊不应再回管理命令提示");
  },
);
// 群聊方式的选项由平台能力矩阵决定（与 MyAgents 的 groupActivation 对齐）：
//   能识别 @ → 给「@提及」；能收到未 @ 的消息 → 给「全部消息」；
//   企微只在被 @ 时下发群回调，因此只有 @提及（不能假装支持「全部消息」）。
test("群聊方式：@提及 / 全部消息 选项按平台能力矩阵提供", async () => {
  const { resolveBotGroupChatCapabilities, botProviderSupportsGroupMention } =
    await import("@mode/shared");
  for (const provider of ["feishu", "lark"]) {
    assert.deepEqual(
      resolveBotGroupChatCapabilities(provider),
      { mention: true, always: true },
      provider + " 应同时提供 @提及 与 全部消息",
    );
  }
  // 企微/钉钉：平台只在被 @ 时下发回调 → 只有 mention，没有 always
  for (const provider of ["wecom", "dingtalk"]) {
    assert.deepEqual(
      resolveBotGroupChatCapabilities(provider),
      { mention: true, always: false },
      provider + " 只有 mention 语义",
    );
    assert.equal(botProviderSupportsGroupMention(provider), true);
  }
  // 微信按 A+ 只保留私聊；其余历史 provider 不进入产品面。
  for (const provider of ["weixin", "telegram", "webhook", "discord", "astrbot"] as const) {
    assert.deepEqual(
      resolveBotGroupChatCapabilities(provider),
      { mention: false, always: false },
      provider + " 不提供群聊模式",
    );
  }
  assert.equal(botProviderSupportsGroupMention("weixin"), false);

  const card = await import("node:fs/promises").then((fs) =>
    fs.readFile(
      new URL("../../../packages/ui/src/BotsDialog/BotGroupChatCard.tsx", import.meta.url),
      "utf8",
    ),
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
