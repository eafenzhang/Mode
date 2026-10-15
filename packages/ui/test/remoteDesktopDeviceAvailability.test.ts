import assert from "node:assert/strict";
import test from "node:test";

// 远程桌面设备页可用性判定（docs/specs/remote-desktop-system-rdp.md 产品规则 7/8 与失败语义表）：
// 判据优先级 = 探测中/探测失败 → 离线 → 未共享（提示性共享位 false）→ 家庭版 → 未开启系统 RDP → 可连接；
// systemRdp 缺键 = 状态未知（不置灰，失败交给 mstsc 自身）；
// 共享位参与置灰 = 尊重对端机主的共享意愿（否则被控端开关在 UI 上等于不存在）。
// 纯函数单测。
const {
  resolveRemoteDesktopDeviceAvailability,
  resolveRemoteDesktopDeviceLabel,
  resolveRemoteDesktopDeviceStatusPill,
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

test("对端离线：即使系统 RDP 就绪也不可连接", () => {
  assert.deepEqual(
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: {
        remoteDesktop: true,
        online: false,
        systemRdp: { enabled: true, editionOk: true },
      },
    }),
    { canConnect: false, reason: "offline" },
  );
});

test("对端为家庭版：置灰并优先于「未开启」", () => {
  assert.deepEqual(
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: {
        remoteDesktop: true,
        online: true,
        systemRdp: { enabled: true, editionOk: false },
      },
    }),
    { canConnect: false, reason: "system-rdp-home" },
  );
  // 家庭版且未开启：根因是家庭版（用户想开也开不了），故 home 优先。
  assert.deepEqual(
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: {
        remoteDesktop: true,
        online: true,
        systemRdp: { enabled: false, editionOk: false },
      },
    }),
    { canConnect: false, reason: "system-rdp-home" },
  );
});

test("对端未开启系统远程桌面：置灰", () => {
  assert.deepEqual(
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: {
        remoteDesktop: true,
        online: true,
        systemRdp: { enabled: false, editionOk: true },
      },
    }),
    { canConnect: false, reason: "system-rdp-disabled" },
  );
});

// spec 失败语义表：「systemRdp 缺键（旧对端）→ 不置灰，按状态未知呈现」。
test("systemRdp 缺键 = 状态未知，不置灰（仍允许发起）", () => {
  assert.deepEqual(
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: { remoteDesktop: true, online: true },
    }),
    { canConnect: true, reason: null },
  );
});

test("在线且系统 RDP 就绪才可连接", () => {
  assert.deepEqual(
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: {
        remoteDesktop: true,
        online: true,
        systemRdp: { enabled: true, editionOk: true },
        name: "DESKTOP-1",
      },
    }),
    { canConnect: true, reason: null },
  );
});

// 提示性共享位：它是**对端机主的共享意愿声明**，参与置灰；否则被控端开关在 UI 上等于不存在。
test("对端未共享为可远控设备：置灰（先于系统能力检查）", () => {
  assert.deepEqual(
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: {
        remoteDesktop: false,
        online: true,
        systemRdp: { enabled: true, editionOk: true },
      },
    }),
    { canConnect: false, reason: "not-shared" },
  );
  // 未共享且家庭版：先暴露「未共享」（对端的意愿），开启后再自然暴露家庭版根因。
  assert.deepEqual(
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: {
        remoteDesktop: false,
        online: true,
        systemRdp: { enabled: false, editionOk: false },
      },
    }),
    { canConnect: false, reason: "not-shared" },
  );
});

test("设备展示名优先自报名，其次回退 serverId", () => {
  assert.equal(resolveRemoteDesktopDeviceLabel({ serverId: "srv-1", name: " 工作站 " }), "工作站");
  assert.equal(resolveRemoteDesktopDeviceLabel({ serverId: "srv-1", name: "   " }), "srv-1");
  assert.equal(resolveRemoteDesktopDeviceLabel({ serverId: "srv-1" }), "srv-1");
});

