/**
 * 局域网访问状态读取夹具（spec: docs/specs/lan-paired-devices.md「契约：LanAccessClientRecord
 * 与状态读取」；2026-10-13 超时修复的回归）：
 * 1. 监听未开启时的状态快照（readDisabledLanAccessState）= clients 从凭据读取、
 *    port/addresses/pairCode 为空——设置页「已配对设备」单列表不依赖服务端在跑；
 * 2. 该快照（含 remoteDesktopAuthorized）必须能通过 main 侧的 host 回帖 schema——
 *    strict schema 漏声明该键时 main 会整条拒收 lan-access-state-result，UI 表现为
 *    `mode:get-lan-access-state` 8s 超时（prod 日志实证：state.clients.0: Unrecognized
 *    key "remoteDesktopAuthorized"）。
 */
import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

process.env.MODE_DATA_BASE_DIR = await mkdtemp(join(tmpdir(), "mode-lan-disabled-state-"));
const { readDisabledLanAccessState } = await import("../../src/host/lanAccessServer.js");
const { LAN_ACCESS_CLIENTS_CREDENTIAL_KEY, hostLanAccessStateResponseSchema } = await import(
  "@mode/shared"
);

/** 最小凭据服务假件：只实现 load（readLanAccessClientRecords 的唯一读口）。 */
const fakeCredentials = (stored) =>
  ({
    load: async (key) => (key === LAN_ACCESS_CLIENTS_CREDENTIAL_KEY ? stored : null),
  });

const storedClients = [
  {
    id: "client-a",
    label: "Office-PC",
    createdAt: 1_700_000_000_000,
    lastUsedAt: 1_700_000_100_000,
    lastWorkspacePath: "D:\\work\\demo",
    remoteDesktopAuthorized: true,
  },
  { id: "client-b", label: "Laptop", createdAt: 1_700_000_000_000, lastUsedAt: null },
];

// ── 1. 关开关的快照：clients 读凭据、其余字段为空 ──
const state = await readDisabledLanAccessState(
  fakeCredentials(JSON.stringify(storedClients)),
);
assert.equal(state.enabled, false);
assert.equal(state.port, null);
assert.deepEqual(state.addresses, []);
assert.equal(state.pairCode, null);
assert.equal(state.clients.length, 2, "配对记录不随监听开关消失");
assert.equal(state.clients[0].remoteDesktopAuthorized, true, "历史字段原样保留（不抹掉老数据）");
assert.equal(state.clients[0].lastWorkspacePath, "D:\\work\\demo");
assert.equal(state.clients[1].remoteDesktopAuthorized, undefined, "无该字段的记录不受影响");

// ── 2. 空/畸形记录：读失败降级为空列表，不抛错 ──
assert.deepEqual((await readDisabledLanAccessState(fakeCredentials(null))).clients, []);
assert.deepEqual((await readDisabledLanAccessState(fakeCredentials("{oops"))).clients, []);

// ── 3. 回帖形状（回归核心）：带 remoteDesktopAuthorized 的 state 必须整条可解析 ──
const parsed = hostLanAccessStateResponseSchema.safeParse({
  type: "lan-access-state-result",
  requestId: "r1",
  ok: true,
  state,
});
assert.equal(parsed.success, true, "host 回帖必须能过 main 的 strict schema（否则 8s 超时）");

console.log("LAN_ACCESS_DISABLED_STATE_OK");
