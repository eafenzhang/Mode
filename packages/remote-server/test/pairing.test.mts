/**
 * 跨账号配对 v1（docs/specs/remote-control-v3-pairing.md）：
 * ticket → claim → 8 位码 → Host 确认 → 双端换凭证。
 */
import assert from "node:assert/strict";
import test from "node:test";

import { generateIdentity, type DeviceDescriptor } from "@mode/remote-core";

import { startRemoteServer, type RemoteServerHandle } from "../src/index.js";

const HOST_ID = "019a0000-0000-7000-8000-0000000000b1";
const CLIENT_ID = "019a0000-0000-7000-8000-0000000000b2";
const OTHER_ID = "019a0000-0000-7000-8000-0000000000b3";

async function withServer(
  run: (server: RemoteServerHandle) => Promise<void>,
  pairingTtlMs?: number,
): Promise<void> {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "pairing-test-secret-long-enough",
    ...(pairingTtlMs ? { pairingTtlMs } : {}),
    githubVerifier: async (token) => {
      if (token === "github-alice") return { providerUserId: "alice", login: "alice" };
      if (token === "github-bob") return { providerUserId: "bob", login: "bob" };
      throw new Error("invalid github token");
    },
  });
  try {
    await run(server);
  } finally {
    await server.close();
  }
}

