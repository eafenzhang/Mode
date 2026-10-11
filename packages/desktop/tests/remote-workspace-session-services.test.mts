import { test } from "node:test";
import assert from "node:assert/strict";
import { buildRemoteWorkspaceSessionServices } from "../src/renderer/src/remoteWorkspaceSessionServices.ts";

function makeAccessor(overrides: Record<string, unknown> = {}) {
  return { cuaPermissionService: { kind: "base" }, ...overrides } as never;
}

// 权限服务归属：LAN 新对端（capabilities.computerUse 为布尔，键出现即新版对端且其
// createLocalServices 恒注册该 channel）→ 用对端服务；旧对端/ssh → 维持本机 base 回落
// （docs/specs/lan-remote-computer-use.md「权限服务透传与归属」）。
test("LAN 新对端：cuaPermissionService 归属覆盖为对端服务", () => {
  const base = makeAccessor();
  const remote = makeAccessor({ cuaPermissionService: { kind: "remote" } });
  const merged = buildRemoteWorkspaceSessionServices(base, remote, {
    capabilities: { computerUse: true },
  });
  assert.deepEqual(merged.cuaPermissionService, { kind: "remote" });
});

test("能力位为 false（新对端 kill-switch 关）仍归属对端：状态查询是对端事实", () => {
  const merged = buildRemoteWorkspaceSessionServices(
    makeAccessor(),
    makeAccessor({ cuaPermissionService: { kind: "remote" } }),
    { capabilities: { computerUse: false } },
  );
  assert.deepEqual(merged.cuaPermissionService, { kind: "remote" });
});

test("无 capabilities（旧对端 / ssh）维持本机 base 回落", () => {
  const base = makeAccessor();
  const merged = buildRemoteWorkspaceSessionServices(
    base,
    makeAccessor({ cuaPermissionService: { kind: "remote" } }),
    {},
  );
  assert.deepEqual(merged.cuaPermissionService, { kind: "base" });
  const mergedNoOptions = buildRemoteWorkspaceSessionServices(
    base,
    makeAccessor({ cuaPermissionService: { kind: "remote" } }),
  );
  assert.deepEqual(mergedNoOptions.cuaPermissionService, { kind: "base" });
});
