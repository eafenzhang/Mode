import assert from "node:assert/strict";
import test from "node:test";

// 群聊回合装配与"沉默回执"判定：规则块/发送者标记/工具禁用/NO_REPLY 归一化。
// 参考 MyAgents 的群聊上下文注入（规则块 + 历史 + [from: 名字 时间] + @ 标记）。

test("isGroupSilenceReply：只认 <NO_REPLY> / NO_REPLY（含 markdown 与空白归一化）", async () => {
  const { isGroupSilenceReply } = await import("../src/bots/groupPrompt.js");
  assert.equal(isGroupSilenceReply("<NO_REPLY>"), true);
  assert.equal(isGroupSilenceReply("NO_REPLY"), true);
  assert.equal(isGroupSilenceReply("  <NO_REPLY>\n"), true);
  assert.equal(isGroupSilenceReply("**`NO_REPLY`**"), true);
  assert.equal(isGroupSilenceReply("no_reply"), true);
  // 正文里提到 NO_REPLY 不算沉默
  assert.equal(isGroupSilenceReply("我会输出 <NO_REPLY> 表示不回复"), false);
  assert.equal(isGroupSilenceReply("<NO_REPLY> 但是我还想说两句"), false);
  assert.equal(isGroupSilenceReply(""), false);
});

test("formatGroupTimestamp：本地时区 YYYY-MM-DD HH:mm:ss 补零", async () => {
  const { formatGroupTimestamp } = await import("../src/bots/groupPrompt.js");
  const at = new Date(2026, 0, 2, 3, 4, 5).getTime();
  assert.equal(formatGroupTimestamp(at), "2026-01-02 03:04:05");
});

test("buildGroupTurnPrompt：always 模式给规则块 + @ 标记 + NO_REPLY 契约，并清洗字段", async () => {
  const { buildGroupTurnPrompt } = await import("../src/bots/groupPrompt.js");
  const prompt = buildGroupTurnPrompt({
    botName: "小助手",
    activation: "always",
    isMention: false,
    senderName: "张三",
    chatId: "oc_group_1",
    providerLabel: "飞书",
    receivedAt: new Date(2026, 9, 7, 20, 31, 5).getTime(),
    locale: "zh-CN",
    historyContext: "[以下是机器人未参与期间的群聊记录，仅供参考]",
    content: "这版设计稿谁看过？",
  });
  assert.ok(prompt.startsWith("<system-reminder>"), "规则块必须在最前");
  assert.ok(prompt.includes("「oc_group_1」飞书群聊"), "群身份要带群 ID 与平台");
  assert.ok(prompt.includes("你的名字是「小助手」"));
  assert.ok(prompt.includes("激活模式：全部消息"));
  assert.ok(prompt.includes("<NO_REPLY>"), "always 模式必须给出沉默契约");
  assert.ok(prompt.includes("[from: 张三 2026-10-07 20:31:05]"), "当前消息必须带发送者与时间");
  assert.ok(prompt.includes("[本条消息未 @你]"), "未被 @ 时要有明确标记");
  assert.ok(prompt.endsWith("这版设计稿谁看过？"), "正文必须保留在末尾");
  // 历史块夹在规则块与当前消息之间
  assert.ok(
    prompt.indexOf("群聊记录") > prompt.indexOf("</system-reminder>") &&
      prompt.indexOf("群聊记录") < prompt.indexOf("[from: 张三"),
  );
});

