import assert from "node:assert/strict";
import test from "node:test";

// 远程桌面设备页可用性判定（docs/specs/lan-remote-desktop.md 产品规则 1/5 与降级语义；
// 原生媒体面版本门见 docs/specs/lan-remote-desktop-native-media.md）：
// 能力位缺键 = 不支持、探测失败/离线一律置灰、只有「在线 + 两个能力位都为真」才可连接。
// 纯函数单测；shared 的坐标/键盘纯函数不在此重复覆盖（packages/shared/test 已测）。
const {
  resolveRemoteDesktopDeviceAvailability,
  resolveRemoteDesktopDeviceLabel,
  filterUnpairedDiscoveredPeers,
} = await import("../src/remote-desktop/deviceAvailability.js");

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
      capability: { remoteDesktop: true, remoteDesktopNative: true, online: false },
    }),
    { canConnect: false, reason: "offline" },
  );
});

test("旧对端缺键（能力位 false）显示不支持", () => {
  assert.deepEqual(
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: { remoteDesktop: false, remoteDesktopNative: false, online: true },
    }),
    { canConnect: false, reason: "unsupported" },
  );
});

test("对端有 remoteDesktop 但缺 remoteDesktopNative（旧版本）→ 不支持", () => {
  assert.deepEqual(
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: { remoteDesktop: true, remoteDesktopNative: false, online: true },
    }),
    { canConnect: false, reason: "unsupported" },
  );
});

test("在线且能力位为真才可连接", () => {
  assert.deepEqual(
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: { remoteDesktop: true, remoteDesktopNative: true, online: true, name: "DESKTOP-1" },
    }),
    { canConnect: true, reason: null },
  );
});

// 产品规则 9（被控中不可发起）：本机被控状态优先于一切对端探测结果。
test("本机被控中：即使对端就绪也整卡置灰", () => {
  assert.deepEqual(
    resolveRemoteDesktopDeviceAvailability(
      { status: "ok", capability: { remoteDesktop: true, remoteDesktopNative: true, online: true } },
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

// UI 规则「设备页」：检测列表只列未配对设备；全部被过滤时 hiddenCount 供空态提示。
test("检测列表过滤已配对设备：保留未配对、统计隐藏数", () => {
  const peers = [
    { serverId: "srv-a", host: "192.168.1.2", port: 45880 },
    { serverId: "srv-b", host: "192.168.1.3", port: 45880 },
    { serverId: "srv-c", host: "192.168.1.4", port: 45880 },
  ];
  assert.deepEqual(filterUnpairedDiscoveredPeers(peers, ["srv-a"]), {
    visible: [peers[1], peers[2]],
    hiddenCount: 1,
  });
  // 无配对：原样返回，hiddenCount=0。
  assert.deepEqual(filterUnpairedDiscoveredPeers(peers, []), { visible: peers, hiddenCount: 0 });
  // 全部已配对：visible 空、hiddenCount = 总数（调用方给「均已配对」提示）。
  assert.deepEqual(filterUnpairedDiscoveredPeers(peers, ["srv-a", "srv-b", "srv-c"]), {
    visible: [],
    hiddenCount: 3,
  });
  // 配对列表含未出现的 serverId：不影响结果。
  assert.deepEqual(filterUnpairedDiscoveredPeers(peers, ["srv-x"]), {
    visible: peers,
    hiddenCount: 0,
  });
});
