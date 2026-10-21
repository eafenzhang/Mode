import assert from "node:assert/strict";
import test from "node:test";

import type { LanDiscoveredPeer, LanPairedPeer } from "@mode/shared";
import {
  buildRemoteDevices,
  lanPeerToRemoteTarget,
} from "../src/remote-desktop/remoteDeviceModel.js";
import type { ModeAgentServerRecord } from "../src/remote-desktop/modeAgentPairing.js";

const lanPeer: LanPairedPeer = {
  serverId: "server-1",
  host: "192.168.1.20",
  port: 45880,
  name: "书房电脑",
  endpoints: [
    { scheme: "http", host: "192.168.1.20", port: 45880, kind: "lan" },
    { scheme: "https", host: "relay.example.com", port: 443, kind: "public", path: "/d/t1" },
  ],
};

const discovered: LanDiscoveredPeer = {
  magic: "mode-lan",
  protocolVersion: 1,
  serverId: "server-2",
  name: "客厅电脑",
  host: "192.168.1.21",
  port: 45880,
  version: "1.0.0",
  platform: "win32",
  requiresPairing: true,
};

const modeAgent = {
  serverName: "mode-agent-abc",
  agentId: "abc",
  name: "XP 工控机",
  host: "192.168.1.30",
  port: 47821,
  server: {},
} as unknown as ModeAgentServerRecord;

test("统一设备投影：LAN 同时具备 workspace/desktop，Mode Agent 只有 conversation", () => {
  const devices = buildRemoteDevices({
    lanPeers: [lanPeer],
    probes: {
      "server-1": {
        status: "ok",
        capability: {
          online: true,
          remoteDesktop: true,
          remoteDesktopNative: true,
        },
      },
    },
    modeAgents: [modeAgent],
    discovered: [discovered],
  });
  assert.equal(devices.length, 3);
  const lan = devices.find((device) => device.origin === "lan");
  assert.deepEqual(lan?.channels, { workspace: true, desktop: true, conversation: false });
  assert.deepEqual(
    lan?.endpoints.map((endpoint) => endpoint.kind),
    ["public", "lan"],
    "设备端点必须公网优先",
  );
  const agent = devices.find((device) => device.origin === "mode-agent");
  assert.deepEqual(agent?.channels, { workspace: false, desktop: false, conversation: true });
  const found = devices.find((device) => device.origin === "discovered");
  assert.deepEqual(found?.channels, { workspace: false, desktop: false, conversation: false });
});

test("LAN 设备转远程工作区目标：保留端点列表，主端点取公网", () => {
  const target = lanPeerToRemoteTarget(lanPeer);
  assert.equal(target?.kind, "lan");
  assert.equal(target?.host, "192.168.1.20", "identity 仍取配对记录的主地址投影");
  assert.equal(target?.endpoints?.length, 2);
  assert.equal(target?.endpoints?.[0]?.kind, "public");
  assert.equal(target?.endpoints?.[0]?.path, "/d/t1");
});

test("旧配对记录没有 endpoints 时仍可构造 LAN 目标", () => {
  const target = lanPeerToRemoteTarget({ serverId: "legacy", host: "10.0.0.2", port: 45880 });
  assert.deepEqual(target?.endpoints, [{ scheme: "http", host: "10.0.0.2", port: 45880, kind: "lan" }]);
  assert.equal(target?.host, "10.0.0.2");
});
