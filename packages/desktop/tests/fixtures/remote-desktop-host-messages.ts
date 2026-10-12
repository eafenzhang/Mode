/**
 * 远程桌面 main↔host 消息契约与双端镜像夹具（spec: docs/specs/lan-remote-desktop.md 事件顺序 ②③⑤⑥）。
 *
 * 两条断言链：
 * 1. 漂移：`src/host/remoteDesktopMessages.ts` 与 `src/main/remoteDesktop/hostMessages.ts`
 *    的类型表 / 常量 / 校验结果必须逐条一致（两文件是镜像，无法共享单一模块，见文件头注释）；
 * 2. 契约：两个方向的消息形状 + 帧上限 + 注入事件严格校验 + 准入判定。
 */
import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

process.env.MODE_DATA_BASE_DIR = await mkdtemp(join(tmpdir(), "mode-rd-messages-"));
const host = await import("../../src/host/remoteDesktopMessages.js");
const main = await import("../../src/main/remoteDesktop/hostMessages.js");
const { evaluateRemoteDesktopRequest } = await import("../../src/host/remoteDesktopAdmission.js");

// ── 漂移：类型表与常量逐条一致 ──
assert.deepEqual(
  { ...host.RemoteDesktopHostMessageTypes },
  { ...main.RemoteDesktopHostMessageTypes },
  "两端消息类型表必须逐字一致",
);
assert.deepEqual(
  [...host.REMOTE_DESKTOP_MESSAGE_TYPE_TAGS].sort(),
  [...main.REMOTE_DESKTOP_MESSAGE_TYPE_TAGS].sort(),
);
assert.equal(host.REMOTE_DESKTOP_INJECT_MAX_EVENTS, main.REMOTE_DESKTOP_INJECT_MAX_EVENTS);
assert.equal(host.REMOTE_DESKTOP_SIGNAL_MAX_FRAME_BYTES, main.REMOTE_DESKTOP_SIGNAL_MAX_FRAME_BYTES);
assert.equal(host.REMOTE_DESKTOP_INJECT_MAX_EVENTS, 512, "与 Rust MAX_RAW_INPUT_EVENTS 同口径");
assert.equal(host.REMOTE_DESKTOP_SIGNAL_MAX_FRAME_BYTES, 64 * 1024);
assert.equal(main.isRemoteDesktopMessageTypeTag("lan-access-get-state"), false, "共享消息不得被 RD 集合误吞");
assert.equal(main.isRemoteDesktopMessageTypeTag(host.RemoteDesktopHostMessageTypes.RemoteDesktopInject), true);

// ── 契约样例：覆盖六个会话消息 + 设置面 + 主控对端操作 ──
const toHostSamples = [
  {
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopConsentResult,
    clientId: "client-a",
    decision: "allow",
    remember: true,
  },
  {
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopConsentResult,
    clientId: "client-a",
    decision: "deny",
    remember: false,
    reason: "timeout",
  },
  {
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopSignalToHost,
    message: {
      type: "ice",
      candidate: "candidate:1 1 udp 2113937151 192.168.1.4 51234 typ host",
      sdpMid: "0",
      sdpMLineIndex: 0,
    },
  },
  {
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopInject,
    events: [{ k: "move", x: 0, y: 65535 }, { k: "text", text: "中文" }],
  },
  { type: host.RemoteDesktopHostMessageTypes.RemoteDesktopServerStateRequest, requestId: "r1" },
  {
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopSetServerEnabled,
    requestId: "r2",
    enabled: true,
  },
  {
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopRevokeClient,
    requestId: "r3",
    clientId: "client-a",
  },
  {
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopPeerProbe,
    requestId: "r4",
    serverId: "server-x",
  },
  {
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopPeerSessionOpen,
    requestId: "r5",
    serverId: "server-x",
  },
  // P2 原生媒体面（契约 F）：开流/换屏/停流是通知型（无 requestId）。
  {
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopMediaStart,
    rect: { x: 0, y: 0, width: 2880, height: 1620 },
    key: "a".repeat(64),
    salt: "b".repeat(16),
  },
  {
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopMediaSwitch,
    rect: { x: -1920, y: 0, width: 1920, height: 1080 },
  },
  { type: host.RemoteDesktopHostMessageTypes.RemoteDesktopMediaStop },
];
const fromHostSamples = [
  {
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopConsentRequest,
    clientId: "client-a",
    label: "Office-PC",
  },
  {
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopSignalFromHost,
    message: { type: "monitors", monitors: [{ displayId: "1", label: "Screen 1", widthPx: 1920, heightPx: 1080, scaleFactor: 1, primary: true }] },
  },
  { type: host.RemoteDesktopHostMessageTypes.RemoteDesktopSessionClosed, reason: "peer-end" },
  {
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopServerStateResult,
    requestId: "r1",
    ok: true,
    enabled: true,
    authorizedClients: [{ id: "client-a", label: "Office-PC" }],
  },
  {
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopPeerProbeResult,
    requestId: "r4",
    online: true,
    remoteDesktop: true,
    name: "Peer",
  },
  {
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopPeerSessionOpenResult,
    requestId: "r5",
    ok: true,
    wsUrl: "ws://192.168.1.4:3030/ws/remote-desktop?ticket=t",
    // 契约 E 媒体面地址：两端 schema 必须同步声明（strict 漏字段会整条丢弃主控的 open 回帖）。
    mediaUrl: "wss://192.168.1.4:3030/ws/remote-desktop-media?ticket=t2",
  },
  // P2 原生媒体面（契约 F）：冻结状态通知（host → main，通知型）。
  {
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopMediaStatus,
    frozen: true,
    reason: "locked",
  },
  {
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopMediaStatus,
    frozen: false,
  },
];
for (const sample of toHostSamples) {
  const parsedByHost = host.remoteDesktopMessageToHostSchema.safeParse(sample);
  const parsedByMain = main.remoteDesktopMessageToHostSchema.safeParse(sample);
  assert.equal(parsedByHost.success, true, `host 必须接受 ${sample.type}`);
  assert.equal(parsedByMain.success, true, `main 镜像必须接受 ${sample.type}`);
  assert.deepEqual(parsedByHost.data, parsedByMain.data, `${sample.type} 两端解析结果必须一致`);
}
for (const sample of fromHostSamples) {
  const parsedByMain = main.remoteDesktopMessageFromHostSchema.safeParse(sample);
  const parsedByHost = host.remoteDesktopMessageFromHostSchema.safeParse(sample);
  assert.equal(parsedByMain.success, true, `main 必须接受 ${sample.type}`);
  assert.equal(parsedByHost.success, true, `host 镜像必须接受 ${sample.type}`);
  assert.deepEqual(parsedByMain.data, parsedByHost.data);
}

