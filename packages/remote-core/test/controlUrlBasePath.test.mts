import assert from "node:assert/strict";
import test from "node:test";

import { toControlUrl as nodeControlUrl } from "../src/app/controlUrl.js";
import { toControlUrl as webControlUrl } from "../src/web/transport.js";

test("Control URL 保留网关/反代路径前缀，只追加端点", () => {
  for (const build of [nodeControlUrl, webControlUrl]) {
    assert.equal(
      build("https://nas.example/app/mode"),
      "wss://nas.example/app/mode/ws/v1/connect",
    );
    assert.equal(
      build("https://nas.example/app/mode/"),
      "wss://nas.example/app/mode/ws/v1/connect",
    );
    assert.equal(
      build("https://remote.example.com/?x=1#h"),
      "wss://remote.example.com/ws/v1/connect",
      "query/hash 必须丢弃",
    );
    assert.equal(build("http://127.0.0.1:45890"), "ws://127.0.0.1:45890/ws/v1/connect");
  }
});

test("相对 Relay 端点按客户端服务地址解析", async () => {
  const { resolveRelayEndpoint } = await import("../src/domain/relayEndpoint.js");
  assert.equal(
    resolveRelayEndpoint("/app/mode/ws/v1/relay", "https://nas.example"),
    "wss://nas.example/app/mode/ws/v1/relay",
  );
  assert.equal(
    resolveRelayEndpoint("/app/mode/ws/v1/relay", "https://nas.example/app/mode"),
    "wss://nas.example/app/mode/ws/v1/relay",
  );
  assert.equal(
    resolveRelayEndpoint("wss://relay.example/ws/v1/relay", "https://nas.example"),
    "wss://relay.example/ws/v1/relay",
    "绝对地址原样返回",
  );
});
