import assert from "node:assert/strict";
import test from "node:test";

import { generateIdentity, type DeviceDescriptor } from "@mode/remote-core";

import { startRemoteServer, type RemoteServerHandle } from "../src/index.js";

const HOST_ID = "019a0000-0000-7000-8000-000000000001";
const CLIENT_ID = "019a0000-0000-7000-8000-000000000002";
const BOB_HOST_ID = "019a0000-0000-7000-8000-000000000003";
const BOB_CLIENT_ID = "019a0000-0000-7000-8000-000000000004";

async function withServer(run: (server: RemoteServerHandle) => Promise<void>): Promise<void> {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "test-secret-that-is-long-enough",
    githubVerifier: async (token) => {
      if (token === "github-alice") return { providerUserId: "1", login: "alice" };
      if (token === "github-bob") return { providerUserId: "2", login: "bob" };
      throw new Error("invalid github token");
    },
  });
  try {
    await run(server);
  } finally {
    await server.close();
  }
}

interface CallOptions {
  method?: string;
  token?: string;
  body?: unknown;
}

async function call(
  server: RemoteServerHandle,
  path: string,
  options: CallOptions = {},
): Promise<{ status: number; body: any }> {
  const headers: Record<string, string> = { "content-type": "application/json" };
  if (options.token) headers.authorization = `Bearer ${options.token}`;
  const response = await fetch(`${server.url}${path}`, {
    method: options.method ?? "GET",
    headers,
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
  });
  return { status: response.status, body: await response.json() };
}

async function login(server: RemoteServerHandle, token: string): Promise<string> {
  const result = await call(server, "/api/v1/auth/github", {
    method: "POST",
    body: { accessToken: token },
  });
  assert.equal(result.status, 200);
  return result.body.sessionToken as string;
}

async function register(
  server: RemoteServerHandle,
  sessionToken: string,
  descriptor: DeviceDescriptor,
): Promise<{ accessToken: string; refreshToken: string }> {
  const result = await call(server, "/api/v1/devices", {
    method: "POST",
    token: sessionToken,
    body: { device: descriptor, addresses: ["192.168.1.10:45880"] },
  });
  assert.equal(result.status, 201, JSON.stringify(result.body));
  assert.equal(result.body.device.identityKey, descriptor.identityKey);
  return {
    accessToken: result.body.accessToken as string,
    refreshToken: result.body.refreshToken as string,
  };
}

function descriptor(id: string, role: "host" | "client", name: string): DeviceDescriptor {
  return {
    deviceId: id,
    name,
    role,
    platform: "desktop",
    identityKey: generateIdentity().identityKey,
    clientVersion: "0.0.1",
  };
}

test("设备注册、目录与 membership 按账号隔离，目录不泄露 token", async () => {
  await withServer(async (server) => {
    const alice = await login(server, "github-alice");
    const bob = await login(server, "github-bob");
    const host = descriptor(HOST_ID, "host", "Alice Host");
    const client = descriptor(CLIENT_ID, "client", "Alice Client");
    await register(server, alice, host);
    const clientCredentials = await register(server, alice, client);
    await register(server, bob, descriptor(BOB_HOST_ID, "host", "Bob Host"));

    const directory = await call(server, "/api/v1/devices", {
      token: clientCredentials.accessToken,
    });
    assert.equal(directory.status, 200);
    assert.equal(directory.body.items.length, 1);
    assert.equal(directory.body.items[0].device.deviceId, HOST_ID);
    assert.equal(directory.body.items[0].device.role, "host");
    assert.equal("accessToken" in directory.body.items[0], false);
    assert.equal("refreshToken" in directory.body, false);

    const crossAccount = await call(server, "/api/v1/connections", {
      method: "POST",
      token: clientCredentials.accessToken,
      body: { hostDeviceId: BOB_HOST_ID },
    });
    assert.equal(crossAccount.status, 403);
    assert.equal(crossAccount.body.error.code, "cross_account");

    const membership = await call(server, "/api/v1/memberships", {
      token: clientCredentials.accessToken,
    });
    assert.equal(membership.status, 200);
    assert.equal(membership.body.items.length, 1);
    assert.equal(membership.body.items[0].peer.deviceId, HOST_ID);
  });
});

test("refresh 轮换成功，旧 refresh 重放会撤销设备", async () => {
  await withServer(async (server) => {
    const alice = await login(server, "github-alice");
    const host = descriptor(HOST_ID, "host", "Alice Host");
    const client = descriptor(CLIENT_ID, "client", "Alice Client");
    await register(server, alice, host);
    const credentials = await register(server, alice, client);

    const refreshed = await call(server, "/api/v1/devices/refresh", {
      method: "POST",
      body: { deviceId: CLIENT_ID, refreshToken: credentials.refreshToken },
    });
    assert.equal(refreshed.status, 200, JSON.stringify(refreshed.body));
    const nextAccess = refreshed.body.accessToken as string;
    const nextRefresh = refreshed.body.refreshToken as string;
    assert.notEqual(nextAccess, credentials.accessToken);

    const directory = await call(server, "/api/v1/devices", { token: nextAccess });
    assert.equal(directory.status, 200);

    const replay = await call(server, "/api/v1/devices/refresh", {
      method: "POST",
      body: { deviceId: CLIENT_ID, refreshToken: credentials.refreshToken },
    });
    assert.equal(replay.status, 401);
    assert.equal(replay.body.error.code, "refresh_reused");

    const revoked = await call(server, "/api/v1/devices", { token: nextAccess });
    assert.equal(revoked.status, 401);

    const refreshAgain = await call(server, "/api/v1/devices/refresh", {
      method: "POST",
      body: { deviceId: CLIENT_ID, refreshToken: nextRefresh },
    });
    assert.equal(refreshAgain.status, 401);
  });
});

test("owner 撤销设备后设备凭证与授权关系全部失效", async () => {
  await withServer(async (server) => {
    const alice = await login(server, "github-alice");
    const host = descriptor(HOST_ID, "host", "Alice Host");
    const client = descriptor(CLIENT_ID, "client", "Alice Client");
    await register(server, alice, host);
    const credentials = await register(server, alice, client);

    const revoke = await call(server, `/api/v1/devices/${CLIENT_ID}`, {
      method: "DELETE",
      token: alice,
    });
    assert.equal(revoke.status, 200);

    const directory = await call(server, "/api/v1/devices", {
      token: credentials.accessToken,
    });
    assert.equal(directory.status, 401);

    const revokedAgain = await call(server, "/api/v1/devices", {
      method: "POST",
      token: alice,
      body: { device: { ...client, identityKey: generateIdentity().identityKey } },
    });
    assert.equal(revokedAgain.status, 409);
    assert.equal(revokedAgain.body.error.code, "device_conflict");
  });
});
