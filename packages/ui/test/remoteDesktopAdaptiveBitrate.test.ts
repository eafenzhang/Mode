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

test("画质档位自定义两档（P1-3）：降/升都落在档位区间，不越级到旧常量", () => {
  const t0 = 0;
  // 均衡档 5M ↔ 2.5M（预设值口径）。
  let { state, nextBps } = stepAdaptiveBitrate(
    initialAdaptiveBitrateState(5_000_000, 2_500_000),
    { rttMs: 80, droppedFrames: 0 },
    t0,
  );
  assert.equal(nextBps, 2_500_000, "降档目标 = 档位低档（而非旧常量 4M）");
  assert.equal(state.lowBps, 2_500_000);
  // 连续健康 + 驻留后回升到档位高档。
  for (let i = 1; i <= 3; i += 1) {
    state = stepAdaptiveBitrate(state, { rttMs: 5, droppedFrames: 0 }, t0 + i * 3_000).state;
  }
  const recover = stepAdaptiveBitrate(state, { rttMs: 5, droppedFrames: 0 }, t0 + 12_000);
  assert.equal(recover.nextBps, 5_000_000, "升档目标 = 档位高档（而非旧常量 8M）");
  // 畸形入参（低档 > 高档）按高档收敛，升降语义不反转。
  const clamped = initialAdaptiveBitrateState(3_000_000, 9_000_000);
  assert.equal(clamped.lowBps, 3_000_000);
});

// ---- 动态降档（spec: native-media「画质档位」动态降档 2026-10-13）----
const {
  nextLowerQuality,
  stepQualityDownshift,
  INITIAL_QUALITY_DOWNSHIFT_STATE,
} = await import("../src/remote-desktop/adaptiveBitrate.js");

test("降档链：sharp → balanced → smooth → null（只降不升）", () => {
  assert.equal(nextLowerQuality("sharp"), "balanced");
  assert.equal(nextLowerQuality("balanced"), "smooth");
  assert.equal(nextLowerQuality("smooth"), null, "最低档无处可降");
});

test("动态降档：连 2 窗拥塞（丢帧≥3 且 rtt≥120）才触发；单维度异常不算", () => {
  const t0 = 2_000_000;
  // 丢帧够但 RTT 正常 → 不触发（先交给码率两档）。
  let s = INITIAL_QUALITY_DOWNSHIFT_STATE;
  let r = stepQualityDownshift(s, { rttMs: 30, droppedFrames: 5 }, t0);
  assert.equal(r.downshift, false);
  assert.equal(r.state.congestedStreak, 0, "非拥塞窗清零计数");
  // RTT 高但无丢帧 → 不触发。
  r = stepQualityDownshift(r.state, { rttMs: 200, droppedFrames: 0 }, t0 + 3_000);
  assert.equal(r.downshift, false);
  // 双维度拥塞：第 1 窗只计数，第 2 窗触发。
  r = stepQualityDownshift(r.state, { rttMs: 150, droppedFrames: 4 }, t0 + 6_000);
  assert.equal(r.downshift, false, "第 1 窗只计数");
  assert.equal(r.state.congestedStreak, 1);
  r = stepQualityDownshift(r.state, { rttMs: 150, droppedFrames: 4 }, t0 + 9_000);
  assert.equal(r.downshift, true, "第 2 窗触发降档");
  assert.equal(r.state.lastDownshiftAt, t0 + 9_000);
  assert.equal(r.state.congestedStreak, 0);
});

test("动态降档：冷却期内不重复触发；rtt 未测得（null）不算拥塞", () => {
  const t0 = 3_000_000;
  let r = stepQualityDownshift(INITIAL_QUALITY_DOWNSHIFT_STATE, { rttMs: null, droppedFrames: 9 }, t0);
  assert.equal(r.downshift, false, "rtt null 不算拥塞");
  // 触发一次后进入冷却：冷却内连续拥塞不触发。
  let s = { congestedStreak: 1, lastDownshiftAt: null };
  r = stepQualityDownshift(s, { rttMs: 180, droppedFrames: 5 }, t0);
  assert.equal(r.downshift, true);
  s = r.state;
  r = stepQualityDownshift(s, { rttMs: 180, droppedFrames: 5 }, t0 + 3_000);
  r = stepQualityDownshift(r.state, { rttMs: 180, droppedFrames: 5 }, t0 + 6_000);
  assert.equal(r.downshift, false, "30s 冷却内不重复触发");
  // 冷却过后（≥30s）仍拥塞 → 可再降一档。
  r = stepQualityDownshift(r.state, { rttMs: 180, droppedFrames: 5 }, t0 + 33_000);
  assert.equal(r.downshift, true, "冷却过后可再降");
});
