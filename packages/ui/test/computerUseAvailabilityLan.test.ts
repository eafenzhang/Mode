import assert from "node:assert/strict";
import test from "node:test";

const { resolveComputerUseAvailability } =
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