// 检测列表只列未配对设备；全部被过滤时 hiddenCount 供空态提示。
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
  assert.deepEqual(filterUnpairedDiscoveredPeers(peers, []), { visible: peers, hiddenCount: 0 });
  assert.deepEqual(filterUnpairedDiscoveredPeers(peers, ["srv-a", "srv-b", "srv-c"]), {
    visible: [],
    hiddenCount: 3,
  });
  assert.deepEqual(filterUnpairedDiscoveredPeers(peers, ["srv-x"]), {
    visible: peers,
    hiddenCount: 0,
  });
});

// 状态胶囊与连接入口同源（都投影自同一份探测结果）：胶囊只表达状态。
test("状态胶囊：探测中 / 状态未知用弱化档（不冒充离线）", () => {
  assert.deepEqual(resolveRemoteDesktopDeviceStatusPill(undefined), {
    labelId: "remoteDesktop.device.status.probing",
    tone: "muted",
  });
  assert.deepEqual(resolveRemoteDesktopDeviceStatusPill({ status: "probing" }), {
    labelId: "remoteDesktop.device.status.probing",
    tone: "muted",
  });
  assert.deepEqual(resolveRemoteDesktopDeviceStatusPill({ status: "probe-failed" }), {
    labelId: "remoteDesktop.device.status.unknown",
    tone: "muted",
  });
});

test("状态胶囊：离线 / 家庭版 / 未开启用提醒档", () => {
  assert.deepEqual(
    resolveRemoteDesktopDeviceStatusPill({
      status: "ok",
      capability: { remoteDesktop: true, online: false },
    }),
    { labelId: "remoteDesktop.device.status.offline", tone: "attention" },
  );
  assert.deepEqual(
    resolveRemoteDesktopDeviceStatusPill({
      status: "ok",
      capability: { remoteDesktop: true, online: true, systemRdp: { enabled: true, editionOk: false } },
    }),
    { labelId: "remoteDesktop.device.status.homeEdition", tone: "attention" },
  );
  assert.deepEqual(
    resolveRemoteDesktopDeviceStatusPill({
      status: "ok",
      capability: { remoteDesktop: true, online: true, systemRdp: { enabled: false, editionOk: true } },
    }),
    { labelId: "remoteDesktop.device.status.rdpDisabled", tone: "attention" },
  );
});

test("状态胶囊：就绪档与可连接判定同源", () => {
  const probes = [
    undefined,
    { status: "probing" },
    { status: "probe-failed" },
    {
      status: "ok",
      capability: { remoteDesktop: true, online: false, systemRdp: { enabled: true, editionOk: true } },
    },
    {
      status: "ok",
      capability: { remoteDesktop: true, online: true, systemRdp: { enabled: true, editionOk: false } },
    },
    {
      status: "ok",
      capability: { remoteDesktop: true, online: true, systemRdp: { enabled: false, editionOk: true } },
    },
    {
      status: "ok",
      capability: { remoteDesktop: false, online: true, systemRdp: { enabled: true, editionOk: true } },
    },
    { status: "ok", capability: { remoteDesktop: true, online: true } },
  ];
  for (const probe of probes) {
    const ready = resolveRemoteDesktopDeviceStatusPill(probe).tone === "ready";
    const { canConnect, reason } = resolveRemoteDesktopDeviceAvailability(probe);
    assert.equal(ready, canConnect, `探测状态 ${JSON.stringify(probe)} 的胶囊与入口不一致`);
    if (!canConnect) {
      assert.notEqual(reason, null);
    }
  }
});

// 置灰时必定有原因文案 key（设备卡直接用它取 i18n）。
test("置灰原因都能映射到稳定文案 key", () => {
  const reasons = [
    resolveRemoteDesktopDeviceAvailability(undefined).reason,
    resolveRemoteDesktopDeviceAvailability({ status: "probe-failed" }).reason,
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: { remoteDesktop: true, online: false },
    }).reason,
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: { remoteDesktop: false, online: true },
    }).reason,
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: { remoteDesktop: true, online: true, systemRdp: { enabled: true, editionOk: false } },
    }).reason,
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: { remoteDesktop: true, online: true, systemRdp: { enabled: false, editionOk: true } },
    }).reason,
  ];
  assert.deepEqual(reasons, [
    "probing",
    "probe-failed",
    "offline",
    "not-shared",
    "system-rdp-home",
    "system-rdp-disabled",
  ]);
});
