import assert from "node:assert/strict";
import test from "node:test";

import type { LanPairedPeer, RemoteV3DeviceSummary } from "@mode/shared";
import { lanPeerToRemoteTarget, sortV3Hosts } from "../src/remote-desktop/remoteDeviceModel.js";

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

function host(deviceId: string, name: string, online: boolean): RemoteV3DeviceSummary {
  return {
    deviceId,
    name,
    role: "host",
    platform: "desktop",
    identityKey: `identity-${deviceId}`,
    online,
    addresses: [],
  };
}

test("账号目录投影：在线优先，其次按名称，最后按 deviceId 稳定排序", () => {
  const sorted = sortV3Hosts([
    host("b", "Beta", false),
    host("a", "Alpha", true),
    host("d", "Alpha", false),
    host("c", "Gamma", true),
  ]);
  assert.deepEqual(
    sorted.map((item) => item.deviceId),
    ["a", "c", "d", "b"],
    "在线 Host 前置，同名按 deviceId 兜底",
  );
});

test("账号目录投影不修改入参", () => {
  const input = [host("b", "Beta", false), host("a", "Alpha", true)];
  sortV3Hosts(input);
  assert.deepEqual(
    input.map((item) => item.deviceId),
    ["b", "a"],
    "排序必须是纯函数，页面轮询会复用同一份快照",
  );
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
  assert.deepEqual(target?.endpoints, [
    { scheme: "http", host: "10.0.0.2", port: 45880, kind: "lan" },
  ]);
  assert.equal(target?.host, "10.0.0.2");
});
