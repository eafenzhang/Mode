import assert from "node:assert/strict";
import test from "node:test";

// 状态条读数采集器（docs/specs/lan-remote-desktop-native-media.md，Task 11）：
// 数据源从 getStats 换成自有计数——fps = 采样间隔内 onDecoded 次数（sample 后清零），
// RTT = 最近一次 pong 的往返毫秒（无则 null），帧龄 = 采样窗口内中位数（无则 null），
// 分辨率 = 最近一次 decoder 配置尺寸（无则 null）。
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
  assert.deepEqual(c.sample(), { ...EMPTY_REMOTE_DESKTOP_SESSION_STATS, fps: 0 });
});

test("帧龄取窗口内中位数（奇偶两种取法）；采样后清窗", () => {
  const c = createNativeSessionStatsCollector();
  assert.equal(c.sample().frameAgeMs, null, "无样本为 null");
  [30, 90, 20].forEach((a) => c.onFrameAge(a));
  assert.equal(c.sample().frameAgeMs, 30, "奇数个取中位");
  c.onFrameAge(10);
  c.onFrameAge(21);
  assert.equal(c.sample().frameAgeMs, 16, "偶数个取中位两数均值四舍五入（15.5 → 16）");
  assert.equal(c.sample().frameAgeMs, null, "采样后清窗");
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
    frameAgeMs: null,
    width: null,
    height: null,
    bitrateKbps: null,
    decodeDrops: 0,
    decodeQueue: null,
    freezeCount: 0,
  });
});

// ---- P2-4 补充指标：码率 / 丢帧 / 解码队列 / 冻结次数 ----

test("码率 = 近一拍字节 × 8 / 间隔；首拍无基准为 null，采样后清零", () => {
  let t = 1_000_000;
  const c = createNativeSessionStatsCollector({ now: () => t });
  // 首拍：有字节但无间隔基准 → null（不能拿 0ms 当分母）。
  c.onBytesReceived(999);
  assert.equal(c.sample().bitrateKbps, null);
  // 3 秒收 3MB → 3_000_000 × 8 / 3000 = 8000 kbps。
  c.onBytesReceived(3_000_000);
  t += 3000;
  assert.equal(c.sample().bitrateKbps, 8000);
  // 窗口清零：下一拍无新字节 → 0 kbps（而非残留旧值）。
  t += 3000;
  assert.equal(c.sample().bitrateKbps, 0);
});

test("解码丢弃计数随采样清零（与自适应采样同一增量口径）", () => {
  const c = createNativeSessionStatsCollector();
  c.onFrameDropped();
  c.onFrameDropped();
  c.onFrameDropped();
  assert.equal(c.sample().decodeDrops, 3);
  assert.equal(c.sample().decodeDrops, 0);
});

test("解码队列取最近一次读数；未解码过为 null", () => {
  const c = createNativeSessionStatsCollector();
  assert.equal(c.sample().decodeQueue, null);
  c.onDecodeQueue(1);
  c.onDecodeQueue(4);
  assert.equal(c.sample().decodeQueue, 4, "取最近一次（4 是队列堆积的即时读数）");
});

test("冻结次数为会话累计值，跨采样不清零", () => {
  const c = createNativeSessionStatsCollector();
  c.onFrozen();
  assert.equal(c.sample().freezeCount, 1);
  c.onFrozen();
  assert.equal(c.sample().freezeCount, 2, "累计值不清零");
  assert.equal(c.sample().freezeCount, 2);
});