test("buildGroupTurnPrompt：mention 模式不注入 NO_REPLY 契约，@ 标记带上", async () => {
  const { buildGroupTurnPrompt } = await import("../src/bots/groupPrompt.js");
  const mention = buildGroupTurnPrompt({
    activation: "mention",
    isMention: true,
    senderName: "Li",
    chatId: "chat-1",
    providerLabel: "Telegram",
    receivedAt: Date.now(),
    locale: "en-US",
    historyContext: null,
    content: "run the tests",
  });
  assert.ok(!mention.includes("<NO_REPLY>"), "mention 模式不应出现沉默契约");
  assert.ok(!mention.includes("mentions you - you must reply"), "mention 模式不需要 @ 标记");
  assert.ok(mention.includes("[from: Li "), "发送者标记仍然要有");
  assert.ok(mention.includes("Activation mode: mention only"));

  const always = buildGroupTurnPrompt({
    activation: "always",
    isMention: true,
    senderName: "Li",
    chatId: "chat-1",
    providerLabel: "Telegram",
    receivedAt: Date.now(),
    locale: "en-US",
    historyContext: null,
    content: "hi",
  });
  assert.ok(always.includes("[This message mentions you - you must reply]"));
  assert.ok(always.includes("<NO_REPLY>"));
});

test("buildGroupTurnPrompt：群 ID/昵称里的换行与尖括号被清洗，缺省有兜底", async () => {
  const { buildGroupTurnPrompt } = await import("../src/bots/groupPrompt.js");
  const prompt = buildGroupTurnPrompt({
    botName: "<script>\n坏名字",
    activation: "always",
    isMention: false,
    senderName: "a\nb<evil>",
    chatId: "oc\n<inj>",
    providerLabel: "Webhook",
    receivedAt: Date.now(),
    locale: "zh-CN",
    historyContext: null,
    content: "hi",
  });
  assert.ok(!prompt.includes("<script>"), "机器人名里的标签必须被剥掉");
  assert.ok(!prompt.includes("<inj>"), "群 ID 里的标签必须被剥掉");
  assert.ok(prompt.includes("[from: a b evil "), "昵称里的换行/标签必须被清洗");
  assert.ok(!prompt.includes("\n坏名字\n"), "机器人名不应带换行进入规则块");

  const fallback = buildGroupTurnPrompt({
    activation: "mention",
    isMention: true,
    chatId: "chat-2",
    providerLabel: "Telegram",
    receivedAt: Date.now(),
    locale: "zh-CN",
    historyContext: null,
    content: "hi",
  });
  assert.ok(fallback.includes("你的名字是「助手」"), "机器人无名字时用兜底名");
  assert.ok(fallback.includes("[from: 未知成员 "), "发送者缺失时用兜底名");
});

test("群聊工具禁用：覆盖本机读写工具（与 MyAgents 的 DEFAULT_GROUP_TOOLS_DENY 对齐）", async () => {
  const { GROUP_CHAT_TOOL_DENYLIST } = await import("../src/bots/groupPrompt.js");
  for (const tool of ["Bash", "Edit", "Write", "ApplyPatch"]) {
    assert.ok(GROUP_CHAT_TOOL_DENYLIST.includes(tool), tool + " 必须在群聊禁用列表里");
  }
});

test("群聊回合装配已接入服务：prompt/工具禁用/沉默收口都是源码契约", async () => {
  const source = await import("node:fs/promises").then((fs) =>
    fs.readFile(new URL("../src/bots/botsService.ts", import.meta.url), "utf8"),
  );
  // 群聊回合用 buildGroupTurnPrompt 装配（不再只拼历史）
  assert.ok(source.includes("buildGroupTurnPrompt(groupPromptInput)"), "缺少群聊回合装配");
  // 两条 sendPrompt 路径都要带群聊工具禁用
  const denylistUses = source.match(/isGroupTurn \? GROUP_CHAT_TOOL_DENYLIST : undefined/gu) ?? [];
  assert.equal(denylistUses.length, 2, "新任务与既有任务两条路径都要带群聊工具禁用");
  // 可能沉默的回合不建卡片、不发中途工具摘要
  assert.ok(
    source.includes("if (groupSilenceCapable) {\n        return false;\n      }") ||
      /groupSilenceCapable[\s\S]{0,200}不支持|可能沉默的回合[\s\S]{0,120}不建卡片/u.test(source),
    "沉默回合必须关闭流式卡片",
  );
  assert.ok(source.includes("isGroupSilenceReply(groupText)"), "缺少沉默回执判定");
  assert.ok(source.includes("!groupSilenceCapable"), "中途工具摘要必须避开沉默回合");
});
