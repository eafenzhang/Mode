import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

// 离散气泡档的逐块刷出（docs/specs/bot-reply-granularity.md「逐块刷出」）：
//   R1 进行中按"完整片段"刷出，前缀不足最小长度不单独成气泡，超长无强边界时兜底切弱边界；
//   R2 群聊「全部消息」模式例外——必须整段等到终态，否则 <NO_REPLY> 判定前已有气泡落地。

const serviceUrl = new URL("../src/bots/botsService.ts", import.meta.url);

const codePointLength = (value: string): number => [...value].length;

test("没有强边界：无论多长都不刷出（只攒着）", async () => {
  const { drainCompleteBotReplySegments } = await import("../src/bots/botText.js");
  // 弱边界（逗号）不算完整片段，且长度未到兜底上限。
  const pending = "我正在读构建日志，刚才那一步看起来是这个问题的关键，后面还有";
  assert.deepEqual(drainCompleteBotReplySegments(pending), { messages: [], rest: pending });
});

test("前缀不足最小长度：完整句子也不单独成气泡", async () => {
  const { drainCompleteBotReplySegments } = await import("../src/bots/botText.js");
  const short = "好的。";
  assert.deepEqual(drainCompleteBotReplySegments(short), { messages: [], rest: short });
});

test("强边界且前缀够长：一次切出整段，尾巴留在缓冲", async () => {
  const { drainCompleteBotReplySegments } = await import("../src/bots/botText.js");
  const first = "我已经读完了构建日志，找到了失败的根本原因。";
  const second = "接下来我要改配置。";
  const tail = "现在开始改代码";
  assert.ok(codePointLength(first) >= 20, "夹具本身要过最小长度门槛");

  const drained = drainCompleteBotReplySegments(`${first}\n${second}\n${tail}`);
  // 切在最后一个强边界（第二句的句号）——多句在同一批增量里也只成一个气泡。
  assert.deepEqual(drained.messages, [`${first}\n${second}`]);
  assert.equal(drained.rest, tail);
});

test("超长且无强边界：退到弱边界切一刀，内容不丢", async () => {
  const { BOT_REPLY_SEGMENT_MAX_CHARS, drainCompleteBotReplySegments } = await import(
    "../src/bots/botText.js"
  );
  // 只用逗号（弱边界）构造远长于兜底上限、但没有句末标点的文本。
  const long = "甲乙丙丁，".repeat(200);
  assert.ok(codePointLength(long) > BOT_REPLY_SEGMENT_MAX_CHARS);
  assert.ok(!/[。！？；;.!?]/u.test(long), "夹具不能含强边界");

  const drained = drainCompleteBotReplySegments(long);
  assert.equal(drained.messages.length, 1, "兜底切分只发一片");
  assert.ok(codePointLength(drained.messages[0]!) <= BOT_REPLY_SEGMENT_MAX_CHARS);
  // 边界处空白不保留，正文本身不丢字符。
  const rejoined = [...drained.messages, drained.rest].join("").replace(/\s/gu, "");
  assert.equal(rejoined, long.replace(/\s/gu, ""));
  assert.ok(drained.rest.length > 0, "必须留尾巴，不能一次清空");
});

test("纯 CJK 无空白无标点：保持等待，不硬切汉字", async () => {
  const { drainCompleteBotReplySegments } = await import("../src/bots/botText.js");
  const hard = "甲".repeat(800);
  assert.deepEqual(drainCompleteBotReplySegments(hard), { messages: [], rest: hard });
});

test("CRLF 归一化后再判边界", async () => {
  const { drainCompleteBotReplySegments } = await import("../src/bots/botText.js");
  const first = "这是第一段足够长的正文内容，后面还会继续补充。";
  assert.ok(codePointLength(first) >= 20, "夹具本身要过最小长度门槛");
  const drained = drainCompleteBotReplySegments(`${first}\r\n尾巴`);
  assert.deepEqual(drained.messages, [first]);
  assert.equal(drained.rest, "尾巴");
});

test("源码契约：非强制刷出按逐块切分，且群聊「全部消息」模式不走逐块", async () => {
  const source = await readFile(serviceUrl, "utf8");
  const start = source.indexOf("const flushAssistantReplyBuffer =");
  assert.ok(start >= 0, "缺少 flushAssistantReplyBuffer");
  const body = source.slice(start, start + 1600).replace(/\s+/g, " ");
  assert.ok(
    body.includes("drainCompleteBotReplySegments(assistantReplyBuffer)"),
    "非强制刷出必须走逐块切分，否则纯文本回合仍是回合末整段落下",
  );
  assert.ok(
    body.includes("const progressive = !force && !groupSilenceCapable;"),
    "群聊「全部消息」模式必须排除在逐块刷出之外：中途发言会破坏 <NO_REPLY> 沉默无痕",
  );
  assert.ok(
    body.includes("extractBotAssistantResponseMessages(assistantReplyBuffer, force)"),
    "强制刷出（工具边界 / 回合末）仍用既有整段提取，保证终稿内容与分片不变",
  );
});
