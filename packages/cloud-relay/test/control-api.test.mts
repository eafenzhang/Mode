/**
 * Mode Cloud 控制面：会话路径 enroll、设备目录、在线状态、账号隔离、撤销。
 * 使用 :memory: SQLite 与 mock GitHub；不依赖外部服务。
 */
import assert from "node:assert/strict";
import test from "node:test";

import { startRelayServer, type RelayServer } from "../src/server.js";
import {
  startRelayTunnelClient,
  type RelayTunnelClient,
} from "../src/tunnelClient.js";
import {
  enrollWithSession,
  githubSession,
  listDevices,
  startMockGithub,
  withRelay,
} from "./helpers.mts";

test("enroll + 目录：设备、公网地址与离线状态", async () => {
  const github = await startMockGithub({ tokens: { t1: { id: 1, login: "u1" } } });
  try {
    await withRelay({ githubApiBaseUrl: `http://127.0.0.1:${github.port}` }, async (relay) => {
      const { sessionToken } = await githubSession(relay, "t1");
      const enrolled = await enrollWithSession(relay.port, sessionToken, {
        deviceId: "device-1",
        tunnelId: "tunnel-0000000001",
        accessToken: "agent-token-device-0001",
      });
      assert.equal(enrolled.status, 200);
      const deviceToken = String(enrolled.body.deviceToken);
      assert.equal(typeof deviceToken, "string");

      const listed = await listDevices(relay.port, deviceToken);
      assert.equal(listed.status, 200);
      const devices = listed.body.devices as Array<Record<string, unknown>>;
      assert.equal(devices.length, 1);
      assert.equal(devices[0]?.id, "device-1");
      assert.equal(devices[0]?.online, false);
      assert.equal(devices[0]?.relayUrl, "https://relay.example.test/d/tunnel-0000000001");
    });
  } finally {
    await github.close();
  }
});

test("在线状态跟随隧道注册与断开", async () => {
  const github = await startMockGithub({ tokens: { t1: { id: 2, login: "u2" } } });
  try {
    await withRelay({ githubApiBaseUrl: `http://127.0.0.1:${github.port}` }, async (relay) => {
      const { sessionToken } = await githubSession(relay, "t1");
      const enrolled = await enrollWithSession(relay.port, sessionToken, {
        deviceId: "device-2",
        tunnelId: "tunnel-0000000002",
      });
      const deviceToken = String(enrolled.body.deviceToken);

      let tunnel: RelayTunnelClient | null = startRelayTunnelClient({
        relayUrl: `ws://127.0.0.1:${relay.port}/_tunnel`,
        token: String(enrolled.body.relayToken),
        tunnelId: "tunnel-0000000002",
        localBaseUrl: "http://127.0.0.1:1",
        log: () => undefined,
        reconnect: false,
      });
      try {
        const startedAt = Date.now();
        while (!relay.getTunnelIds().includes("tunnel-0000000002")) {
          if (Date.now() - startedAt > 5_000) {
            throw new Error("tunnel registration timeout");
          }
          await new Promise((resolve) => setTimeout(resolve, 20));
        }
        const online = await listDevices(relay.port, deviceToken);
        assert.equal((online.body.devices as Array<Record<string, unknown>>)[0]?.online, true);

        tunnel.close();
        tunnel = null;
        const stoppedAt = Date.now();
        while (relay.getTunnelIds().includes("tunnel-0000000002")) {
          if (Date.now() - stoppedAt > 5_000) {
            throw new Error("tunnel disconnect timeout");
          }
          await new Promise((resolve) => setTimeout(resolve, 20));
        }
        const offline = await listDevices(relay.port, deviceToken);
        assert.equal((offline.body.devices as Array<Record<string, unknown>>)[0]?.online, false);
      } finally {
        tunnel?.close();
      }
    });
  } finally {
    await github.close();
  }
});

test("账号隔离：不同 GitHub 账号的设备互不可见；无 Bearer 401", async () => {
  const github = await startMockGithub({
    tokens: { ta: { id: 10, login: "alice" }, tb: { id: 20, login: "bob" } },
  });
  try {
    await withRelay({ githubApiBaseUrl: `http://127.0.0.1:${github.port}` }, async (relay) => {
      const alice = await githubSession(relay, "ta");
      const bob = await githubSession(relay, "tb");
      const enrolledA = await enrollWithSession(relay.port, alice.sessionToken, {
        deviceId: "device-a",
        tunnelId: "tunnel-000000000a",
      });
      const enrolledB = await enrollWithSession(relay.port, bob.sessionToken, {
        deviceId: "device-b",
        tunnelId: "tunnel-000000000b",
      });
      assert.equal(enrolledA.status, 200);
      assert.equal(enrolledB.status, 200);

      const noToken = await fetch(`http://127.0.0.1:${relay.port}/v1/devices`);
      assert.equal(noToken.status, 401);

      const listA = await listDevices(relay.port, String(enrolledA.body.deviceToken));
      const listB = await listDevices(relay.port, String(enrolledB.body.deviceToken));
      assert.deepEqual(
        (listA.body.devices as Array<Record<string, unknown>>).map((device) => device.id),
        ["device-a"],
      );
      assert.deepEqual(
        (listB.body.devices as Array<Record<string, unknown>>).map((device) => device.id),
        ["device-b"],
      );
    });
  } finally {
    await github.close();
  }
});

test("撤销设备：目录立即移除，令牌失效", async () => {
  const github = await startMockGithub({ tokens: { t1: { id: 3, login: "u3" } } });
  try {
    await withRelay({ githubApiBaseUrl: `http://127.0.0.1:${github.port}` }, async (relay) => {
      const { sessionToken } = await githubSession(relay, "t1");
      const enrolled = await enrollWithSession(relay.port, sessionToken, {
        deviceId: "device-revoke",
        tunnelId: "tunnel-000000000c",
      });
      const deviceToken = String(enrolled.body.deviceToken);

      const revoked = await fetch(
        `http://127.0.0.1:${relay.port}/v1/devices/device-revoke/revoke`,
        { method: "POST", headers: { authorization: `Bearer ${deviceToken}` } },
      );
      assert.equal(revoked.status, 200);
      const after = await listDevices(relay.port, deviceToken);
      assert.equal(after.status, 401);
    });
  } finally {
    await github.close();
  }
});

test("健康检查：控制面启用且数据库可读", async () => {
  const relay: RelayServer = await startRelayServer({
    port: 0,
    token: "relay-token",
    publicBaseUrl: "https://relay.example.test",
    log: () => undefined,
    cloud: { dbPath: ":memory:", accountPepper: "pepper", dbKey: "test-db-key" },
  });
  try {
    const response = await fetch(`http://127.0.0.1:${relay.port}/v1/health`);
    assert.equal(response.status, 200);
    const body = (await response.json()) as Record<string, unknown>;
    assert.equal(body.ok, true);
    assert.equal(body.control, true);
  } finally {
    await relay.close();
  }
});
