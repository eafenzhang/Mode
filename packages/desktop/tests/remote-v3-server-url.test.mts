import assert from "node:assert/strict";
import test from "node:test";

import { normalizeRemoteServerUrl } from "../src/host/remoteV3/api.js";

test("v3 服务地址保留网关/反代路径前缀", () => {
  assert.equal(
    normalizeRemoteServerUrl("https://nas.example/app/mode/"),
    "https://nas.example/app/mode",
  );
  assert.equal(normalizeRemoteServerUrl("https://remote.example.com/"), "https://remote.example.com");
  assert.throws(() => normalizeRemoteServerUrl("http://nas.example/app/mode"), /HTTPS/u);
});

test("LAN 直连地址按 scheme 选择 ws/wss", async () => {
  const { buildDirectSocketUrl } = await import("../src/host/remoteV3/wsSocket.js");
  assert.equal(
    buildDirectSocketUrl("192.168.1.5:45880"),
    "ws://192.168.1.5:45880/ws/v1/direct",
  );
  assert.equal(
    buildDirectSocketUrl("wss://192.168.1.5:45880/"),
    "wss://192.168.1.5:45880/ws/v1/direct",
  );
  assert.equal(
    buildDirectSocketUrl("ws://192.168.1.5:45880"),
    "ws://192.168.1.5:45880/ws/v1/direct",
  );
});
