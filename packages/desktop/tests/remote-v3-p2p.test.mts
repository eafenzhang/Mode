/**
 * WebRTC/P2P 阶段 2：跨账号配对后，两台桌面 Host 之间的 allowlist RPC 走
 * werift DataChannel（loopback host candidates，无需 STUN/TURN）。
 *
 * 断言：
 * 1. 配对流程完成后 B 的 Client 目录能看到 A；
 * 2. B 对 A 的加密 RPC 选路命中 `p2p`（onTransport 事件）；
 * 3. RPC 在 p2p 协商超时阈值内完成（若回落 Relay，8s 协商超时会先发生并使断言失败）。
 */
import assert from "node:assert/strict";
import test from "node:test";

import type { TransportKind } from "@mode/remote-core";
import { startRemoteServer, type RemoteServerHandle } from "@mode/remote-server";
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

async function startService(input: {
  server: RemoteServerHandle;
  github: string;
  deviceName: string;
  credentials: ICredentialService;
  transports: TransportKind[];
}): Promise<Awaited<ReturnType<typeof createRemoteV3HostService>>> {
  const service = await createRemoteV3HostService({
    credentials: input.credentials,
    env: {},
    log: (line: string) => console.error(`[${input.deviceName}]`, line),
    version: "0.0.1-test",
    deviceName: input.deviceName,
    // p2p 适配器：service.ts 内部默认使用 werift 适配器（createWeriftP2PAdapter）。
    onTransport: (transport) => input.transports.push(transport),
  });
  const session = await fetch(`${input.server.url}/api/v1/auth/github`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ accessToken: input.github }),
  });
  const { sessionToken } = (await session.json()) as { sessionToken: string };
  const configured = await service.configure({
    serverUrl: input.server.url,
    webSessionToken: sessionToken,
    deviceName: input.deviceName,
  });
  assert.equal(configured.status, "online", configured.error ?? "");
  return service;
}

test("配对后跨账号 RPC 走 werift DataChannel（p2p）", async () => {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "p2p-e2e-secret-long-enough",
    githubVerifier: async (token) => {
      if (token === "github-alice") return { providerUserId: "alice", login: "alice" };
      if (token === "github-bob") return { providerUserId: "bob", login: "bob" };
      throw new Error("invalid github token");
    },
  });
  const alice = createFakeCredentials();
  const bob = createFakeCredentials();
  const transportsB: TransportKind[] = [];
  let serviceA: Awaited<ReturnType<typeof createRemoteV3HostService>> | undefined;
  let serviceB: Awaited<ReturnType<typeof createRemoteV3HostService>> | undefined;
  try {
    serviceA = await startService({
      server,
      github: "github-alice",
      deviceName: "Host A",
      credentials: alice.credentials,
      transports: [],
    });
    serviceB = await startService({
      server,
      github: "github-bob",
      deviceName: "Host B",
      credentials: bob.credentials,
      transports: transportsB,
    });

    const started = await serviceA.beginPairing();
    const claim = await serviceB.claimPairing(started.ticket);
    await serviceA.confirmPairing({
      pairingId: started.pairingId,
      code: claim.verificationCode,
    });
    await serviceB.redeemPairing(started.pairingId);

    const hosts = await serviceB.listHosts();
    const hostA = hosts.find((host) => host.name === "Host A");
    assert.ok(hostA, "配对后 B 的目录必须能看到 A");

    const result = await serviceB.request({
      hostDeviceId: hostA.deviceId,
      method: "remote.echo",
      params: { value: "p2p-hello" },
      timeoutMs: 15_000,
    });
    assert.deepEqual(result.result, { value: "p2p-hello" });
    assert.ok(
      transportsB.includes("p2p"),
      `B 的选路必须命中 p2p，实际：${transportsB.join(",")}`,
    );
  } finally {
    if (serviceA) await serviceA.disconnectAccount();
    if (serviceB) await serviceB.disconnectAccount();
    await server.close();
  }
});
