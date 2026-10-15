import assert from "node:assert/strict";
import test from "node:test";

// 断线重连退避策略（P2-1，spec: docs/specs/lan-remote-desktop-native-media.md 降级语义）：
// 媒体 WS 瞬断与整会话重连共用同一阶梯 —— 250ms 起、2s 封顶、8 次上限。
const { nextReconnectDelayMs, RECONNECT_MAX_ATTEMPTS } = await import(
  "../src/remote-desktop/reconnectPolicy.js"
);

test("退避阶梯：250 → 500 → 1000 → 2000 封顶", () => {
  assert.equal(nextReconnectDelayMs(1), 250);
  assert.equal(nextReconnectDelayMs(2), 500);
  assert.equal(nextReconnectDelayMs(3), 1_000);
  assert.equal(nextReconnectDelayMs(4), 2_000);
  assert.equal(nextReconnectDelayMs(5), 2_000, "封顶后不再增长");
  assert.equal(nextReconnectDelayMs(8), 2_000);
});

test("越界入参不放大、不崩：attempt ≤ 0 按首档处理", () => {
  assert.equal(nextReconnectDelayMs(0), 250, "attempt 0（首轮）按 250ms");
  assert.equal(nextReconnectDelayMs(-3), 250);
});

test("尝试上限 = 8（≈14s 窗口覆盖一次网络抖动）", () => {
  assert.equal(RECONNECT_MAX_ATTEMPTS, 8);
  let total = 0;
  for (let attempt = 1; attempt <= RECONNECT_MAX_ATTEMPTS; attempt += 1) {
    total += nextReconnectDelayMs(attempt);
  }
  assert.ok(total > 10_000 && total < 20_000, `总窗口 ${total}ms 应在 10–20s 量级`);
});
