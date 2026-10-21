import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import { modeBotSendMediaParamsSchema, modeProtocolMethods } from "@mode/shared";
import { createWeComBotProvider } from "../src/bots/providers/wecomProvider.js";
import type { WeComConnectionRegistry } from "../src/bots/wecomConnection.js";
import type { BotConfig } from "@mode/shared";

// 出站文件（docs/specs/bot-outbound-media.md）：
//   群聊 /send 不再被群命令过滤吞掉；agent 的 SendFile 经 Host 回到
//   botsService.deliverBotMedia（同一套白名单/限额/目标解析）；
//   企微 caption 以独立 markdown 先发，不静默丢弃。

const botsServiceSource = await readFile(
  new URL("../src/bots/botsService.ts", import.meta.url),
  "utf8",
);

function windowAfter(source: string, marker: string, lineSpan: number): string {
  const index = source.indexOf(marker);
  assert.ok(index >= 0, `未找到锚点：${marker}`);
  return source.slice(index).split("\n").slice(0, lineSpan).join("\n");
}

test("群聊命令过滤放行 /send 与 /help，仍丢弃其它管理命令", () => {
  const filter = windowAfter(
    botsServiceSource,
    'command.type !== "message" &&\n          command.type !== "stop"',
    12,
  );
  assert.ok(
    filter.includes('command.type !== "send"'),
    "群聊必须放行 /send（用户显式发文件不是群管理命令）",
  );
  assert.ok(
    filter.includes('command.type !== "help"'),
    "群聊必须放行 /help（只读说明，群文案已承诺支持，/start 也归入 help）",
  );
  assert.ok(
    filter.includes('command.type !== "approve"') && filter.includes('command.type !== "deny"'),
    "对话/停止/权限响应的既有放行不能改变",
  );
});

test("SendFile 的 Host 协议方法已注册，且目标/路径/说明进入 schema", () => {
  assert.equal(modeProtocolMethods.botSendMedia, "bots/sendMedia");
  const parsed = modeBotSendMediaParamsSchema.parse({
    target: {
      provider: "wecom",
      botId: "bot-1",
      providerUserId: "chat-or-user-1",
      chatType: "group",
    },
    filePath: "reports/today.md",
    caption: "今天的报告",
  });
  assert.equal(parsed.target.chatType, "group");
  assert.equal(parsed.filePath, "reports/today.md");
  assert.equal(parsed.caption, "今天的报告");
  assert.equal(
    modeBotSendMediaParamsSchema.safeParse({ filePath: "a.md" }).success,
    false,
    "目标必须由 Host 注入，缺目标应被协议拒绝",
  );
});

test("sendBotMedia 按投递目标转交 deliverBotMedia，并返回文件名/类型", () => {
  const impl = windowAfter(botsServiceSource, "async sendBotMedia(params", 28);
  assert.ok(
    impl.includes("deliverBotMedia({"),
    "sendBotMedia 必须复用 deliverBotMedia（路径白名单/限额/目标解析的唯一所有者）",
  );
  assert.ok(impl.includes("return deliverBotMedia({"), "必须把 filename/kind 返回给工具回执");
  assert.ok(
    impl.includes("bot.provider !== params.provider"),
    "跨进程传入的 provider 必须与 bot 记录校验",
  );
});

function createWeComMediaHarness(options: { withFrame?: boolean } = {}) {
  const calls: Array<{ method: string; args: unknown[] }> = [];
  const frame = { headers: { req_id: "req-1" }, body: { msgid: "m1" } };
  const client = {
    async uploadMedia(...args: unknown[]) {
      calls.push({ method: "uploadMedia", args });
      return { type: "file", media_id: "media-1", created_at: "2026-10-21" };
    },
    async sendMediaMessage(...args: unknown[]) {
      calls.push({ method: "sendMediaMessage", args });
      return {};
    },
    async replyMedia(...args: unknown[]) {
      calls.push({ method: "replyMedia", args });
      return {};
    },
    async sendMessage(...args: unknown[]) {
      calls.push({ method: "sendMessage", args });
      return {};
    },
    async replyStream(...args: unknown[]) {
      calls.push({ method: "replyStream", args });
      return {};
    },
  };
  const connection = {
    client,
    getLastFrame: () => (options.withFrame === false ? null : frame),
  };
  const registry = {
    getConnection: () => connection,
    setConnection: () => undefined,
    clearConnection: () => undefined,
  } as unknown as WeComConnectionRegistry;
  return {
    calls,
    provider: createWeComBotProvider({
      loadCredential: async () => "secret",
      connection: registry,
    }),
  };
}

