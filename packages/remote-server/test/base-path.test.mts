import assert from "node:assert/strict";
import test from "node:test";

import { startRemoteServer } from "../src/index.js";

test("publicUrl 的路径前缀进入 Relay 端点（fnOS 网关/反代）", async () => {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "base-path-secret-long-enough",
    publicUrl: "https://nas.example/app/mode",
    githubVerifier: async () => ({ providerUserId: "base", login: "base" }),
  });
  try {
    assert.equal(server.relayEndpoint, "wss://nas.example/app/mode/ws/v1/relay");
  } finally {
    await server.close();
  }
});

test("publicUrl 为子路径时下发相对 Relay 端点", async () => {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "base-path-secret-long-enough",
    publicUrl: "/app/mode",
    githubVerifier: async () => ({ providerUserId: "base", login: "base" }),
  });
  try {
    assert.equal(server.relayEndpoint, "/app/mode/ws/v1/relay");
  } finally {
    await server.close();
  }
});
