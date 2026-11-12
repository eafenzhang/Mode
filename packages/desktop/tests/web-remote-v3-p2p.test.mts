/**
 * 浏览器 WebControlLink 的 P2P 数据面（阶段 3 wiring）端到端。
 *
 * Node 没有原生 RTCPeerConnection，用 werift 适配器替换浏览器实现：
 * 1. capability → 服务端确认 p2pPreferred → offer/answer → DataChannel → Noise → RPC；
 * 2. 协商迟迟不开通道时按超时回落 Relay，业务不受影响。
 */
import assert from "node:assert/strict";
import test from "node:test";

import type { P2PAdapter, P2PInitiator, TransportKind } from "@mode/remote-core";
import { WebControlLink, generateWebIdentity } from "@mode/remote-core/web";
import { startRemoteServer, type RemoteServerHandle } from "@mode/remote-server";
import type { ICredentialService } from "@mode/services";

import { createWeriftP2PAdapter } from "../src/host/remoteV3/p2pWerift.js";
import { createRemoteV3HostService } from "../src/host/remoteV3/service.js";

const WEB_DEVICE_ID = "019a0000-0000-7000-8000-0000000000d2";

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

interface Fixture {
  server: RemoteServerHandle;
  hostService: Awaited<ReturnType<typeof createRemoteV3HostService>>;
  hostDeviceId: string;
  identity: ReturnType<typeof generateWebIdentity>;
  accessToken: string;
}

async function setup(): Promise<Fixture> {
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
  const hostState = await hostService.configure({
    serverUrl: server.url,
    webSessionToken: await githubSession(server, "github-web-p2p"),
    deviceName: "Web P2P Host",
  });
  assert.equal(hostState.status, "online", hostState.error ?? "");
  assert.ok(hostState.deviceId);
  const identity = generateWebIdentity();
  const response = await fetch(`${server.url}/api/v1/devices`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${await githubSession(server, "github-web-p2p")}`,
    },
    body: JSON.stringify({
      device: {
        deviceId: WEB_DEVICE_ID,
        name: "Web Client",
        role: "client",
        platform: "web",
        identityKey: identity.identityKey,
      },
    }),
  });
  assert.equal(response.status, 201, await response.clone().text());
  const { accessToken } = (await response.json()) as { accessToken: string };
  return { server, hostService, hostDeviceId: hostState.deviceId, identity, accessToken };
}

function deadAdapter(): P2PAdapter {
  const initiator: P2PInitiator = {
    createOffer: async () => "dead-offer-sdp",
    acceptAnswer: async () => undefined,
    onLocalCandidate: () => undefined,
    addRemoteCandidate: () => undefined,
    onChannel: () => undefined,
    close: () => undefined,
  };
  return {
    createInitiator: () => initiator,
    createResponder: () => {
      throw new Error("browser client 不创建 responder");
    },
  };
}

async function runFixture(
  fixture: Fixture,
  p2p: { adapter: P2PAdapter; timeoutMs: number },
): Promise<{ transports: TransportKind[]; link: WebControlLink }> {
  const transports: TransportKind[] = [];
  const link = new WebControlLink({
    serverUrl: fixture.server.url,
    deviceId: WEB_DEVICE_ID,
    accessToken: fixture.accessToken,
    identity: fixture.identity,
    p2p,
    onTransport: (transport) => transports.push(transport),
  });
  await link.start();
  await link.connect(fixture.hostDeviceId);
  return { transports, link };
}

test("浏览器 WebControlLink 走 werift DataChannel 完成 P2P RPC", async () => {
  const fixture = await setup();
  let link: WebControlLink | undefined;
  try {
    const result = await runFixture(fixture, {
      adapter: createWeriftP2PAdapter(),
      timeoutMs: 8_000,
    });
    link = result.link;
    assert.deepEqual(result.transports, ["p2p"], "浏览器链路必须命中 p2p 选路");
    assert.deepEqual(await link.request("remote.echo", { value: "web-p2p" }, 15_000), {
      value: "web-p2p",
    });
    // 浏览器侧缺少分片层时，>48KB 的响应会被丢弃；200KB 同时覆盖双向分片。
    const large = "x".repeat(200_000);
    const echoed = (await link.request("remote.echo", { value: large }, 20_000)) as {
      value?: string;
    };
    assert.equal(echoed.value?.length, large.length, "大响应必须完整分片重组");
    assert.equal(echoed.value, large);
  } finally {
    link?.close();
    await fixture.hostService.disconnectAccount();
    await fixture.server.close();
  }
});

test("P2P 协商超时后回落 Relay，业务请求仍然成功", async () => {
  const fixture = await setup();
  let link: WebControlLink | undefined;
  try {
    const result = await runFixture(fixture, { adapter: deadAdapter(), timeoutMs: 300 });
    link = result.link;
    assert.deepEqual(result.transports, ["relay"], "未打开 DataChannel 时必须回落 Relay");
    assert.deepEqual(await link.request("remote.echo", { value: "relay-fallback" }, 15_000), {
      value: "relay-fallback",
    });
    const large = "y".repeat(200_000);
    const echoed = (await link.request("remote.echo", { value: large }, 20_000)) as {
      value?: string;
    };
    assert.equal(echoed.value, large, "Relay 路径同样需要分片重组");
  } finally {
    link?.close();
    await fixture.hostService.disconnectAccount();
    await fixture.server.close();
  }
});

test("Host 重启后浏览器自动重连并重新协商 P2P", async () => {
  const fixture = await setup();
  let link: WebControlLink | undefined;
  try {
    const result = await runFixture(fixture, {
      adapter: createWeriftP2PAdapter(),
      timeoutMs: 8_000,
    });
    link = result.link;
    assert.deepEqual(result.transports, ["p2p"]);

    // Host 下线再上线：Control 断开触发浏览器重连，重连后必须重新协商数据面。
    await fixture.hostService.disconnectAccount();
    const again = await fixture.hostService.configure({
      serverUrl: fixture.server.url,
      webSessionToken: await githubSession(fixture.server, "github-web-p2p"),
      deviceName: "Web P2P Host",
    });
    assert.equal(again.status, "online", again.error ?? "");

    const value = await waitForRpc(link, "after-p2p-reconnect", 15_000);
    assert.deepEqual(value, { value: "after-p2p-reconnect" });
    assert.equal(
      result.transports.filter((transport) => transport === "p2p").length >= 2,
      true,
      `重连后应再次命中 p2p，实际：${result.transports.join(",")}`,
    );
  } finally {
    link?.close();
    await fixture.hostService.disconnectAccount();
    await fixture.server.close();
  }
});

async function waitForRpc(
  link: WebControlLink,
  value: string,
  timeoutMs: number,
): Promise<unknown> {
  const deadline = Date.now() + timeoutMs;
  let lastError: unknown;
  while (Date.now() < deadline) {
    try {
      return await link.request("remote.echo", { value }, 2_000);
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 200));
    }
  }
  throw lastError instanceof Error ? lastError : new Error("等待重连后的 RPC 超时");
}
