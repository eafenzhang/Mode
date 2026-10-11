import assert from "node:assert/strict";
import test from "node:test";

const { shouldForwardCuaPermissionObservation } = await import(
  "../src/mode-agent/modeAgentService.js"
);
const { createModeAgentConnectionScope } = await import(
  "../src/mode-agent/modeAgentConnectionScope.js"
);
import type { IModeAgentService } from "../src/mode-agent/modeAgent.js";

// ---- base 过滤（对端 host 进程内）----
test("base 过滤：本机与 remote:lan 放行；ssh/wsl/docker 与 remoteSessionId 丢弃", () => {
  assert.equal(shouldForwardCuaPermissionObservation({}), true);
  assert.equal(
    shouldForwardCuaPermissionObservation({
      workspaceIdentity: "remote:lan:192.168.1.5:3030:/tmp/ws",
    }),
    true,
  );
  assert.equal(
    shouldForwardCuaPermissionObservation({ workspaceIdentity: "remote:ssh:h:22:/tmp/ws" }),
    false,
  );
  assert.equal(
    shouldForwardCuaPermissionObservation({ workspaceIdentity: "remote:wsl:Ubuntu:/tmp/ws" }),
    false,
  );
  assert.equal(
    shouldForwardCuaPermissionObservation({ workspaceIdentity: "remote:docker:c:/tmp/ws" }),
    false,
  );
  assert.equal(shouldForwardCuaPermissionObservation({ remoteSessionId: "rs-1" }), false);
});

// ---- scope 门（连接面）----
function makeBase() {
  const calls: unknown[] = [];
  const base = {
    onDynamicCuaPermissionObservation: (params?: unknown) => {
      calls.push(params);
      return { dispose: () => {} };
    },
  } as unknown as IModeAgentService;
  return { base, calls };
}
const continuous = { connectionId: "conn-1", clientMode: "desktop-continuous" as const };

test("scope 门：terminal-client + desktop-continuous 放行并携带 trusted 载体", () => {
  const { base, calls } = makeBase();
  const scope = createModeAgentConnectionScope(base, { ...continuous });
  scope.service.onDynamicCuaPermissionObservation();
  assert.equal(calls.length, 1);
  const carrier = calls[0] as {
    __modeTrustedV4Connection?: { clientMode?: string };
  };
  assert.equal(carrier.__modeTrustedV4Connection?.clientMode, "desktop-continuous");
  scope.dispose();
});

test("scope 门：trusted-host-relay + 可信 desktop-continuous 下游放行（LAN 先例）", () => {
  const { base, calls } = makeBase();
  const scope = createModeAgentConnectionScope(base, {
    connectionId: "server-ws-1",
    clientMode: "desktop-continuous",
    role: "trusted-host-relay",
  });
  scope.service.onDynamicCuaPermissionObservation({
    __modeTrustedV4Connection: { connectionId: "host-rpc-1", clientMode: "desktop-continuous" },
  });
  assert.equal(calls.length, 1);
  scope.dispose();
});

test("scope 门：relay 无可信下游 → RpcEvent.None，不落 base", () => {
  const { base, calls } = makeBase();
  const scope = createModeAgentConnectionScope(base, {
    connectionId: "server-ws-1",
    clientMode: "desktop-continuous",
    role: "trusted-host-relay",
  });
  scope.service.onDynamicCuaPermissionObservation(undefined);
  assert.equal(calls.length, 0);
  scope.dispose();
});

test("scope 门：relay 下游是手机 replay → 拒绝", () => {
  const { base, calls } = makeBase();
  const scope = createModeAgentConnectionScope(base, {
    connectionId: "server-ws-1",
    clientMode: "desktop-continuous",
    role: "trusted-host-relay",
  });
  scope.service.onDynamicCuaPermissionObservation({
    __modeTrustedV4Connection: { connectionId: "att-1", clientMode: "web-remote-replayable" },
  });
  assert.equal(calls.length, 0);
  scope.dispose();
});

test("scope 门：本连接 clientMode 为 replayable（手机 attachment）恒拒绝", () => {
  const { base, calls } = makeBase();
  const scope = createModeAgentConnectionScope(base, {
    connectionId: "att-1",
    clientMode: "web-remote-replayable",
  });
  scope.service.onDynamicCuaPermissionObservation();
  assert.equal(calls.length, 0);
  scope.dispose();
});
