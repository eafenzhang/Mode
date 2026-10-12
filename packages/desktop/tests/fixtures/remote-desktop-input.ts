/**
 * 远程桌面输入回转换算与批量夹具（坐标映射口径：lan-remote-desktop.md 产品规则 6；
 * 链路：lan-remote-desktop-native-media.md 输入面，host 侧单跳换算；坐标纯函数复用
 * shared `mapMonitorNormalizedToVirtualAbsolute`）。纯函数断言，跨平台可跑。
 */
import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

process.env.MODE_DATA_BASE_DIR = await mkdtemp(join(tmpdir(), "mode-rd-input-"));
const {
  REMOTE_DESKTOP_INJECT_MAX_EVENTS,
  createRemoteDesktopInjectBatcher,
  convertControlEvent,
  convertPointerEvent,
  normalizeWheelAmount,
  splitRemoteDesktopInjectFrames,
  toRemoteDesktopPxRect,
} = await import("../../src/host/inputConversion.js");

const single = { xPx: 0, yPx: 0, widthPx: 1920, heightPx: 1080 };
const singleVirtual = { xPx: 0, yPx: 0, widthPx: 1920, heightPx: 1080 };
// 双屏：主屏在右（物理 0..1920），副屏挂在它左边（物理 -1920..0）→ 虚拟桌面带负原点。
const rightMonitor = { xPx: 0, yPx: 0, widthPx: 1920, heightPx: 1080 };
const leftMonitor = { xPx: -1920, yPx: 0, widthPx: 1920, heightPx: 1080 };
const dualVirtual = { xPx: -1920, yPx: 0, widthPx: 3840, heightPx: 1080 };

// ── host 契约矩形（x/width 口径）→ 换算 PxRect（xPx/widthPx 口径）逐字段搬运 ──
assert.deepEqual(
  toRemoteDesktopPxRect({ x: -1920, y: 0, width: 1920, height: 1080 }),
  leftMonitor,
  "MediaStart/MediaSwitch 载荷字段名与换算口径的桥接",
);

// ── pointer：单屏中心 → 0..65535 绝对坐标 ──
assert.deepEqual(convertPointerEvent({ t: "pm", x: 0.5, y: 0.5 }, single, singleVirtual), [
  { k: "move", x: 32768, y: 32768 },
]);
// 左上 / 右下端点（(widthPx-1) 口径：1.0 精确落在最右/最下像素）
assert.deepEqual(convertPointerEvent({ t: "pm", x: 0, y: 0 }, single, singleVirtual), [
  { k: "move", x: 0, y: 0 },
]);
assert.deepEqual(convertPointerEvent({ t: "pm", x: 1, y: 1 }, single, singleVirtual), [
  { k: "move", x: 65535, y: 65535 },
]);

// ── 多屏负原点：副屏左边界映射到虚拟桌面原点，主屏右边界映射到 65535 ──
assert.deepEqual(convertPointerEvent({ t: "pm", x: 0, y: 0 }, leftMonitor, dualVirtual), [
  { k: "move", x: 0, y: 0 },
]);
assert.deepEqual(convertPointerEvent({ t: "pm", x: 1, y: 1 }, rightMonitor, dualVirtual), [
  { k: "move", x: 65535, y: 65535 },
]);
// 两屏交界：左屏右边界与主屏左边界分别落在虚拟桌面中轴两侧（不重叠、不跳到端点）。
const leftEdge = convertPointerEvent({ t: "pm", x: 1, y: 0 }, leftMonitor, dualVirtual)[0];
const rightEdge = convertPointerEvent({ t: "pm", x: 0, y: 0 }, rightMonitor, dualVirtual)[0];
assert.equal(leftEdge.x, Math.round((1919 / 3839) * 65535));
assert.equal(rightEdge.x, Math.round((1920 / 3839) * 65535));
assert.ok(leftEdge.x < 32768 && rightEdge.x > 32768, "交界两侧不得落在同一点");

// ── 越界钳制：负坐标与 >1 坐标都收敛到端点 ──
assert.deepEqual(convertPointerEvent({ t: "pm", x: -3, y: 4 }, single, singleVirtual), [
  { k: "move", x: 0, y: 65535 },
]);

