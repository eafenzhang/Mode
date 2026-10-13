import assert from "node:assert/strict";
import test from "node:test";

// 首帧看门狗（spec: docs/specs/lan-remote-desktop-native-media.md「媒体面可观测性与首帧约束」）：
// 「媒体 WS 已开但一帧没画」必须可日志、可上屏——2026-10-14 实机实证：被控未出流时主控侧是
// 「画面已连接 / 0 fps / 0.0 Mbps」的无提示纯黑，且双端日志零行。
const { createFirstFrameWatchdog, shouldReportNoFirstFrame, FIRST_FRAME_WATCHDOG_MS } =
  await import("../src/remote-desktop/firstFrameWatchdog.js");

test("判定：窗口内一帧未画才报，画过就不报", () => {
  assert.equal(FIRST_FRAME_WATCHDOG_MS, 5_000);
  assert.equal(shouldReportNoFirstFrame(FIRST_FRAME_WATCHDOG_MS - 1, 0), false, "窗口未到不报");
  assert.equal(shouldReportNoFirstFrame(FIRST_FRAME_WATCHDOG_MS, 0), true, "窗口到期且零帧则报");
  assert.equal(shouldReportNoFirstFrame(FIRST_FRAME_WATCHDOG_MS, 1), false, "已画过帧不报");
  assert.equal(shouldReportNoFirstFrame(FIRST_FRAME_WATCHDOG_MS * 60, 1), false, "静置后也不报");
});

test("看门狗：窗口到期零帧回调一次，画过帧/收口后不再回调", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const events = [];
  let stopped = false;
  let bytes = 0;
  const watchdog = createFirstFrameWatchdog({
    bytesReceived: () => bytes,
    isStopped: () => stopped,
    onNoFrames: (info) => events.push(info),
  });

  // 窗口未到不报。
  watchdog.arm();
  t.mock.timers.tick(FIRST_FRAME_WATCHDOG_MS - 1);
  assert.equal(events.length, 0);

  // 到期且零帧：报一次，带上累计收流字节（区分「没数据」与「有数据但解不出画面」）。
  bytes = 4096;
  t.mock.timers.tick(1);
  assert.deepEqual(events, [{ waitedMs: FIRST_FRAME_WATCHDOG_MS, bytesReceived: 4096 }]);
  // 每个 socket 至多一次：再走一个窗口也不重复刷屏。
  t.mock.timers.tick(FIRST_FRAME_WATCHDOG_MS);
  assert.equal(events.length, 1);

  // 画过帧的 socket 不报；noteFrame 只对「本 socket 首帧」返回 true（调用方据此撤提示）。
  watchdog.arm();
  assert.equal(watchdog.noteFrame(), true, "本 socket 首帧");
  assert.equal(watchdog.noteFrame(), false, "后续帧不是首帧");
  t.mock.timers.tick(FIRST_FRAME_WATCHDOG_MS * 2);
  assert.equal(events.length, 1, "画过帧不报");

  // 收口闸：已收口的会话即使定时器到点也不报（与 stop 竞争的最后一道闸）。
  watchdog.arm();
  stopped = true;
  t.mock.timers.tick(FIRST_FRAME_WATCHDOG_MS);
  assert.equal(events.length, 1, "收口后不报");

  // clear 后不触发；重新 arm（重连新 socket）重新计时。
  stopped = false;
  watchdog.arm();
  watchdog.clear();
  t.mock.timers.tick(FIRST_FRAME_WATCHDOG_MS * 2);
  assert.equal(events.length, 1, "clear 后不再触发");
  watchdog.arm();
  t.mock.timers.tick(FIRST_FRAME_WATCHDOG_MS);
  assert.equal(events.length, 2, "重新 arm 后重新计时");
  t.mock.timers.reset();
});
