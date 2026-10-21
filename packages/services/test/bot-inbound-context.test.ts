import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { parseBotCommand } from "../src/bots/commandParser.js";
import { buildPrivateTurnPrompt } from "../src/bots/privatePrompt.js";
import { createWeComBotProvider } from "../src/bots/providers/wecomProvider.js";
import type { WeComConnectionRegistry } from "../src/bots/wecomConnection.js";

// Bot 各环节对齐（docs/specs/im-bot-agent-channel-aplus.md 规则 8/9）：
// 私聊要有渠道上下文，/start 是核心命令，企微“引用消息”要进入模型上下文。

function parseWeComText(body: Record<string, unknown>) {
  const provider = createWeComBotProvider({
    loadCredential: async () => null,
    connection: {
      getConnection: () => null,
      setConnection: () => undefined,
      clearConnection: () => undefined,
    } as unknown as WeComConnectionRegistry,
  });
  return provider.parseCallback({
    botId: "bot-1",
    frame: {
      headers: { req_id: "req-1" },
      body: {
        msgid: "m1",
        aibotid: "bot-1",
        chattype: "single",
        from: { userid: "user-1" },
        ...body,
      },
    },
  });
}

test("私聊渠道上下文：告诉模型在 IM 里回答，并提示用 SendFile 交付文件", () => {
  const zh = buildPrivateTurnPrompt({
    providerLabel: "企业微信",
    mediaEnabled: true,
    content: "把报告发我",
    locale: "zh-CN",
  });
  assert.match(zh, /企业微信/);
  assert.match(zh, /SendFile/);
  assert.match(zh, /桌面端/);
  assert.ok(zh.endsWith("把报告发我"), "用户正文必须保留在最后");

  const en = buildPrivateTurnPrompt({
    providerLabel: "WeCom",
    mediaEnabled: true,
    content: "send me the report",
    locale: "en-US",
  });
  assert.match(en, /WeCom/);
  assert.match(en, /SendFile/);

  const noMedia = buildPrivateTurnPrompt({
    providerLabel: "微信",
    mediaEnabled: false,
    content: "把报告发我",
    locale: "zh-CN",
  });
  assert.doesNotMatch(noMedia, /SendFile/, "无出站媒体的通道不应提示模型调用 SendFile");
});

test("/start 与 /开始 归入核心帮助入口", () => {
  assert.deepEqual(parseBotCommand("/start"), { type: "help" });
  assert.deepEqual(parseBotCommand("/开始"), { type: "help" });
});

test("企微引用消息进入 quotedText，正文与命令解析不受影响", () => {
  const parsed = parseWeComText({
    msgtype: "text",
    text: { content: "/send reports/today.pdf" },
    quote: { msgtype: "text", text: { content: "上一段对话" } },
  });
  assert.equal(parsed.length, 1);
  assert.equal(parsed[0]!.text, "/send reports/today.pdf");
  assert.equal(parsed[0]!.quotedText, "上一段对话");
  // 命令解析只看 text，引用前缀不能把 /send 打成普通消息。
  assert.deepEqual(parseBotCommand(parsed[0]!.text), {
    type: "send",
    value: "reports/today.pdf",
  });
});

test("任务标题只取用户原始输入，不把 system-reminder 带进侧栏", async () => {
  const source = await readFile(new URL("../src/bots/botsService.ts", import.meta.url), "utf8");
  const titleSourceIndex = source.indexOf("const titleSource =");
  const groupPromptIndex = source.indexOf("content: buildGroupTurnPrompt(groupPromptInput)");
  assert.ok(titleSourceIndex >= 0, "必须保留用户原始输入作为标题来源");
  assert.ok(
    source.includes("message.text.trim() || stripBotChannelContextForDisplay"),
    "标题必须优先用用户正文，附件-only 时才回退到剥离引用行的内容",
  );
  assert.ok(
    titleSourceIndex < groupPromptIndex,
    "标题来源必须在群/私聊上下文包装之前捕获",
  );
  assert.ok(
    source.includes("deriveTaskTitle(titleSource"),
    "deriveTaskTitle 必须使用原始输入，而不是包装后的 prompt",
  );
});

test("企微引用图片/文件只保留类型占位，不重复下载", () => {
  const image = parseWeComText({
    msgtype: "text",
    text: { content: "这张图里的内容是什么" },
    quote: { msgtype: "image", image: { url: "https://example.invalid/a.jpg" } },
  });
  assert.equal(image[0]!.quotedText, "[图片]");

  const file = parseWeComText({
    msgtype: "text",
    text: { content: "总结一下" },
    quote: { msgtype: "file", file: { url: "https://example.invalid/a.pdf" } },
  });
  assert.equal(file[0]!.quotedText, "[文件]");
});
