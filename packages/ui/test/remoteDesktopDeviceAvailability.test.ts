import assert from "node:assert/strict";
import test from "node:test";

// 远程桌面设备页可用性判定（docs/specs/lan-remote-desktop.md 产品规则 1/5 与降级语义）：
// 能力位缺键 = 不支持、探测失败/离线一律置灰、只有「在线 + 能力位为真」才可连接。
// 纯函数单测；shared 的坐标/键盘纯函数不在此重复覆盖（packages/shared/test 已测）。
const { resolveRemoteDesktopDeviceAvailability, resolveRemoteDesktopDeviceLabel } =
  await import("../src/remote-desktop/deviceAvailability.js");

test("探测未开始时按「检测中」置灰", () => {
  assert.deepEqual(resolveRemoteDesktopDeviceAvailability(undefined), {
    canConnect: false,
    reason: "probing",
  });
  assert.deepEqual(resolveRemoteDesktopDeviceAvailability({ status: "probing" }), {
    canConnect: false,
    reason: "probing",
  });
});

test("探测失败按置灰处理（能力位不猜测）", () => {
  assert.deepEqual(resolveRemoteDesktopDeviceAvailability({ status: "probe-failed" }), {
    canConnect: false,
    reason: "probe-failed",
  });
});

test("对端离线：即使能力位为真也不可连接", () => {
  assert.deepEqual(
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: { remoteDesktop: true, online: false },
    }),
    { canConnect: false, reason: "offline" },
  );
});

test("旧对端缺键（能力位 false）显示不支持", () => {
  assert.deepEqual(
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: { remoteDesktop: false, online: true },
    }),
    { canConnect: false, reason: "unsupported" },
  );
});

test("在线且能力位为真才可连接", () => {
  assert.deepEqual(
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: { remoteDesktop: true, online: true, name: "DESKTOP-1" },
    }),
    { canConnect: true, reason: null },
  );
});

// 产品规则 9（被控中不可发起）：本机被控状态优先于一切对端探测结果。
test("本机被控中：即使对端就绪也整卡置灰", () => {
  assert.deepEqual(
    resolveRemoteDesktopDeviceAvailability(
      { status: "ok", capability: { remoteDesktop: true, online: true } },
      { locallyControlled: true },
    ),
    { canConnect: false, reason: "locally-controlled" },
  );
  // 对端还在探测时，被控原因也优先展示（更可操作）。
  assert.deepEqual(
    resolveRemoteDesktopDeviceAvailability({ status: "probing" }, { locallyControlled: true }),
    { canConnect: false, reason: "locally-controlled" },
  );
});

test("设备展示名优先自报名，其次回退 serverId", () => {
  assert.equal(resolveRemoteDesktopDeviceLabel({ serverId: "srv-1", name: " 工作站 " }), "工作站");
  assert.equal(resolveRemoteDesktopDeviceLabel({ serverId: "srv-1", name: "   " }), "srv-1");
  assert.equal(resolveRemoteDesktopDeviceLabel({ serverId: "srv-1" }), "srv-1");
});
