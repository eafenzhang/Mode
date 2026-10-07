import assert from "node:assert/strict";
import test from "node:test";

// 出站文本与媒体边界的纯函数：首帧门槛、分片、媒体判定、路径白名单。
// 这些规则决定流式观感与"文件只能发到用户自己的聊天"这条安全边界。

test("hasSentenceBoundary：短文本无标点不发、有标点或达到长度阈值即发", async () => {
  const { hasSentenceBoundary, BOT_FIRST_SEND_MIN_CHARS } = await import("../src/bots/botText.js");
  assert.equal(hasSentenceBoundary(""), false);
  assert.equal(hasSentenceBoundary("好"), false);
  assert.equal(hasSentenceBoundary("正在处理"), false);
  // 句子边界立即放行
  assert.equal(hasSentenceBoundary("好的，"), true);
  assert.equal(hasSentenceBoundary("Done."), true);
  assert.equal(hasSentenceBoundary("第一行\n"), true);
  // 达到最小长度（按码点计数）放行
  assert.equal(hasSentenceBoundary("a".repeat(BOT_FIRST_SEND_MIN_CHARS)), true);
  assert.equal(hasSentenceBoundary("中".repeat(BOT_FIRST_SEND_MIN_CHARS - 1) + "中"), true);
  assert.equal(hasSentenceBoundary("中".repeat(BOT_FIRST_SEND_MIN_CHARS - 1)), false);
});

test("splitBotText：短文本单片、超长按段落/行边界切且不丢内容", async () => {
  const { splitBotText } = await import("../src/bots/botText.js");
  assert.deepEqual(splitBotText("hello", 100), ["hello"]);
  assert.deepEqual(splitBotText("   ", 100), []);

  // 段落边界优先：limit=20 时应切在 \n\n 处而不是硬切
  const paragraphText = `${"a".repeat(15)}\n\n${"b".repeat(15)}\n\n${"c".repeat(15)}`;
  const chunks = splitBotText(paragraphText, 20);
  assert.ok(chunks.length >= 2);
  for (const chunk of chunks) {
    assert.ok([...chunk].length <= 20, `chunk over limit: ${chunk.length}`);
  }
  const rejoined = chunks.join("").replace(/\s/gu, "");
  assert.equal(rejoined, paragraphText.replace(/\s/gu, ""));
});

test("splitBotText：无边界超长文本硬切也不丢字符", async () => {
  const { splitBotText } = await import("../src/bots/botText.js");
  const text = "x".repeat(95);
  const chunks = splitBotText(text, 30);
  assert.equal(chunks.join(""), text);
  assert.ok(chunks.every((chunk) => chunk.length <= 30));
});

test("resolveOutboundMediaKind：图片扩展名映射 MIME，未知按文件", async () => {
  const { resolveOutboundMediaKind } = await import("../src/bots/botMedia.js");
  assert.deepEqual(resolveOutboundMediaKind("chart.png"), {
    kind: "image",
    mimeType: "image/png",
  });
  assert.deepEqual(resolveOutboundMediaKind("SCREENSHOT.PNG"), {
    kind: "image",
    mimeType: "image/png",
  });
  assert.deepEqual(resolveOutboundMediaKind("photo.jpeg"), {
    kind: "image",
    mimeType: "image/jpeg",
  });
  assert.deepEqual(resolveOutboundMediaKind("report.pdf"), {
    kind: "file",
    mimeType: "application/octet-stream",
  });
  assert.deepEqual(resolveOutboundMediaKind("Makefile"), {
    kind: "file",
    mimeType: "application/octet-stream",
  });
});

test("isPathInside：带上限的前缀比较（/foo 不应命中 /foobar）", async () => {
  const { isPathInside } = await import("../src/bots/botMedia.js");
  const root = process.platform === "win32" ? "C:\\workspace" : "/workspace";
  const inside = process.platform === "win32" ? "C:\\workspace\\a\\b.png" : "/workspace/a/b.png";
  const sibling =
    process.platform === "win32" ? "C:\\workspace-extra\\x" : "/workspace-extra/x";
  const nested =
    process.platform === "win32" ? "C:\\workspace\\sub\\..\\file.txt" : "/workspace/sub/../file.txt";

  assert.equal(isPathInside(root, root), true);
  assert.equal(isPathInside(root, inside), true);
  // 关键回归：前缀相同但不是同一目录，必须拒绝
  assert.equal(isPathInside(root, sibling), false);
  assert.equal(isPathInside(root, nested), true);
});
