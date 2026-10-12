import assert from "node:assert/strict";
import test from "node:test";

// 自适应码率判定（纯逻辑，spec: docs/plans/2026-10-12-remote-desktop-quality-latency-optimization）。
const {
  ADAPTIVE_BITRATE_HIGH,
  ADAPTIVE_BITRATE_LOW,
  initialAdaptiveBitrateState,
  stepAdaptiveBitrate,
} = await import("../src/remote-desktop/adaptiveBitrate.js");

test("高 RTT 或任何丢弃 → 立即降档；已在低档不重复下发", () => {
  const t0 = 1_000_000;
  let { state, nextBps } = stepAdaptiveBitrate(
    initialAdaptiveBitrateState(),
    { rttMs: 80, droppedFrames: 0 },
    t0,
  );
  assert.equal(nextBps, ADAPTIVE_BITRATE_LOW, "RTT 超阈应立即降档");
  assert.equal(state.currentBps, ADAPTIVE_BITRATE_LOW);
  assert.equal(state.lastSwitchAt, t0);

  ({ state, nextBps } = stepAdaptiveBitrate(state, { rttMs: 90, droppedFrames: 5 }, t0 + 3_000));
  assert.equal(nextBps, null, "已在低档不再重复下发");
});

test("丢弃优先于 RTT：rtt 正常但丢帧同样降档", () => {
  const { nextBps } = stepAdaptiveBitrate(
    initialAdaptiveBitrateState(),
    { rttMs: 10, droppedFrames: 1 },
    0,
  );
  assert.equal(nextBps, ADAPTIVE_BITRATE_LOW);
});

test("rtt 未测得（null）不算健康也不算拥塞：保持现状、不累计健康", () => {
  let { state } = stepAdaptiveBitrate(
    initialAdaptiveBitrateState(),
    { rttMs: 90, droppedFrames: 0 },
    0,
  );
  const before = state.healthyStreak;
  ({ state } = stepAdaptiveBitrate(state, { rttMs: null, droppedFrames: 0 }, 3_000));
  assert.equal(state.healthyStreak, before, "null RTT 不累计健康次数");
  assert.equal(state.currentBps, ADAPTIVE_BITRATE_LOW);
});

test("升档需连续健康采样 + 最小驻留，缺一不可", () => {
  const t0 = 0;
  let state = stepAdaptiveBitrate(
    initialAdaptiveBitrateState(),
    { rttMs: 80, droppedFrames: 0 },
    t0,
  ).state; // 降档，lastSwitchAt = 0

  // 3 次健康采样（不足 RECOVER_STREAK=4）→ 不升
  for (let i = 1; i <= 3; i += 1) {
    state = stepAdaptiveBitrate(state, { rttMs: 5, droppedFrames: 0 }, t0 + i * 3_000).state;
  }
  assert.equal(state.currentBps, ADAPTIVE_BITRATE_LOW, "健康次数不足不升档");

  // 第 4 次健康：驻留 12s ≥ 10s 且 streak=4 → 升档
  const result = stepAdaptiveBitrate(state, { rttMs: 5, droppedFrames: 0 }, t0 + 12_000);
  assert.equal(result.nextBps, ADAPTIVE_BITRATE_HIGH, "连续健康+驻留满足应升档");
  assert.equal(result.state.healthyStreak, 0, "升档后健康计数清零");
  assert.equal(result.state.lastSwitchAt, t0 + 12_000);
});

test("驻留期内驻足以外的健康采样会打断 streak（回中间态不升档）", () => {
  const t0 = 0;
  let state = stepAdaptiveBitrate(
    initialAdaptiveBitrateState(),
    { rttMs: 80, droppedFrames: 0 },
    t0,
  ).state;
  // 健康 3 次后出现一次「半健康」（RTT 介于 30..60：不健康也不拥塞）→ streak 清零
  for (let i = 1; i <= 3; i += 1) {
    state = stepAdaptiveBitrate(state, { rttMs: 5, droppedFrames: 0 }, t0 + i * 3_000).state;
  }
  state = stepAdaptiveBitrate(state, { rttMs: 45, droppedFrames: 0 }, t0 + 12_000).state;
  assert.equal(state.currentBps, ADAPTIVE_BITRATE_LOW, "56ms 区间不升档（滞回带）");
  assert.equal(state.healthyStreak, 0, "非健康采样清零计数");
});