// ── 严格校验：未知键 / 越界 / 未知类型一律拒绝 ──
assert.equal(
  main.remoteDesktopMessageToHostSchema.safeParse({ ...toHostSamples[0], extra: 1 }).success,
  false,
  "未知键必须拒绝",
);
assert.equal(
  main.remoteDesktopMessageToHostSchema.safeParse({
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopInject,
    events: [{ k: "move", x: 0, y: 65536 }],
  }).success,
  false,
  "越界坐标必须拒绝（0..65535）",
);
assert.equal(
  main.remoteDesktopMessageToHostSchema.safeParse({
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopInject,
    events: [{ k: "key", vk: 0, down: true }],
  }).success,
  false,
  "vk=0 不属于 raw_input 面（1..255）",
);
assert.equal(
  main.remoteDesktopMessageToHostSchema.safeParse({
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopInject,
    events: Array.from({ length: 513 }, () => ({ k: "move", x: 0, y: 0 })),
  }).success,
  false,
  "注入帧超过 512 条必须拒绝（host 侧兜底）",
);
assert.equal(
  main.remoteDesktopMessageToHostSchema.safeParse({
    type: "remote-desktop-unknown",
  }).success,
  false,
);

// ── 媒体面消息严格校验（契约 F）：key/salt 只收 64/16 位 hex；通知型不带 requestId ──
assert.equal(
  main.remoteDesktopMessageToHostSchema.safeParse({
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopMediaStart,
    rect: { x: 0, y: 0, width: 1920, height: 1080 },
    key: "A".repeat(64),
    salt: "b".repeat(16),
  }).success,
  false,
  "key 必须是小写 hex（与 media-open 信令同一口径）",
);
assert.equal(
  host.remoteDesktopMessageToHostSchema.safeParse({
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopMediaStart,
    rect: { x: 0, y: 0, width: 1920, height: 1080 },
    key: "a".repeat(64),
    salt: "b".repeat(15),
  }).success,
  false,
  "salt 必须是 16 位 hex",
);
assert.equal(
  main.remoteDesktopMessageToHostSchema.safeParse({
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopMediaSwitch,
    rect: { x: 0, y: 0, width: 1920, height: 1080 },
    requestId: "r",
  }).success,
  false,
  "MediaSwitch 是通知型消息，多带 requestId 必须拒绝（strict）",
);
assert.equal(
  host.remoteDesktopMessageFromHostSchema.safeParse({
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopMediaStatus,
    frozen: "yes",
  }).success,
  false,
  "MediaStatus.frozen 必须是 boolean",
);
assert.equal(
  main.remoteDesktopMessageFromHostSchema.safeParse({
    type: host.RemoteDesktopHostMessageTypes.RemoteDesktopMediaStart,
    rect: { x: 0, y: 0, width: 1920, height: 1080 },
    key: "a".repeat(64),
    salt: "b".repeat(16),
  }).success,
  false,
  "main→host 消息不得被 host→main 联合体误收",
);

// ── 准入判定（产品规则 1/5）：kill-switch 优先，总开关关 = disabled ──
assert.deepEqual(
  evaluateRemoteDesktopRequest({ killSwitchEngaged: true, serverEnabled: true }),
  { action: "reject", error: "disabled" },
);
assert.deepEqual(
  evaluateRemoteDesktopRequest({ killSwitchEngaged: false, serverEnabled: false }),
  { action: "reject", error: "disabled" },
);
assert.deepEqual(
  evaluateRemoteDesktopRequest({ killSwitchEngaged: true, serverEnabled: false }),
  { action: "reject", error: "disabled" },
);
assert.deepEqual(evaluateRemoteDesktopRequest({ killSwitchEngaged: false, serverEnabled: true }), {
  action: "consent",
});

console.log("REMOTE_DESKTOP_HOST_MESSAGES_OK");
