/**
 * 被控媒体面行为夹具（Task 6 fix round 1：审查 Important 1/2 的行为断言）。
 * 失效模式：reason 归一矩阵错 / start-capture 先启动后信令 / select-monitor 路由错 /
 * peer-session-open 回帖在 main 侧丢 mediaUrl（契约 E 断链，renderer 永远连不上 media WS）。
 * 纯函数断言，跨平台可跑（不引 Electron 运行时）。
 */
import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

process.env.MODE_DATA_BASE_DIR = await mkdtemp(join(tmpdir(), "mode-rd-coordinator-"));
const media = await import("../../src/main/remoteDesktop/mediaCoordination.js");
const { buildPeerSessionOpenOutcome } = await import(
  "../../src/main/remoteDesktop/peerSessionOpenOutcome.js"
);
const hostMirror = await import("../../src/host/remoteDesktopMessages.js");
const mainMirror = await import("../../src/main/remoteDesktop/hostMessages.js");
const shared = await import("@mode/shared/remote-desktop");

// ── Important 1a：冻结 reason 归一矩阵（协议面 union vs helper 实现面错误串）──
assert.equal(media.normalizeMediaFreezeReason("locked"), "locked", "locked 保真");
assert.equal(media.normalizeMediaFreezeReason("capture-error"), "access-lost", "采集错误归 access-lost");
assert.equal(media.normalizeMediaFreezeReason("encode-error"), "access-lost", "编码错误归 access-lost");
assert.equal(media.normalizeMediaFreezeReason("access-lost"), "access-lost");
assert.equal(media.normalizeMediaFreezeReason(undefined), "access-lost", "无 reason 归 access-lost");

// ── Important 1a'：frozen 布尔分支的完整出站规划（fix round 2：unfreeze 分支与 freeze 对称断言）──
assert.deepEqual(
  media.buildMediaStatusOutbound(true, "locked"),
  [{ kind: "signal", message: { type: "freeze", reason: "locked" } }],
  "frozen:true + locked → freeze{locked} 保真",
);
assert.deepEqual(
  media.buildMediaStatusOutbound(true, "capture-error"),
  [{ kind: "signal", message: { type: "freeze", reason: "access-lost" } }],
  "frozen:true + 实现面错误串 → freeze{access-lost} 归并",
);
assert.deepEqual(
  media.buildMediaStatusOutbound(true),
  [{ kind: "signal", message: { type: "freeze", reason: "access-lost" } }],
  "frozen:true 无 reason → freeze{access-lost}",
);
assert.deepEqual(
  media.buildMediaStatusOutbound(false),
  [{ kind: "signal", message: { type: "unfreeze" } }],
  "frozen:false → unfreeze",
);
assert.deepEqual(
  media.buildMediaStatusOutbound(false, "locked"),
  [{ kind: "signal", message: { type: "unfreeze" } }],
  "frozen:false 带 reason（畸形输入）仍只出 unfreeze，不得夹带冻结理由",
);
assert.equal(
  shared.remoteDesktopSignalMessageSchema.safeParse(
    media.buildMediaStatusOutbound(true, "locked")[0]?.message,
  ).success,
  true,
  "freeze 信令过契约 D",
);

// ── Important 1b：start-capture 出站顺序 = 先 media-open 信令、后 MediaStart（spec 事件顺序 ②）──
const key = "a".repeat(64);
const salt = "b".repeat(16);
const rect = { x: 0, y: 0, width: 2880, height: 1620 };
// fix round 1：MediaStart/Switch 同时携带 virtualRect（host 输入换算基准），与 rect 同帧同源。
const virtualRect = { x: -1920, y: 0, width: 4800, height: 1620 };
const startSequence = media.buildStartCaptureOutbound({ rect, virtualRect, key, salt });
assert.deepEqual(
  startSequence.map((outbound) => outbound.kind),
  ["signal", "host"],
  "先信令（media-open 带 key/salt）后启动（MediaStart）",
);
assert.deepEqual(startSequence[0]?.message, { type: "media-open", key, salt }, "media-open 载荷与 MediaStart 同源");
assert.equal(shared.remoteDesktopSignalMessageSchema.safeParse(startSequence[0]?.message).success, true, "media-open 过契约 D");
const hostStart = startSequence[1]?.message;
assert.equal(mainMirror.remoteDesktopMessageToHostSchema.safeParse(hostStart).success, true, "MediaStart 过 main 侧契约 F");
assert.equal(hostMirror.remoteDesktopMessageToHostSchema.safeParse(hostStart).success, true, "MediaStart 过 host 侧契约 F（镜像一致）");
assert.deepEqual(
  (hostStart as { virtualRect?: unknown }).virtualRect,
  virtualRect,
  "MediaStart 必须带输入换算基准 virtualRect（fix round 1）",
);

