import assert from "node:assert/strict";
import test from "node:test";

// 生产者帧率回调（docs/specs/remote-desktop-latency.md §2.1）：解压压力降档、
// 健康采样回升、10s 驻留防抖。纯函数，时钟注入。
const { INITIAL_PRODUCER_RATE_STATE, MIN_PRODUCER_FPS, PRODUCER_DWELL_MS, stepProducerRate } =
  await import("../src/remote-desktop/producerRate.js");

test("解码端持续丢帧：60 → 30 → 15，最低不再降", () => {
  let state = { ...INITIAL_PRODUCER_RATE_STATE };
  let now = 0;
  const first = stepProducerRate(state, { baseFps: 60, droppedFrames: 5, rttMs: 40 }, now);
  assert.equal(first.nextFps, 30);
  state = first.state;

  // 驻留期内继续拥塞：不再降（避免一拍一档把 fps 打到地板）。
  const second = stepProducerRate(state, { baseFps: 60, droppedFrames: 9, rttMs: 40 }, now + 1_000);
  assert.equal(second.nextFps, null);
  assert.equal(second.state.fps, 30);

  now += PRODUCER_DWELL_MS;
  const third = stepProducerRate(second.state, { baseFps: 60, droppedFrames: 4, rttMs: 40 }, now);
  assert.equal(third.nextFps, 15);

  now += PRODUCER_DWELL_MS;
  const floor = stepProducerRate(third.state, { baseFps: 60, droppedFrames: 4, rttMs: 40 }, now);
  assert.equal(floor.nextFps, null);
  assert.equal(floor.state.fps, MIN_PRODUCER_FPS);
});

test("恢复：连续健康 + 驻留后逐级翻倍，到顶回到预设", () => {
  let state = { fps: 15, lastAdjustAt: 0, healthyStreak: 0 };
  let now = PRODUCER_DWELL_MS;
  // 前 3 次健康不够 streak（4 次才达标）。
  for (let i = 0; i < 3; i += 1) {
    const step = stepProducerRate(state, { baseFps: 60, droppedFrames: 0, rttMs: 20 }, now + i);
    assert.equal(step.nextFps, null);
    state = step.state;
  }
  const up = stepProducerRate(state, { baseFps: 60, droppedFrames: 0, rttMs: 20 }, now + 3);
  assert.equal(up.nextFps, 30);
  state = up.state;

  now = now + 3 + PRODUCER_DWELL_MS;
  let recovered: ReturnType<typeof stepProducerRate> | null = null;
  for (let i = 0; i < 4; i += 1) {
    recovered = stepProducerRate(state, { baseFps: 60, droppedFrames: 0, rttMs: 20 }, now + i);
    state = recovered.state;
  }
  assert.equal(recovered?.nextFps, 60, "翻倍到预设并下发一次");
  assert.equal(recovered?.state.fps, null, "到顶后回到「跟随预设」");
});

test("RTT 未测得不算健康；有丢帧立即清零 streak", () => {
  let state = { fps: 30, lastAdjustAt: 0, healthyStreak: 3 };
  const step = stepProducerRate(
    state,
    { baseFps: 60, droppedFrames: 0, rttMs: null },
    PRODUCER_DWELL_MS,
  );
  assert.equal(step.nextFps, null);
  assert.equal(step.state.healthyStreak, 0);

  state = stepProducerRate(
    { ...state, healthyStreak: 3 },
    { baseFps: 60, droppedFrames: 1, rttMs: 20 },
    PRODUCER_DWELL_MS,
  ).state;
  assert.equal(state.healthyStreak, 0);
});
