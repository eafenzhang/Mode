/** v3 设备目录地址格式：允许 `wss://host:port`，拒绝其它协议与畸形值。 */
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import test from "node:test";

import { generateIdentity, type DeviceDescriptor } from "@mode/remote-core";
import { startRemoteServer, type RemoteServerHandle } from "../src/index.js";

async function withServer(run: (server: RemoteServerHandle) => Promise<void>): Promise<void> {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "address-test-secret-long-enough",
    githubVerifier: async () => ({ providerUserId: "addr", login: "addr" }),
  });
  try {
    await run(server);
  } finally {
    await server.close();
  }
}

async function sessionToken(server: RemoteServerHandle): Promise<string> {
  const response = await fetch(`${server.url}/api/v1/auth/github`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ accessToken: "github" }),
  });
  const body = (await response.json()) as { sessionToken?: string };
  assert.equal(response.status, 200);
  return body.sessionToken!;
}

function host(id: string): DeviceDescriptor {
  return {
    deviceId: id,
    name: "Host",
    role: "host",
    platform: "desktop",
    identityKey: generateIdentity().identityKey,
  };
}

test("设备注册地址接受 wss:// 前缀并拒绝其它协议", async () => {
  await withServer(async (server) => {
    const token = await sessionToken(server);
    const register = (addresses: string[]) =>
      fetch(`${server.url}/api/v1/devices`, {
        method: "POST",
        headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
        body: JSON.stringify({ device: host(randomUUID()), addresses }),
      });

    const accepted = await register(["wss://192.168.1.5:45880", "192.168.1.6:45880"]);
    assert.equal(accepted.status, 201, await accepted.text());

    for (const invalid of ["https://192.168.1.5:45880", "ws://bad host:1", "192.168.1.5"]) {
      const response = await register([invalid]);
      assert.equal(response.status, 422, `${invalid} 必须被拒绝`);
    }
  });
});
