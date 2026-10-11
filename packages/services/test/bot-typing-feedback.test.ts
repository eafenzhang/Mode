import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

// IM 等待期反馈接线（docs/specs/im-typing-feedback.md）：
// botsService 内部态（订阅表、typingTargets）没有现成的单元测试脚手架，
// 与 conversation-typing-bubble.test.ts 同款做法——用源码断言钉住三条契约：
//   1. 订阅残留的 early-return 分支也必须起 typing（连续提问无反馈的根因）；
//   2. sendTyping 型通道同样登记 typingTargets（终态取消与 guard 依赖它）；
//   3. stopInboundTyping 的「长回合进行中」guard 按 bot+聊天匹配，不被后续入站掐断。

const sourceUrl = new URL("../src/bots/botsService.ts", import.meta.url);

test("watchTaskStream 订阅残留分支也调 startTyping", async () => {
  const source = await readFile(sourceUrl, "utf8");
  assert.match(
    source,
    /if \(streamSubscriptions\.has\(streamSubscriptionKey\)\) \{[\s\S]{0,400}?startTyping\(/,
    "复用订阅的早退分支必须补 startTyping，否则该问全程没有等待反馈",
  );
});

test("startTyping 的 sendTyping 分支登记 typingTargets", async () => {
  const source = await readFile(sourceUrl, "utf8");
  const sendBranch = source.match(/if \(adapter\.sendTyping\) \{[\s\S]*?\r?\n    \}\r?\n  \}/)?.[0];
  assert.ok(sendBranch, "找不到 startTyping 的 sendTyping 分支");
  assert.match(
    sendBranch,
    /typingTargets\.set\(typingKey, \{ bot, target \}\)/,
    "不登记 typingTargets，终态收口拿不到目标，原生「正在输入」无法取消",
  );
});

test("stopInboundTyping guard 按 bot+聊天匹配；typing 失败统一 warn 不静默", async () => {
  const source = await readFile(sourceUrl, "utf8");
  assert.match(
    source,
    /typing\.bot\.id === bot\.id && typing\.target\.providerUserId === targetId/,
    "长回合进行中，同聊天的后续入站完成不得取消进行中的 typing",
  );
  assert.doesNotMatch(
    source,
    /typing\.target\.providerMessageId === actor\.providerMessageId/,
    "按单条入站消息匹配会让连续提问掐掉上一问刚亮起的 typing",
  );
  assert.match(
    source,
    /function warnTypingFailure\(/,
    "typing 是 best-effort 但失败必须可观测（warn 落日志）",
  );
  assert.ok(
    /warnTypingFailure\("startTyping"/.test(source) &&
      /warnTypingFailure\("sendTyping"/.test(source) &&
      /warnTypingFailure\("stopTyping"/.test(source),
    "start/send/stop 三个阶段的失败都要经 warnTypingFailure",
  );
});
