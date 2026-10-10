import assert from "node:assert/strict";
import test from "node:test";

const { resolveComputerUseAvailability, isRemoteCuaPermissionRowVisible } =
  await import("../src/settings/computerUseAvailability.js");

const lanTarget = { kind: "lan" as const, host: "192.168.1.5", port: 3030 };
const lanIdentity = "remote:lan:192.168.1.5:3030:/tmp/ws";

test("LAN + 能力位 true → remote-lan supported", () => {
  assert.deepEqual(
    resolveComputerUseAvailability({
      isDesktop: true,
      isWindowsDesktop: true,
      remoteTarget: lanTarget,
      lanComputerUseCapable: true,
    }),
    { kind: "remote-lan", supported: true },
  );
});

test("LAN 无能力位（旧对端）→ remote-lan unsupported", () => {
  assert.deepEqual(
    resolveComputerUseAvailability({
      isDesktop: true,
      isWindowsDesktop: true,
      remoteTarget: lanTarget,
    }),
    { kind: "remote-lan", supported: false },
  );
});

test("LAN 能力位 false（对端 kill-switch 关）→ unsupported", () => {
  assert.deepEqual(
    resolveComputerUseAvailability({
      isDesktop: true,
      isWindowsDesktop: true,
      remoteTarget: lanTarget,
      lanComputerUseCapable: false,
    }),
    { kind: "remote-lan", supported: false },
  );
});

test("仅 identity 为 remote:lan（历史恢复无 target）也走 remote-lan 分支", () => {
  assert.deepEqual(
    resolveComputerUseAvailability({
      isDesktop: true,
      isWindowsDesktop: true,
      workspaceIdentity: lanIdentity,
      lanComputerUseCapable: true,
    }),
    { kind: "remote-lan", supported: true },
  );
});

test("ssh 维持 remote-ssh unsupported（回归）", () => {
  assert.deepEqual(
    resolveComputerUseAvailability({
      isDesktop: true,
      isWindowsDesktop: true,
      remoteTarget: { kind: "ssh", host: "h", port: 22, username: "u" },
      lanComputerUseCapable: true, // 即使误传能力位也不得放行 ssh
    }),
    { kind: "remote-ssh", supported: false },
  );
});

test("本机与 Web、Linux 分支不变（回归）", () => {
  assert.deepEqual(resolveComputerUseAvailability({ isDesktop: true, isWindowsDesktop: true }), {
    kind: "local-windows",
    supported: true,
  });
  assert.deepEqual(resolveComputerUseAvailability({ isDesktop: true, isMacDesktop: true }), {
    kind: "local-macos",
    supported: true,
  });
  assert.deepEqual(resolveComputerUseAvailability({ isDesktop: true }), {
    kind: "local-linux",
    supported: false,
  });
  assert.deepEqual(resolveComputerUseAvailability({}), { kind: "web", supported: false });
});

test("远端权限行：成功回包不带 available 字段 → 可见", () => {
  assert.equal(
    isRemoteCuaPermissionRowVisible({
      grantOwner: "user",
      accessibility: "granted",
      screenRecording: "granted",
    }),
    true,
  );
});

test("远端权限行：显式 available:false（对端 win/linux/未启用）→ 隐藏", () => {
  assert.equal(
    isRemoteCuaPermissionRowVisible({
      available: false,
      reason: "CUA permissions are only available on macOS.",
    }),
    false,
  );
});

test("远端权限行：尚未查询（null）→ 隐藏", () => {
  assert.equal(isRemoteCuaPermissionRowVisible(null), false);
});
