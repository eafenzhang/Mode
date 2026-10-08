import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import { shouldShowTypingBubble } from "../../../packages/ui/src/v4/conversationTypingBubble";

// 需求：消息发出后、回复首字到达前，会话时间线用「正在输入」三点气泡占位，
// 回复一到就让位给正文/ChatLoading。钉住：
//   1. 判据纯函数三态（running 空窗才显示，有内容或不在跑都不显示）；
//   2. 槽位渲染接线（TurnChatLoadingSlot 按 typing 分支渲染气泡，三个调用点都传了）；
//   3. i18n 三语齐（chat.typing）。

test("判据三态：running 且零助手内容才显示气泡", () => {
  assert.equal(shouldShowTypingBubble({ isRunning: true, assistantRowCount: 0 }), true);
  assert.equal(shouldShowTypingBubble({ isRunning: true, assistantRowCount: 3 }), false);
  assert.equal(shouldShowTypingBubble({ isRunning: false, assistantRowCount: 0 }), false);
  assert.equal(shouldShowTypingBubble({ isRunning: false, assistantRowCount: 2 }), false);
});

test("槽位接线：气泡有 testid，retry 优先、typing 次之、ChatLoading 兜底", async () => {
  const source = await readFile(
    new URL("../../../packages/ui/src/v4/ConversationTurnGroup.tsx", import.meta.url),
    "utf8",
  );
  assert.ok(
    source.includes('data-testid="conversation-typing-bubble"'),
    "气泡要挂稳定 testid 供实机断言",
  );
  assert.ok(
    source.includes('role="status"') && source.includes("chat.typing"),
    "气泡要有 role=status 与 i18n 可访问名",
  );
  const wiredCalls = source.split("typing={showTypingBubble}").length - 1;
  assert.ok(wiredCalls >= 3, `三个 TurnChatLoadingSlot 调用点都要传 typing，实际 ${wiredCalls} 处`);
  assert.ok(
    source.includes("shouldShowTypingBubble({"),
    "判据必须来自纯函数，不允许在组件里即兴判断",
  );
});

test("chat.typing 三语齐", async () => {
  for (const [locale, expected] of [
    ["zh-CN", "正在输入"],
    ["en-US", "Typing"],
    ["fa", "در حال تایپ"],
  ] as const) {
    const source = await readFile(
      new URL(`../../../packages/ui/src/i18n/locales/${locale}.ts`, import.meta.url),
      "utf8",
    );
    assert.ok(
      source.includes(`"chat.typing": "${expected}"`),
      `${locale} 缺 chat.typing 或文案不符`,
    );
  }
});