// ── Important 1c：select-monitor 路由——解析成功 → MediaSwitch{rect,virtualRect}；失败 → error{internal} ──
const switchRect = { x: -1920, y: 0, width: 1920, height: 1080 };
const switchOutbound = media.buildSelectMonitorOutbound({ rect: switchRect, virtualRect });
assert.deepEqual(switchOutbound.map((outbound) => outbound.kind), ["host"]);
assert.deepEqual(switchOutbound[0]?.message, {
  type: "remote-desktop-media-switch",
  rect: switchRect,
  virtualRect,
});
assert.equal(mainMirror.remoteDesktopMessageToHostSchema.safeParse(switchOutbound[0]?.message).success, true, "MediaSwitch 过 main 侧契约 F");
assert.equal(hostMirror.remoteDesktopMessageToHostSchema.safeParse(switchOutbound[0]?.message).success, true, "MediaSwitch 过 host 侧契约 F（镜像一致）");
const failedOutbound = media.buildSelectMonitorOutbound(null);
assert.deepEqual(failedOutbound.map((outbound) => outbound.kind), ["signal"]);
assert.deepEqual(failedOutbound[0]?.message, { type: "error", code: "internal", message: "unknown monitor" });
assert.equal(shared.remoteDesktopSignalMessageSchema.safeParse(failedOutbound[0]?.message).success, true, "error 信令过契约 D");

// ── Important 2：peer-session-open 回帖的 main 侧载荷映射——mediaUrl 必须透传（契约 E）──
const openResult = {
  type: hostMirror.RemoteDesktopHostMessageTypes.RemoteDesktopPeerSessionOpenResult,
  requestId: "r5",
  ok: true,
  wsUrl: "ws://192.168.1.4:3030/ws/remote-desktop?ticket=t",
  mediaUrl: "ws://192.168.1.4:3030/ws/remote-desktop-media?mediaTicket=t2",
};
// 入站先过 schema（strict：带 mediaUrl 的回帖不得被丢弃），再过 settle 载荷映射。
const parsedOpen = mainMirror.remoteDesktopMessageFromHostSchema.safeParse(openResult);
assert.equal(parsedOpen.success, true, "main 侧必须接受带 mediaUrl 的回帖");
assert.ok(parsedOpen.success);
const outcome = buildPeerSessionOpenOutcome(parsedOpen.data);
assert.equal(outcome.ok, true);
assert.equal(outcome.ok && outcome.mediaUrl, openResult.mediaUrl, "mediaUrl 必须透传到 pending resolve 载荷");
assert.equal(outcome.ok && outcome.wsUrl, openResult.wsUrl);

// 旧 host 无 mediaUrl：载荷不得出现 mediaUrl 键（renderer 按「无媒体地址」收口）。
const legacyOutcome = buildPeerSessionOpenOutcome({
  ...openResult,
  mediaUrl: undefined,
} as typeof parsedOpen.data);
assert.equal("mediaUrl" in legacyOutcome, false, "缺省时不得带 mediaUrl 键");

// 失败分支：errorCode / error 原样带出，error 缺省时给稳定文案。
assert.deepEqual(
  buildPeerSessionOpenOutcome({
    type: hostMirror.RemoteDesktopHostMessageTypes.RemoteDesktopPeerSessionOpenResult,
    requestId: "r6",
    ok: false,
    errorCode: "disabled",
    error: "被控未开启",
  }),
  { ok: false, errorCode: "disabled", error: "被控未开启" },
);
assert.deepEqual(
  buildPeerSessionOpenOutcome({
    type: hostMirror.RemoteDesktopHostMessageTypes.RemoteDesktopPeerSessionOpenResult,
    requestId: "r7",
    ok: false,
  }),
  { ok: false, error: "对端会话请求失败" },
);

console.log("REMOTE_DESKTOP_COORDINATOR_OK");
