import assert from "node:assert/strict";
import test from "node:test";

// 状态条读数采集器（docs/specs/lan-remote-desktop-native-media.md，Task 11）：
// 数据源从 getStats 换成自有计数——fps = 采样间隔内 onDecoded 次数（sample 后清零），
// RTT = 最近一次 pong 的往返毫秒（无则 null），分辨率 = 最近一次 decoder 配置尺寸（无则 null）。
const { createNativeSessionStatsCollector, EMPTY_REMOTE_DESKTOP_SESSION_STATS } =
  await import("../src/remote-desktop/sessionStats.js");

test("fps = 采样间隔内解码帧数；连续采样重置计数", () => {
  const c = createNativeSessionStatsCollector();
  for (let i = 0; i < 60; i++) c.onDecoded();
  const s = c.sample();
  assert.equal(s.fps, 60);
  assert.equal(c.sample().fps, 0, "第二次采样无新帧 → 0");
});

test("首次采样前无解码也返回 fps 0（非 null，避免状态条把未出帧误读成空）", () => {
  const c = createNativeSessionStatsCollector();
  assert.equal(c.sample().fps, 0);
});

test("rtt 取最近一次 pong；分辨率来自 decoder 配置", () => {
  const c = createNativeSessionStatsCollector();
  c.onPong(17);
  c.onPong(19);
  c.onConfigured(1920, 1080);
  const s = c.sample();
  assert.equal(s.rttMs, 19);
  assert.equal(s.width, 1920);
  assert.equal(s.height, 1080);
});

test("未收到 pong / 配置前 rtt 与分辨率留空", () => {
  const c = createNativeSessionStatsCollector();
  assert.deepEqual(c.sample(), { fps: 0, rttMs: null, width: null, height: null });
});

test("换分辨率后读数跟随最近一次配置", () => {
  const c = createNativeSessionStatsCollector();
  c.onConfigured(1280, 720);
  c.onConfigured(2560, 1440);
  const s = c.sample();
  assert.equal(s.width, 2560);
  assert.equal(s.height, 1440);
});

test("空态导出形状不变", () => {
  assert.deepEqual(EMPTY_REMOTE_DESKTOP_SESSION_STATS, {
    fps: null,
    rttMs: null,
    width: null,
    height: null,
  });
});