test("企微 sendMedia：先发 caption markdown，再发文件消息", async () => {
  const { provider, calls } = createWeComMediaHarness();
  const bot = { id: "bot-1", provider: "wecom" } as unknown as BotConfig;
  await provider.sendMedia!(
    bot,
    { providerUserId: "chat-1" },
    {
      kind: "file",
      filename: "report.pdf",
      mimeType: "application/pdf",
      data: new Uint8Array([1, 2, 3]),
      caption: "这是说明",
    },
  );

  assert.deepEqual(
    calls.map((call) => call.method),
    ["uploadMedia", "replyStream", "replyMedia"],
    "caption 与文件都挂在入站消息下（回复引用），不另发主动推送",
  );
  const captionCall = calls[1]!.args as [unknown, string, string, boolean];
  assert.equal(captionCall[2], "这是说明");
  assert.equal(captionCall[3], true);
  const mediaCall = calls[2]!.args as [unknown, string, string];
  assert.equal(mediaCall[1], "file");
  assert.equal(mediaCall[2], "media-1");
});

test("企微 sendMedia：有等待占位流时由媒体收口，caption 原地替换而不是另发一条", async () => {
  const { provider, calls } = createWeComMediaHarness();
  const bot = { id: "bot-1", provider: "wecom" } as unknown as BotConfig;
  await provider.startTyping!(bot, { providerUserId: "chat-1", providerMessageId: "m1" });
  calls.length = 0;

  await provider.sendMedia!(
    bot,
    { providerUserId: "chat-1" },
    {
      kind: "file",
      filename: "report.pdf",
      mimeType: "application/pdf",
      data: new Uint8Array([1, 2, 3]),
      caption: "这是说明",
    },
  );

  assert.deepEqual(
    calls.map((call) => call.method),
    ["uploadMedia", "replyStream", "replyMedia"],
    "占位流用它自己的 replyStream 收口，caption 不再另发一条文本",
  );
  const streamCall = calls[1]!.args as [unknown, string, string, boolean];
  assert.equal(streamCall[2], "这是说明");
  assert.equal(streamCall[3], true, "媒体出站必须 finish 占位流");
});

test("企微 sendMedia：无 caption 时用文件名收口占位流，避免永久 loading", async () => {
  const { provider, calls } = createWeComMediaHarness();
  const bot = { id: "bot-1", provider: "wecom" } as unknown as BotConfig;
  await provider.startTyping!(bot, { providerUserId: "chat-1", providerMessageId: "m1" });
  calls.length = 0;

  await provider.sendMedia!(
    bot,
    { providerUserId: "chat-1" },
    {
      kind: "image",
      filename: "shot.png",
      mimeType: "image/png",
      data: new Uint8Array([1]),
    },
  );

  assert.deepEqual(
    calls.map((call) => call.method),
    ["uploadMedia", "replyStream", "replyMedia"],
  );
  const streamCall = calls[1]!.args as [unknown, string, string, boolean];
  assert.equal(streamCall[2], "shot.png");
  assert.equal(streamCall[3], true);
});

test("企微 sendMedia：无 caption 时不额外发文本，保持单条媒体消息", async () => {
  const { provider, calls } = createWeComMediaHarness();
  const bot = { id: "bot-1", provider: "wecom" } as unknown as BotConfig;
  await provider.sendMedia!(
    bot,
    { providerUserId: "chat-1" },
    {
      kind: "image",
      filename: "shot.png",
      mimeType: "image/png",
      data: new Uint8Array([1]),
    },
  );

  assert.deepEqual(
    calls.map((call) => call.method),
    ["uploadMedia", "replyMedia"],
  );
});

test("企微 sendMedia：无入站帧时 caption 与文件都回落主动推送", async () => {
  const { provider, calls } = createWeComMediaHarness({ withFrame: false });
  const bot = { id: "bot-1", provider: "wecom" } as unknown as BotConfig;
  await provider.sendMedia!(
    bot,
    { providerUserId: "chat-1" },
    {
      kind: "file",
      filename: "report.pdf",
      mimeType: "application/pdf",
      data: new Uint8Array([1]),
      caption: "这是说明",
    },
  );

  assert.deepEqual(
    calls.map((call) => call.method),
    ["uploadMedia", "sendMessage", "sendMediaMessage"],
  );
  const captionCall = calls[1]!.args as [string, { msgtype: string; markdown: { content: string } }];
  assert.equal(captionCall[0], "chat-1");
  assert.equal(captionCall[1].markdown.content, "这是说明");
});

test("企微 sendMedia：无入站帧时回落主动推送 sendMediaMessage", async () => {
  const { provider, calls } = createWeComMediaHarness({ withFrame: false });
  const bot = { id: "bot-1", provider: "wecom" } as unknown as BotConfig;
  await provider.sendMedia!(
    bot,
    { providerUserId: "chat-1" },
    {
      kind: "file",
      filename: "report.pdf",
      mimeType: "application/pdf",
      data: new Uint8Array([1]),
    },
  );

  assert.deepEqual(
    calls.map((call) => call.method),
    ["uploadMedia", "sendMediaMessage"],
    "没有入站帧时只能主动推送",
  );
});