// ── control：按钮下落/抬起带定位坐标，键与文本不带坐标 ──
assert.deepEqual(convertControlEvent({ t: "md", button: "left", x: 1, y: 1 }, single, singleVirtual), [
  { k: "button", button: "left", down: true, x: 65535, y: 65535 },
]);
assert.deepEqual(convertControlEvent({ t: "mu", button: "right", x: 0, y: 0 }, single, singleVirtual), [
  { k: "button", button: "right", down: false, x: 0, y: 0 },
]);
assert.deepEqual(convertControlEvent({ t: "kd", vk: 65 }, single, singleVirtual), [
  { k: "key", vk: 65, down: true },
]);
assert.deepEqual(convertControlEvent({ t: "ku", vk: 17 }, single, singleVirtual), [
  { k: "key", vk: 17, down: false },
]);
assert.deepEqual(convertControlEvent({ t: "tx", text: "中文abc" }, single, singleVirtual), [
  { k: "text", text: "中文abc" },
]);

// ── 滚轮：delta → 刻度（Rust amount 单位 = WHEEL_DELTA 刻度）；0 刻度不下发 ──
assert.deepEqual(convertControlEvent({ t: "mw", direction: "up", delta: 100, x: 0, y: 0 }, single, singleVirtual), [
  { k: "wheel", direction: "up", amount: 1, x: 0, y: 0 },
]);
assert.equal(normalizeWheelAmount(0), 0);
assert.equal(normalizeWheelAmount(100), 1);
assert.equal(normalizeWheelAmount(3), 1);
assert.equal(normalizeWheelAmount(1200), 12);
assert.equal(normalizeWheelAmount(2400), 20);
assert.equal(normalizeWheelAmount(99999), 20);
assert.ok(Math.abs(normalizeWheelAmount(1) - 1 / 3) < 1e-9, "极小 delta 至少 1/3 刻度（round 后非零）");
assert.deepEqual(
  convertControlEvent({ t: "mw", direction: "down", delta: 0, x: 0, y: 0 }, single, singleVirtual),
  [],
  "delta=0 的滚轮不产生注入事件",
);

// ── 帧拆分：≤512 一条，>512 拆帧 ──
const event = { k: "move", x: 0, y: 0 };
assert.deepEqual(splitRemoteDesktopInjectFrames([]), []);
assert.equal(splitRemoteDesktopInjectFrames(Array.from({ length: 512 }, () => event)).length, 1);
const splitFrames = splitRemoteDesktopInjectFrames(Array.from({ length: 513 }, () => event));
assert.equal(splitFrames.length, 2);
assert.equal(splitFrames[0].length, 512);
assert.equal(splitFrames[1].length, 1);
assert.equal(REMOTE_DESKTOP_INJECT_MAX_EVENTS, 512);

// ── 批量器：pointer 0ms 同 tick 聚合、control 即时 flush、爆量立即 flush、reset 丢弃 ──
const timers = [];
const frames = [];
const batcher = createRemoteDesktopInjectBatcher({
  now: () => Date.now(),
  schedule: (callback, delayMs) => {
    const timer = { callback, delayMs, cancelled: false };
    timers.push(timer);
    return timer;
  },
  cancel: (timer) => {
    if (timer && typeof timer === "object") timer.cancelled = true;
  },
  onFrame: (events) => frames.push(events),
});
const advance = () => {
  const pending = timers.splice(0, timers.length).filter((timer) => !timer.cancelled);
  for (const timer of pending) timer.callback();
};

batcher.enqueuePointer([{ k: "move", x: 1, y: 1 }]);
batcher.enqueuePointer([{ k: "move", x: 2, y: 2 }]);
assert.equal(frames.length, 0, "pointer 必须等聚合窗口（同 tick 合并，不即时 flush）");
assert.equal(timers[0].delayMs, 0, "聚合窗口 = 0ms（native-media 输入面：不人为排队）");
advance();
assert.equal(frames.length, 1);
assert.deepEqual(frames[0], [
  { k: "move", x: 1, y: 1 },
  { k: "move", x: 2, y: 2 },
]);

frames.length = 0;
batcher.enqueuePointer([{ k: "move", x: 3, y: 3 }]);
batcher.enqueueControl([{ k: "key", vk: 65, down: true }]);
assert.equal(frames.length, 1, "control 必须即时 flush");
assert.deepEqual(frames[0], [
  { k: "move", x: 3, y: 3 },
  { k: "key", vk: 65, down: true },
], "同帧内 pointer 先于 control");

frames.length = 0;
batcher.enqueuePointer(Array.from({ length: 513 }, () => ({ k: "move", x: 0, y: 0 })));
assert.equal(frames.length, 2, "爆量立即 flush 并拆帧");
assert.equal(batcher.pendingCount(), 0);

frames.length = 0;
batcher.enqueuePointer([{ k: "move", x: 9, y: 9 }]);
batcher.reset();
advance();
assert.equal(frames.length, 0, "reset 后不得补发残留输入");

console.log("REMOTE_DESKTOP_INPUT_OK");