async function call(
  server: RemoteServerHandle,
  path: string,
  options: { method?: string; token?: string; body?: unknown } = {},
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

function descriptor(id: string, role: "host" | "client", name: string): DeviceDescriptor {
  return {
    deviceId: id,
    name,
    role,
    platform: "desktop",
    identityKey: generateIdentity().identityKey,
  };
}

async function register(
  server: RemoteServerHandle,
  github: string,
  device: DeviceDescriptor,
): Promise<{ accessToken: string; refreshToken: string; sessionToken: string }> {
  const login = await call(server, "/api/v1/auth/github", {
    method: "POST",
    body: { accessToken: github },
  });
  assert.equal(login.status, 200, JSON.stringify(login.body));
  const result = await call(server, "/api/v1/devices", {
    method: "POST",
    token: login.body.sessionToken as string,
    body: { device },
  });
  assert.equal(result.status, 201, JSON.stringify(result.body));
  return {
    accessToken: result.body.accessToken as string,
    refreshToken: result.body.refreshToken as string,
    sessionToken: login.body.sessionToken as string,
  };
}

async function createClaimConfirm(
  server: RemoteServerHandle,
  hostToken: string,
  clientToken: string,
): Promise<{ pairingId: string; code: string; hostToken: string }> {
  const created = await call(server, "/api/v1/pairing/sessions", {
    method: "POST",
    token: hostToken,
  });
  assert.equal(created.status, 201, JSON.stringify(created.body));
  const claimed = await call(server, "/api/v1/pairing/sessions/claim", {
    method: "POST",
    token: clientToken,
    body: { ticket: created.body.ticket },
  });
  assert.equal(claimed.status, 200, JSON.stringify(claimed.body));
  assert.match(claimed.body.verificationCode, /^\d{8}$/u);
  const confirmed = await call(
    server,
    `/api/v1/pairing/sessions/${created.body.pairingId}/confirm`,
    { method: "POST", token: hostToken, body: { code: claimed.body.verificationCode } },
  );
  assert.equal(confirmed.status, 200, JSON.stringify(confirmed.body));
  return {
    pairingId: created.body.pairingId as string,
    code: claimed.body.verificationCode as string,
    hostToken: confirmed.body.accessToken as string,
  };
}

test("配对成功：host 换凭证、client redeem 一次、membership 建立", async () => {
  await withServer(async (server) => {
    const host = await register(server, "github-alice", descriptor(HOST_ID, "host", "Alice Host"));
    const client = await register(server, "github-bob", descriptor(CLIENT_ID, "client", "Bob Web"));

    const created = await call(server, "/api/v1/pairing/sessions", {
      method: "POST",
      token: host.accessToken,
    });
    assert.equal(created.status, 201, JSON.stringify(created.body));
    assert.ok(created.body.ticket);

    const claimed = await call(server, "/api/v1/pairing/sessions/claim", {
      method: "POST",
      token: client.accessToken,
      body: { ticket: created.body.ticket },
    });
    assert.equal(claimed.status, 200, JSON.stringify(claimed.body));
    assert.equal(claimed.body.hostName, "Alice Host");

    const confirmed = await call(
      server,
      `/api/v1/pairing/sessions/${created.body.pairingId}/confirm`,
      { method: "POST", token: host.accessToken, body: { code: claimed.body.verificationCode } },
    );
    assert.equal(confirmed.status, 200, JSON.stringify(confirmed.body));
    assert.equal(confirmed.body.status, "confirmed");
    const newHostToken = confirmed.body.accessToken as string;

    const oldHost = await call(server, "/api/v1/auth/me", { token: host.accessToken });
    assert.equal(oldHost.status, 401, "Host 旧凭证必须失效");
    const newHost = await call(server, "/api/v1/auth/me", { token: newHostToken });
    assert.equal(newHost.status, 200);
    assert.equal(newHost.body.kind, "device");

    const status = await call(server, `/api/v1/pairing/sessions/${created.body.pairingId}`, {
      token: newHostToken,
    });
    assert.equal(status.body.status, "confirmed");

    const redeemed = await call(
      server,
      `/api/v1/pairing/sessions/${created.body.pairingId}/redeem`,
      { method: "POST", token: client.accessToken },
    );
    assert.equal(redeemed.status, 200, JSON.stringify(redeemed.body));
    const second = await call(
      server,
      `/api/v1/pairing/sessions/${created.body.pairingId}/redeem`,
      { method: "POST", token: client.accessToken },
    );
    assert.equal(second.status, 401, "旧 client token 已轮换");

    const memberships = await call(server, "/api/v1/memberships", {
      token: redeemed.body.accessToken,
    });
    assert.equal(memberships.status, 200);
    assert.equal(memberships.body.items.length, 1);
    assert.equal(memberships.body.items[0].peer.deviceId, HOST_ID);
  });
});

test("重复 claim、重复 confirm 与错误码次数限制", async () => {
  await withServer(async (server) => {
    const host = await register(server, "github-alice", descriptor(HOST_ID, "host", "Host"));
    const client = await register(server, "github-bob", descriptor(CLIENT_ID, "client", "Client"));
    const created = await call(server, "/api/v1/pairing/sessions", {
      method: "POST",
      token: host.accessToken,
    });
    const first = await call(server, "/api/v1/pairing/sessions/claim", {
      method: "POST",
      token: client.accessToken,
      body: { ticket: created.body.ticket },
    });
    assert.equal(first.status, 200);
    const second = await call(server, "/api/v1/pairing/sessions/claim", {
      method: "POST",
      token: client.accessToken,
      body: { ticket: created.body.ticket },
    });
    assert.equal(second.status, 409);
    assert.equal(second.body.error.code, "pairing_already_used");

    let last: { status: number; body: any } | null = null;
    for (let attempt = 0; attempt < 5; attempt += 1) {
      last = await call(server, `/api/v1/pairing/sessions/${created.body.pairingId}/confirm`, {
        method: "POST",
        token: host.accessToken,
        body: { code: "00000000" },
      });
    }
    assert.equal(last?.status, 429, JSON.stringify(last?.body));
    assert.equal(last?.body.error.code, "pairing_attempts_exhausted");
    const after = await call(
      server,
      `/api/v1/pairing/sessions/${created.body.pairingId}/confirm`,
      { method: "POST", token: host.accessToken, body: { code: first.body.verificationCode } },
    );
    assert.equal(after.status, 409);
  });
});

test("同账号、角色错误与非参与方访问被拒绝", async () => {
  await withServer(async (server) => {
    const host = await register(server, "github-alice", descriptor(HOST_ID, "host", "Host"));
    const sameAccountClient = await register(
      server,
      "github-alice",
      descriptor(OTHER_ID, "client", "Same Account"),
    );
    const created = await call(server, "/api/v1/pairing/sessions", {
      method: "POST",
      token: host.accessToken,
    });
    const same = await call(server, "/api/v1/pairing/sessions/claim", {
      method: "POST",
      token: sameAccountClient.accessToken,
      body: { ticket: created.body.ticket },
    });
    assert.equal(same.status, 409);
    assert.equal(same.body.error.code, "same_account");

    const roleError = await call(server, "/api/v1/pairing/sessions", {
      method: "POST",
      token: sameAccountClient.accessToken,
    });
    assert.equal(roleError.status, 400);
    assert.equal(roleError.body.error.code, "pairing_role_invalid");

    const otherClient = await register(server, "github-bob", descriptor(CLIENT_ID, "client", "C"));
    const visibility = await call(server, `/api/v1/pairing/sessions/${created.body.pairingId}`, {
      token: otherClient.accessToken,
    });
    assert.equal(visibility.status, 403);
  });
});

test("过期会话领取与取消后确认都失败", async () => {
  await withServer(async (server) => {
    const host = await register(server, "github-alice", descriptor(HOST_ID, "host", "Host"));
    const client = await register(server, "github-bob", descriptor(CLIENT_ID, "client", "Client"));
    const created = await call(server, "/api/v1/pairing/sessions", {
      method: "POST",
      token: host.accessToken,
    });
    await new Promise((resolve) => setTimeout(resolve, 30));
    const expired = await call(server, "/api/v1/pairing/sessions/claim", {
      method: "POST",
      token: client.accessToken,
      body: { ticket: created.body.ticket },
    });
    assert.equal(expired.status, 410);
    assert.equal(expired.body.error.code, "pairing_expired");
  }, 10);

  await withServer(async (server) => {
    const host = await register(server, "github-alice", descriptor(HOST_ID, "host", "Host"));
    const client = await register(server, "github-bob", descriptor(CLIENT_ID, "client", "Client"));
    const created = await call(server, "/api/v1/pairing/sessions", {
      method: "POST",
      token: host.accessToken,
    });
    await call(server, "/api/v1/pairing/sessions/claim", {
      method: "POST",
      token: client.accessToken,
      body: { ticket: created.body.ticket },
    });
    const canceled = await call(
      server,
      `/api/v1/pairing/sessions/${created.body.pairingId}/cancel`,
      { method: "POST", token: client.accessToken },
    );
    assert.equal(canceled.status, 200);
    const confirm = await call(
      server,
      `/api/v1/pairing/sessions/${created.body.pairingId}/confirm`,
      { method: "POST", token: host.accessToken, body: { code: "12345678" } },
    );
    assert.equal(confirm.status, 409);
  });
});

test("createClaimConfirm 辅助流程可用于后续客户端接入", async () => {
  await withServer(async (server) => {
    const host = await register(server, "github-alice", descriptor(HOST_ID, "host", "Host"));
    const client = await register(server, "github-bob", descriptor(CLIENT_ID, "client", "Client"));
    const result = await createClaimConfirm(server, host.accessToken, client.accessToken);
    assert.equal(result.code.length, 8);
    assert.ok(result.hostToken);
  });
});
