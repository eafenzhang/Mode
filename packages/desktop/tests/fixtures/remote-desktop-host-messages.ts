/**
 * 远程桌面 main↔host 消息契约与双端镜像夹具
 * （spec: docs/specs/remote-desktop-system-rdp.md 接口节）。
 *
 * 两条断言链：
 * 1. 漂移：`src/host/remoteDesktopMessages.ts` 与 `src/main/remoteDesktop/hostMessages.ts`
 *    是两个进程入口的镜像（无法共享单一模块），类型表、标签全集与解析判定必须逐条一致；
 * 2. 契约：现存两族的正反例、已删消息族不得复活、parentPort 分流不误吞共享消息。
 *
 * 失效模式（未实现时）：一侧改了 schema 另一侧没跟上 → 跨进程消息被静默判成「未消费」
 * 而丢弃，症状是设置面/探测面永久无回帖且无报错。
 */
import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

// 与其它桌面夹具同口径：先隔离数据目录再动态导入。
process.env.MODE_DATA_BASE_DIR = await mkdtemp(join(tmpdir(), "mode-rd-messages-"));

const host = await import("../../src/host/remoteDesktopMessages.js");
const main = await import("../../src/main/remoteDesktop/hostMessages.js");

// ────────────────────────── 漂移：镜像必须逐条一致 ──────────────────────────

assert.deepEqual(
  { ...host.RemoteDesktopHostMessageTypes },
  { ...main.RemoteDesktopHostMessageTypes },
  "两端消息类型表必须逐字一致",
);
assert.deepEqual(
  [...host.REMOTE_DESKTOP_MESSAGE_TYPE_TAGS],
  [...main.REMOTE_DESKTOP_MESSAGE_TYPE_TAGS],
  "两端类型标签全集必须逐字一致",
);

// 标签全集是冻结契约：系统 RDP 方案下只剩「被控设置读写 + 对端探测」五条。
// 加/删消息族必须同时改这里、两份镜像与 spec 接口节。
assert.deepEqual(
  [...host.REMOTE_DESKTOP_MESSAGE_TYPE_TAGS].sort(),
  [
    "remote-desktop-peer-probe",
    "remote-desktop-peer-probe-result",
    "remote-desktop-server-enabled-set",
    "remote-desktop-server-state-request",
    "remote-desktop-server-state-result",
  ],
  "消息族已变更：同步两份镜像与 spec 接口节后再改本断言",
);

// ────────────────────────── 分流：不误吞、不复活 ──────────────────────────

// 随自研媒体面删除的消息族：任何方向都不得再命中 RD 标签集合——命中的话，parentPort
// 入口会把不存在的消息判成已消费，吞掉本该走共享 schema 的帧。
for (const removed of [
  "remote-desktop-consent-result",
  "remote-desktop-signal",
  "remote-desktop-inject",
  "remote-desktop-media-start",
  "remote-desktop-session-open-result",
]) {
  assert.equal(host.isRemoteDesktopMessageTypeTag(removed), false, `${removed} 必须已移除`);
  assert.equal(main.isRemoteDesktopMessageTypeTag(removed), false, `${removed} 必须已移除`);
  assert.equal(host.parseRemoteDesktopMessageToHost({ type: removed }), null);
  assert.equal(main.parseRemoteDesktopMessageToHost({ type: removed }), null);
}

// 同一 parentPort 入口还承载共享消息：RD 集合不得误吞。
assert.equal(main.isRemoteDesktopMessageTypeTag("lan-access-get-state"), false);
assert.equal(main.parseRemoteDesktopMessageToHost({ type: "lan-access-get-state" }), null);

// ────────────────────────── 契约正例 ──────────────────────────

const toHostStateRequest = { type: "remote-desktop-server-state-request", requestId: "req-1" };
const toHostSetEnabled = {
  type: "remote-desktop-server-enabled-set",
  requestId: "req-1",
  enabled: true,
};
const toHostPeerProbe = {
  type: "remote-desktop-peer-probe",
  requestId: "req-1",
  serverId: "peer-a",
};
const fromHostStateOk = {
  type: "remote-desktop-server-state-result",
  requestId: "req-1",
  ok: true,
  sharingEnabled: false,
};
const fromHostStateWithSystemRdp = {
  type: "remote-desktop-server-state-result",
  requestId: "req-1",
  ok: true,
  sharingEnabled: true,
  systemRdp: { enabled: true, editionOk: true },
};
// ok=false 时共享位与系统状态都可缺（写失败只带 error）。
const fromHostStateFailed = {
  type: "remote-desktop-server-state-result",
  requestId: "req-1",
  ok: false,
  error: "credential-unavailable",
};
const fromHostProbeOnline = {
  type: "remote-desktop-peer-probe-result",
  requestId: "req-1",
  online: true,
  remoteDesktop: true,
  systemRdp: { enabled: false, editionOk: true },
  name: "PC-B",
};
// 探测失败一律 online:false（含缺键降级）。
const fromHostProbeOffline = {
  type: "remote-desktop-peer-probe-result",
  requestId: "req-1",
  online: false,
  remoteDesktop: false,
};

