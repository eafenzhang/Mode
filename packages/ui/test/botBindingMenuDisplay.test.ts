import assert from "node:assert/strict";
import test from "node:test";
import { shortenConversationId } from "../src/lib/botConversationDisplay.js";

// IM 机器人绑定菜单对话展示（docs/specs/bot-binding-menu-display.md）：
// 同一 bot 的两条无昵称对话必须在菜单截断宽度内可区分——降级 token 的共同前缀
// 只允许省略号本身；短 id 不缩。fixture 取自实机 bot-state（2026-10-10 截图根因）。
const CONV_A = "8b43889be073@im.bot"; // draft，未绑 → 「绑定」行
const CONV_B = "o9cq80yCfHT3uR9T3VNctmmH5Ldc@im.wechat"; // mode=task，已绑 → 「解绑」行

function commonPrefix(a: string, b: string): string {
  let i = 0;
  while (i < a.length && i < b.length && a[i] === b[i]) i += 1;
  return a.slice(0, i);
}

test("两条无昵称对话的降级 token 在省略号后立刻分叉", () => {
  const tokenA = shortenConversationId(CONV_A);
  const tokenB = shortenConversationId(CONV_B);
  assert.notEqual(tokenA, tokenB);
  // 经「私聊 · {name}」模板渲染后，可见共同前缀只允许是「私聊 · …」。
  const visiblePrefix = commonPrefix(`私聊 · ${tokenA}`, `私聊 · ${tokenB}`);
  assert.ok(
    visiblePrefix.length <= "私聊 · …".length,
    `可见前缀应立即分叉，实际共同前缀：${visiblePrefix}`,
  );
});

test("降级 token 形状：本地段尾 6 + @域，前置省略号", () => {
  assert.equal(shortenConversationId(CONV_A), "…9be073@im.bot");
  assert.match(shortenConversationId(CONV_B), /^….{6}@im\.wechat$/u);
});

test("短 id（本地段 ≤6）不缩，保持原样", () => {
  assert.equal(shortenConversationId("abc@im.bot"), "abc@im.bot");
  assert.equal(shortenConversationId("123456"), "123456");
});

test("无 @ 的长 id 整体取尾 6", () => {
  assert.equal(shortenConversationId("abcdefghij"), "…efghij");
});
