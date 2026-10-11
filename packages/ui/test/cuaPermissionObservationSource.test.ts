import assert from "node:assert/strict";
import test from "node:test";

// 权限观察的来源归属判定（docs/specs/lan-remote-computer-use.md 接口变更 5、事件顺序 ④）：
// identity 绑定到在册且声明 computerUse 键的 remote session → 对端来源，否则本机 base；
// 投递来源门（shouldForwardCuaPermissionObservationToSource）进一步约束「哪个订阅收到的
// 观察允许被处理」——对端 session 订阅是 host-wide 广播，只接受归属该 session 的 identity。
const { resolveCuaPermissionSource, shouldForwardCuaPermissionObservationToSource } =
  await import("../src/cua-permission/cuaPermissionObservationSource.js");

const base = { tag: "base" } as never;
const remoteSession = {
  services: { tag: "remote" },
  capabilities: { computerUse: true },
} as never;
const LAN_OBSERVATION = { workspaceIdentity: "remote:lan:192.168.1.5:3030:/tmp/ws" };

test("identity 绑定到 LAN 新对端 session → 对端来源", () => {
  const result = resolveCuaPermissionSource({
    baseServices: base,
    observation: { workspaceIdentity: "remote:lan:192.168.1.5:3030:/tmp/ws" },
    sessionsById: { s1: remoteSession as never },
    sessionIdByWorkspaceIdentity: { "remote:lan:192.168.1.5:3030:/tmp/ws": "s1" },
  });
  assert.equal(result.remote, true);
  assert.deepEqual(result.services, { tag: "remote" });
});

test("无绑定 / 旧对端（无 capabilities）→ 本机来源", () => {
  for (const sessionsById of [{}, { s1: { services: { tag: "remote" } } as never }]) {
    const result = resolveCuaPermissionSource({
      baseServices: base,
      observation: { workspaceIdentity: "remote:lan:192.168.1.5:3030:/tmp/ws" },
      sessionsById,
      sessionIdByWorkspaceIdentity: { "remote:lan:192.168.1.5:3030:/tmp/ws": "s1" },
    });
    assert.equal(result.remote, false);
    assert.deepEqual(result.services, base);
  }
});

test("本机 workspace（无 identity）→ 本机来源", () => {
  const result = resolveCuaPermissionSource({
    baseServices: base,
    observation: {},
    sessionsById: {},
    sessionIdByWorkspaceIdentity: {},
  });
  assert.equal(result.remote, false);
});

// ── 投递来源门（事件顺序 ④：对端 host 广播的观察按订阅来源归属，非本会话身份一律丢弃）──

test("对端投递 + identity 归属该 session → 放行并路由到该 session 服务", () => {
  const input = {
    deliveredBy: { sessionId: "s1" },
    observation: LAN_OBSERVATION,
    sessionsById: { s1: remoteSession as never },
    sessionIdByWorkspaceIdentity: { [LAN_OBSERVATION.workspaceIdentity]: "s1" },
  };
  assert.equal(shouldForwardCuaPermissionObservationToSource(input), true);
  // 放行后 resolve 必须落回投递该观察的 session 的服务，而不是本机 base。
  const source = resolveCuaPermissionSource({
    baseServices: base,
    observation: input.observation,
    sessionsById: input.sessionsById,
    sessionIdByWorkspaceIdentity: input.sessionIdByWorkspaceIdentity,
  });
  assert.equal(source.remote, true);
  assert.deepEqual(source.services, { tag: "remote" });
});

test("对端投递 + 无 identity 观察（对端本机 workspace）→ 丢弃", () => {
  assert.equal(
    shouldForwardCuaPermissionObservationToSource({
      deliveredBy: { sessionId: "s1" },
      observation: {},
      sessionsById: { s1: remoteSession as never },
      sessionIdByWorkspaceIdentity: { [LAN_OBSERVATION.workspaceIdentity]: "s1" },
    }),
    false,
  );
});

test("对端投递 + identity 归属其他 session → 丢弃", () => {
  assert.equal(
    shouldForwardCuaPermissionObservationToSource({
      deliveredBy: { sessionId: "s1" },
      observation: LAN_OBSERVATION,
      sessionsById: { s1: remoteSession as never, s2: remoteSession as never },
      sessionIdByWorkspaceIdentity: { [LAN_OBSERVATION.workspaceIdentity]: "s2" },
    }),
    false,
  );
});

test("对端投递 + identity 未绑定任何 session → 丢弃", () => {
  assert.equal(
    shouldForwardCuaPermissionObservationToSource({
      deliveredBy: { sessionId: "s1" },
      observation: LAN_OBSERVATION,
      sessionsById: { s1: remoteSession as never },
      sessionIdByWorkspaceIdentity: {},
    }),
    false,
  );
});

test("对端投递 + identity 归属该 session 但 session 已不在册 → 丢弃", () => {
  assert.equal(
    shouldForwardCuaPermissionObservationToSource({
      deliveredBy: { sessionId: "s1" },
      observation: LAN_OBSERVATION,
      sessionsById: {},
      sessionIdByWorkspaceIdentity: { [LAN_OBSERVATION.workspaceIdentity]: "s1" },
    }),
    false,
  );
});

test("base 投递 + 无 identity 本机观察 → 放行到 base（回归：本机既有行为不变）", () => {
  assert.equal(
    shouldForwardCuaPermissionObservationToSource({
      deliveredBy: "base",
      observation: {},
      sessionsById: {},
      sessionIdByWorkspaceIdentity: {},
    }),
    true,
  );
  const source = resolveCuaPermissionSource({
    baseServices: base,
    observation: {},
    sessionsById: {},
    sessionIdByWorkspaceIdentity: {},
  });
  assert.equal(source.remote, false);
  assert.deepEqual(source.services, base);
});
