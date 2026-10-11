import assert from "node:assert/strict";
import test from "node:test";

// 状态条读数推导（docs/specs/lan-remote-desktop.md UI 规则：延迟/帧率/分辨率来自 getStats）：
// fps 优先 framesPerSecond，缺失时用 framesDecoded 差分；RTT 只取已选中的成功候选对。
const { deriveRemoteDesktopSessionStats, EMPTY_REMOTE_DESKTOP_SESSION_STATS } =
  await import("../src/remote-desktop/sessionStats.js");

function reportOf(entries) {
  return new Map(entries.map((entry, index) => [`stats-${index}`, entry]));
}

test("无视频/无候选时返回空读数", () => {
  const sample = deriveRemoteDesktopSessionStats(reportOf([]), null);
  assert.deepEqual(sample.stats, EMPTY_REMOTE_DESKTOP_SESSION_STATS);
  assert.equal(sample.baseline, null);
});

test("优先使用 framesPerSecond，并带上帧尺寸", () => {
  const sample = deriveRemoteDesktopSessionStats(
    reportOf([
      {
        type: "inbound-rtp",
        kind: "video",
        timestamp: 1000,
        framesPerSecond: 29.6,
        framesDecoded: 300,
        frameWidth: 1920,
        frameHeight: 1080,
      },
    ]),
    null,
  );
  assert.deepEqual(sample.stats, { fps: 30, rttMs: null, width: 1920, height: 1080 });
  assert.deepEqual(sample.baseline, { frames: 300, at: 1000 });
});

test("缺少 framesPerSecond 时用 framesDecoded 差分；首轮无参照留空", () => {
  const first = deriveRemoteDesktopSessionStats(
    reportOf([{ type: "inbound-rtp", kind: "video", timestamp: 1000, framesDecoded: 100 }]),
    null,
  );
  assert.equal(first.stats.fps, null);
  assert.deepEqual(first.baseline, { frames: 100, at: 1000 });

  const second = deriveRemoteDesktopSessionStats(
    reportOf([{ type: "inbound-rtp", kind: "video", timestamp: 2000, framesDecoded: 160 }]),
    first.baseline,
  );
  assert.equal(second.stats.fps, 60);
});

test("RTT 取已选中成功候选对（秒 → 毫秒），未选中/失败候选忽略", () => {
  const sample = deriveRemoteDesktopSessionStats(
    reportOf([
      { type: "transport", timestamp: 1000 },
      { type: "candidate-pair", state: "in-progress", currentRoundTripTime: 0.001 },
      { type: "candidate-pair", state: "succeeded", nominated: false, currentRoundTripTime: 0.05 },
      { type: "candidate-pair", state: "succeeded", nominated: true, currentRoundTripTime: 0.0123 },
    ]),
    null,
  );
  assert.equal(sample.stats.rttMs, 12);
});

test("非视频的 inbound-rtp 不进入读数", () => {
  const sample = deriveRemoteDesktopSessionStats(
    reportOf([
      { type: "inbound-rtp", kind: "audio", timestamp: 1000, framesPerSecond: 50 },
      { type: "outbound-rtp", kind: "video", timestamp: 1000, framesPerSecond: 50 },
    ]),
    null,
  );
  assert.deepEqual(sample.stats, EMPTY_REMOTE_DESKTOP_SESSION_STATS);
});
