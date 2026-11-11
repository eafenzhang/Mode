/**
 * 跨账号配对 E2E（docs/specs/remote-control-v3-pairing.md）：
 * 桌面 Host（A 账号）创建/ticket 与确认，浏览器 Client（B 账号）claim/redeem。
 *
 * 使用真实 @mode/remote-server（内存库 + 假 GitHub 验签）与真实 Host 服务；
 * 不访问外部网络。
 */
import assert from "node:assert/strict";
import test from "node:test";

import { WebAccountClient, type WebCredentialStore } from "@mode/remote-core/web";
import { startRemoteServer } from "@mode/remote-server";
import type { ICredentialService } from "@mode/services";

import { createRemoteV3HostService } from "../src/host/remoteV3/service.js";

function createFakeCredentials(): {
  credentials: ICredentialService;
  store: Map<string, string>;
} {
  const store = new Map<string, string>();
  const credentials = {
    load: async (key: string) => store.get(key) ?? null,
    save: async (key: string, value: string) => {
      store.set(key, value);
    },
    delete: async (key: string) => {
      store.delete(key);
    },
    list: async (prefix: string) => [...store.keys()].filter((key) => key.startsWith(prefix)),
  } as unknown as ICredentialService;
  return { credentials, store };
}

function createWebStore(): WebCredentialStore {
  const map = new Map<string, string>();
  return {
    get: async (key) => map.get(key) ?? null,
    set: async (key, value) => {
      map.set(key, value);
    },
    delete: async (key) => {
      map.delete(key);
    },
  };
}

async function login(serverUrl: string, githubToken: string): Promise<string> {
  const response = await fetch(`${serverUrl}/api/v1/auth/github`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ accessToken: githubToken }),
  });
  const body = (await response.json()) as { sessionToken?: string };
  assert.equal(response.status, 200, JSON.stringify(body));
  assert.ok(body.sessionToken);
  return body.sessionToken;
}

test("跨账号配对：Host 创建与确认，浏览器 Client claim 后 redeem 换新凭证", async () => {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "pairing-e2e-secret-long-enough",
    githubVerifier: async (token) => {
      if (token === "github-alice") return { providerUserId: "alice", login: "alice" };
      if (token === "github-bob") return { providerUserId: "bob", login: "bob" };
      throw new Error("invalid github token");
    },
  });
  const { credentials } = createFakeCredentials();
  const service = await createRemoteV3HostService({
    credentials,
    env: {},
    log: () => undefined,
    version: "0.0.1-test",
    deviceName: "V3 Host",
  });
  try {
    const aliceSession = await login(server.url, "github-alice");
    const configured = await service.configure({
      serverUrl: server.url,
      webSessionToken: aliceSession,
      deviceName: "V3 Host",
    });
    assert.equal(configured.status, "online", configured.error ?? "");
    const hostDeviceId = configured.deviceId;
    assert.ok(hostDeviceId);

    const webStore = createWebStore();
    const client = new WebAccountClient({
      store: webStore,
      serverUrl: server.url,
      deviceName: "Bob Web",
    });
    await client.loginWithGithubToken("github-bob");

    const started = await service.beginPairing();
    assert.ok(started.pairingId);
    assert.ok(started.ticket);

    const claim = await client.claimPairing(started.ticket);
    assert.equal(claim.pairingId, started.pairingId);
    assert.match(claim.verificationCode, /^\d{8}$/u);
    assert.equal(claim.hostName, "V3 Host");

    const confirmed = await service.confirmPairing({
      pairingId: started.pairingId,
      code: claim.verificationCode,
    });
    assert.equal(confirmed.status, "online", confirmed.error ?? "");

    const credentials = await client.redeemPairing(started.pairingId);
    assert.ok(credentials.accessToken);
    assert.equal((await client.getPairing(started.pairingId)).status, "confirmed");

    const hosts = await client.listHosts();
    assert.ok(
      hosts.some((host) => host.deviceId === hostDeviceId),
      "配对后 Client 目录必须能看到 Host",
    );
    assert.equal((await client.getPairing(started.pairingId)).status, "confirmed");
  } finally {
    await service.disconnectAccount();
    await server.close();
  }
});

test("配对取消后 Client claim 失败", async () => {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "pairing-e2e-secret-long-enough",
    githubVerifier: async (token) => {
      if (token === "github-alice") return { providerUserId: "alice", login: "alice" };
      if (token === "github-bob") return { providerUserId: "bob", login: "bob" };
      throw new Error("invalid github token");
    },
  });
  const { credentials } = createFakeCredentials();
  const service = await createRemoteV3HostService({
    credentials,
    env: {},
    log: () => undefined,
    version: "0.0.1-test",
    deviceName: "V3 Host",
  });
  try {
    const aliceSession = await login(server.url, "github-alice");
    await service.configure({
      serverUrl: server.url,
      webSessionToken: aliceSession,
      deviceName: "V3 Host",
    });

    const webStore = createWebStore();
    const client = new WebAccountClient({
      store: webStore,
      serverUrl: server.url,
      deviceName: "Bob Web",
    });
    await client.loginWithGithubToken("github-bob");

    const started = await service.beginPairing();
    const canceled = await service.cancelPairing(started.pairingId);
    assert.equal(canceled.status, "canceled");
    await assert.rejects(client.claimPairing(started.ticket), /配对会话已被使用|配对票无效/u);
  } finally {
    await service.disconnectAccount();
    await server.close();
  }
});
