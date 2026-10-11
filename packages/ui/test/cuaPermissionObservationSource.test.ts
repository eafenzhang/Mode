import assert from "node:assert/strict";
import test from "node:test";

// 权限观察的来源归属判定（docs/specs/lan-remote-computer-use.md 接口变更 5）：
// identity 绑定到在册且声明 computerUse 键的 remote session → 对端来源，否则本机 base。
const { resolveCuaPermissionSource } = await import(
  "../src/cua-permission/cuaPermissionObservationSource.js"
);

const base = { tag: "base" } as never;
const remoteSession = {
  services: { tag: "remote" },
  capabilities: { computerUse: true },
} as never;

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
