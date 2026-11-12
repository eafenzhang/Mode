/**
 * 浏览器 WebControlLink 的 P2P 数据面（阶段 3 wiring）端到端：
 * 用 werift 适配器替换浏览器原生 RTCPeerConnection（Node 无 WebRTC），验证
 * hello capability → 服务端确认 p2pPreferred → offer/answer → DataChannel →
 * Noise → allowlist RPC 全链路，并断言选路事件为 p2p。
 */
import assert from "node:assert/strict";
import test from "node:test";

import type { TransportKind } from "@mode/remote-core";
import { WebControlLink, generateWebIdentity } from "@mode/remote-core/web";
import { startRemoteServer, type RemoteServerHandle } from "@mode/remote-server";
import type { ICredentialService } from "@mode/services";

import { createWeriftP2PAdapter } from "../src/host/remoteV3/p2pWerift.js";
import { createRemoteV3HostService } from "../src/host/remoteV3/service.js";

function createFakeCredentials(): ICredentialService {
  const store = new Map<string, string>();
  return {
    load: async (key: string) => store.get(key) ?? null,
    save: async (key: string, value: string) => {
      store.set(key, value);
    },
    delete: async (key: string) => {
      store.delete(key);
    },
    list: async (prefix: string) => [...store.keys()].filter((key) => key.startsWith(prefix)),
  } as unknown as ICredentialService;
}

async function githubSession(server: RemoteServerHandle, token: string): Promise<string> {
  const response = await fetch(`${server.url}/api/v1/auth/github`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ accessToken: token }),
  });
  assert.equal(response.status, 200);
  return ((await response.json()) as { sessionToken: string }).sessionToken;
}

async function registerClient(
  server: RemoteServerHandle,
  sessionToken: string,
  device: Record<string, unknown>,
): Promise<string> {
  const response = await fetch(`${server.url}/api/v1/devices`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${sessionToken}` },
    body: JSON.stringify({ device }),
  });
  assert.equal(response.status, 201, await response.clone().text());
  return ((await response.json()) as { accessToken: string }).accessToken;
}

test("浏览器 WebControlLink 走 werift DataChannel 完成 P2P RPC", async () => {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "web-p2p-e2e-secret-long-enough",
    githubVerifier: async () => ({ providerUserId: "web-p2p", login: "web-p2p" }),
  });
  const hostService = await createRemoteV3HostService({
    credentials: createFakeCredentials(),
    env: {},
    log: () => undefined,
    version: "0.0.1-test",
    deviceName: "Web P2P Host",
  });
  const identity = generateWebIdentity();
  const webDeviceId = "019a0000-0000-7000-8000-0000000000d2";
  const transports: TransportKind[] = [];
  let link: WebControlLink | undefined;
  try {
    const hostState = await hostService.configure({
      serverUrl: server.url,
      webSessionToken: await githubSession(server, "github-web-p2p"),
      deviceName: "Web P2P Host",
    });
    assert.equal(hostState.status, "online", hostState.error ?? "");
    assert.ok(hostState.deviceId);
    const accessToken = await registerClient(
      server,
      await githubSession(server, "github-web-p2p"),
      {
        deviceId: webDeviceId,
        name: "Web Client",
        role: "client",
        platform: "web",
        identityKey: identity.identityKey,
      },
    );

    link = new WebControlLink({
      serverUrl: server.url,
      deviceId: webDeviceId,
      accessToken,
      identity,
      p2p: { adapter: createWeriftP2PAdapter(), timeoutMs: 8_000 },
      onTransport: (transport) => transports.push(transport),
    });
    await link.start();
    await link.connect(hostState.deviceId);

    assert.deepEqual(transports, ["p2p"], "浏览器链路必须命中 p2p 选路");
    const result = await link.request("remote.echo", { value: "web-p2p" }, 15_000);
    assert.deepEqual(result, { value: "web-p2p" });
  } finally {
    link?.close();
    await hostService.disconnectAccount();
    await server.close();
  }
});
