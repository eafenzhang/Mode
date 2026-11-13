import assert from "node:assert/strict";
import test from "node:test";

import type { RemoteV3DeviceSummary } from "@mode/shared";
import { sortV3Hosts } from "../src/remote-desktop/remoteDeviceModel.js";

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