const toHostValid = [toHostStateRequest, toHostSetEnabled, toHostPeerProbe];
const fromHostValid = [
  fromHostStateOk,
  fromHostStateWithSystemRdp,
  fromHostStateFailed,
  fromHostProbeOnline,
  fromHostProbeOffline,
];

for (const sample of toHostValid) {
  assert.equal(
    host.remoteDesktopMessageToHostSchema.safeParse(sample).success,
    true,
    `正例必须通过：${JSON.stringify(sample)}`,
  );
}
for (const sample of fromHostValid) {
  assert.equal(
    host.remoteDesktopMessageFromHostSchema.safeParse(sample).success,
    true,
    `正例必须通过：${JSON.stringify(sample)}`,
  );
}

// ────────────────────────── 契约反例 + 双端判定一致 ──────────────────────────

const invalid = [
  // requestId 必填且 trim 后非空
  { type: "remote-desktop-server-state-request" },
  { type: "remote-desktop-server-state-request", requestId: "   " },
  // .strict()：未知键一律拒（这里是旧协议残留字段的典型形态）
  { type: "remote-desktop-server-state-request", requestId: "req-1", quality: "smooth" },
  // 各自必填字段缺失
  { type: "remote-desktop-server-enabled-set", requestId: "req-1" },
  { type: "remote-desktop-peer-probe", requestId: "req-1" },
  { type: "remote-desktop-peer-probe-result", requestId: "req-1", online: true },
  // systemRdp 形状不符（两字段都必填且为 boolean）
  {
    type: "remote-desktop-server-state-result",
    requestId: "req-1",
    ok: true,
    systemRdp: { enabled: "yes", editionOk: true },
  },
  { type: "remote-desktop-peer-probe-result", requestId: "req-1", online: true, remoteDesktop: true, systemRdp: { enabled: true } },
  // 未登记类型
  { type: "remote-desktop-unknown", requestId: "req-1" },
];

for (const sample of invalid) {
  const inHost = host.remoteDesktopMessageToHostSchema.safeParse(sample).success;
  const inMain = main.remoteDesktopMessageToHostSchema.safeParse(sample).success;
  assert.equal(inHost, inMain, `两端对同一帧判定必须一致：${JSON.stringify(sample)}`);
  assert.equal(inHost, false, `反例必须被拒：${JSON.stringify(sample)}`);

  const fromHostOk = host.remoteDesktopMessageFromHostSchema.safeParse(sample).success;
  const fromHostMainOk = main.remoteDesktopMessageFromHostSchema.safeParse(sample).success;
  assert.equal(fromHostOk, fromHostMainOk, `两端对同一帧判定必须一致：${JSON.stringify(sample)}`);
  assert.equal(fromHostOk, false, `反例必须被拒：${JSON.stringify(sample)}`);
}

// ────────────────────────── 分流入口行为 ──────────────────────────

const parsed = host.parseRemoteDesktopMessageToHost(toHostPeerProbe);
assert.equal(parsed?.requestId, "req-1", "标签命中且合法 → 返回解析结果");
assert.deepEqual(
  { ...parsed },
  { ...toHostPeerProbe },
  "分流不得改写载荷（原样透传给业务处理）",
);
assert.equal(
  host.parseRemoteDesktopMessageToHost({ type: "remote-desktop-peer-probe", requestId: "" }),
  null,
  "标签命中但 schema 不合法 → null（调用方按未消费处理，不双报错）",
);
assert.equal(host.parseRemoteDesktopMessageToHost(null), null);
assert.equal(host.parseRemoteDesktopMessageToHost("remote-desktop-peer-probe"), null);
assert.deepEqual(
  { ...main.parseRemoteDesktopMessageToHost(toHostStateRequest) },
  { ...toHostStateRequest },
  "main 侧镜像分流结果必须与 host 侧一致",
);

console.log("REMOTE_DESKTOP_HOST_MESSAGES_OK");
