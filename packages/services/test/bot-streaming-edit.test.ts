import assert from "node:assert/strict";
import test from "node:test";

// Telegram 流式回复把流式卡片 blocks 渲染为 Markdown 文本后走 editMessageText。
// 渲染是确定性的纯函数：正文拼接、工具摘要列表、状态指示符、运行中截断。

test("message blocks 拼接为正文；tools blocks 渲染为摘要列表", async () => {
  const { renderStreamingCardStateToText } = await import(
    "../src/bots/providers/telegramProvider.js"
  );
  const text = renderStreamingCardStateToText({
    providerUserId: "u1",
    status: "running",
    blocks: [
      { type: "message", text: "正在分析构建日志。\n\n" },
      { type: "tools", toolIds: [], summaries: ["read src/a.ts", "run pnpm build"], title: "工具摘要" },
      { type: "message", text: "  " },
      { type: "message", text: "找到根因。" },
    ],
  });
  assert.ok(text.includes("正在分析构建日志。"));
  assert.ok(text.includes("**工具摘要**\n- read src/a.ts\n- run pnpm build"));
  assert.ok(text.includes("找到根因。"));
  assert.ok(text.endsWith("⏳"));
  // 空白 message 块被过滤；正文、工具摘要、状态指示符共 3 个空段分隔
  assert.equal(text.split("\n\n").length, 4);
});

test("completed 状态不追加指示符；error 追加 ❌", async () => {
  const { renderStreamingCardStateToText } = await import(
    "../src/bots/providers/telegramProvider.js"
  );
  const base = {
    providerUserId: "u1",
    blocks: [{ type: "message" as const, text: "完成。" }],
  };
  assert.equal(
    renderStreamingCardStateToText({ ...base, status: "completed" }),
    "完成。",
  );
  assert.ok(
    renderStreamingCardStateToText({ ...base, status: "error" }).endsWith("❌"),
  );
});

test("空内容渲染为空串（create 端不会发出占位空消息）", async () => {
  const { renderStreamingCardStateToText } = await import(
    "../src/bots/providers/telegramProvider.js"
  );
  assert.equal(
    renderStreamingCardStateToText({ providerUserId: "u1", status: "running", blocks: [] }),
    "",
  );
});

test("共享层能力表：Telegram/企业微信支持全部粒度，微信不支持 streaming_card", async () => {
  const { getSupportedBotReplyGranularities } = await import("@mode/shared");
  const telegram = getSupportedBotReplyGranularities("telegram");
  assert.ok(telegram.includes("streaming_card"));
  // 企业微信智能机器人经 WebSocket replyStream 原生支持流式回复。
  const wecom = getSupportedBotReplyGranularities("wecom");
  assert.ok(wecom.includes("streaming_card"));
  // 微信（iLink）没有消息编辑/流式通道，仍排除 streaming_card。
  const weixin = getSupportedBotReplyGranularities("weixin");
  assert.ok(!weixin.includes("streaming_card"));
  const feishu = getSupportedBotReplyGranularities("feishu");
  assert.deepEqual([...feishu], ["streaming_card"]);
});

test("企业微信流式渲染：正文/工具摘要拼接，空内容给占位文案", async () => {
  const { renderWeComStreamText } = await import(
    "../src/bots/providers/wecomProvider.js"
  );
  const text = renderWeComStreamText({
    providerUserId: "u1",
    status: "running",
    blocks: [
      { type: "message", text: "正在分析日志。" },
      { type: "tools", toolIds: [], summaries: ["read a.ts"], title: "工具摘要" },
    ],
  });
  assert.ok(text.includes("正在分析日志。"));
  assert.ok(text.includes("**工具摘要**\n- read a.ts"));

  const placeholder = renderWeComStreamText({ providerUserId: "u1", status: "running", blocks: [] });
  assert.ok(placeholder.trim().length > 0, "empty state must render a placeholder");
});

test("企业微信 markdown 分片：短文本单片、长文本按段落切分且不丢内容", async () => {
  const { splitWeComText, WECOM_MARKDOWN_CHUNK_LIMIT } = await import(
    "../src/bots/providers/wecomProvider.js"
  );
  assert.deepEqual(splitWeComText("hello"), ["hello"]);
  assert.deepEqual(splitWeComText("   "), []);

  const paragraph = "x".repeat(100);
  const long = Array.from({ length: 60 }, () => paragraph).join("\n\n");
  const chunks = splitWeComText(long);
  assert.ok(chunks.length > 1);
  for (const chunk of chunks) {
    assert.ok(chunk.length <= WECOM_MARKDOWN_CHUNK_LIMIT);
  }
  // 仅去掉分片边界换行，正文内容不丢失。
  const rejoined = chunks.join("").replace(/\s/gu, "");
  assert.equal(rejoined.length, long.replace(/\s/gu, "").length);
});

// 交付内容不出现工具摘要：工具块只作为"进行中"的过程提示，终稿（completed/sealed）里被过滤掉。
// 行为测试需要整套服务图，这里用源码契约守住这条规则。
test("流式卡片终稿隐藏工具摘要（仅进行中展示）", async () => {
  const source = await import("node:fs/promises").then((fs) =>
    fs.readFile(new URL("../src/bots/botsService.ts", import.meta.url), "utf8"),
  );
  const index = source.indexOf("const buildStreamingCardBlocks =");
  assert.ok(index >= 0, "缺少 buildStreamingCardBlocks");
  const block = source.slice(index, index + 2600);
  // 空白归一后再匹配：源码里的换行/缩进变化不应该让这条契约失效。
  const compact = block.replace(/\s+/g, " ");
  assert.ok(
    compact.includes('const running = streamingCardStatus === "running";'),
    "必须区分进行中与终稿",
  );
  assert.ok(
    compact.includes("if (!running) { continue; }"),
    "终稿必须跳过工具块（否则工具摘要会留在交付内容里）",
  );
  assert.ok(
    compact.includes('msg(locale, running ? "streamingWorking" : "taskCompleted")'),
    "纯工具回合的终稿兜底必须是完成提示而不是过程提示",
  );
  // 进行中仍然展示工具摘要（长工具阶段需要过程反馈）
  assert.ok(
    compact.includes('expanded: streamingCardStatus === "running" && index === latestToolBlockIndex'),
    "进行中的工具摘要保持展开",
  );
});
