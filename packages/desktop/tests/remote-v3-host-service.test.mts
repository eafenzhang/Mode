/**
 * 远程控制 v3 Host 服务（docs/specs/remote-control-v3.md）：
 * 设备注册 → Control 常驻 → Noise 握手 → 加密 RPC allowlist。
 * 使用真实 @mode/remote-server（临时内存库 + 假 GitHub 验签），不访问外部网络。
 */
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import test from "node:test";
import { WebSocket, WebSocketServer } from "ws";

import {
  ControlLink,
  decodeBase64Url,
  encodeBase64Url,
  generateIdentity,
  toControlUrl,
  type BusinessFrame,
  type DeviceDescriptor,
  type RemoteConnection,
  type RemoteSocket,
} from "@mode/remote-core";
import type { RemoteV3TunnelEvent } from "@mode/shared";
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

function connectSocket(url: string): Promise<RemoteSocket> {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url);
    const socket: RemoteSocket = {
      send: (text) => ws.send(text),
      close: (code, reason) => ws.close(code, reason),
      onMessage: (callback) =>
        ws.on("message", (data, isBinary) => {
          if (!isBinary) callback(data.toString());
        }),
      onClose: (callback) => ws.on("close", (code, reason) => callback(code, reason.toString())),
    };
    ws.once("open", () => resolve(socket));
    ws.once("error", reject);
  });
}

async function login(serverUrl: string, accessToken = "github-v3-test"): Promise<string> {
  const response = await fetch(`${serverUrl}/api/v1/auth/github`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ accessToken }),
  });
  const text = await response.text();
  assert.equal(response.status, 200, text);
  return (JSON.parse(text) as { sessionToken: string }).sessionToken;
}

async function registerClient(
  serverUrl: string,
  sessionToken: string,
): Promise<{
  descriptor: DeviceDescriptor;
  accessToken: string;
  identity: ReturnType<typeof generateIdentity>;
}> {
  const identity = generateIdentity();
  const descriptor: DeviceDescriptor = {
    deviceId: randomUUID(),
    name: "V3 Test Client",
    role: "client",
    platform: "desktop",
    identityKey: identity.identityKey,
  };
  const response = await fetch(`${serverUrl}/api/v1/devices`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${sessionToken}`,
    },
    body: JSON.stringify({ device: descriptor }),
  });
  const text = await response.text();
  assert.equal(response.status, 201, text);
  return {
    descriptor,
    accessToken: (JSON.parse(text) as { accessToken: string }).accessToken,
    identity,
  };
}

async function fetchPeer(
  serverUrl: string,
  accessToken: string,
  deviceId: string,
): Promise<DeviceDescriptor> {
  const response = await fetch(`${serverUrl}/api/v1/devices/${deviceId}`, {
    headers: { authorization: `Bearer ${accessToken}` },
  });
  const text = await response.text();
  assert.equal(response.status, 200, text);
  return (JSON.parse(text) as { device: DeviceDescriptor }).device;
}

function waitForFrame(
  connection: RemoteConnection,
  predicate: (frame: BusinessFrame) => boolean,
  timeoutMs = 3000,
): Promise<BusinessFrame> {
  return new Promise((resolve, reject) => {
    let dispose = (): void => undefined;
    const timer = setTimeout(() => {
      dispose();
      reject(new Error("等待 RPC 响应超时"));
    }, timeoutMs);
    dispose = connection.onFrame((frame) => {
      if (!predicate(frame)) return;
      clearTimeout(timer);
      dispose();
      resolve(frame);
    });
  });
}

test("v3 Host 服务注册设备并通过加密通道响应 RPC allowlist", async () => {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "desktop-v3-test-secret-long-enough",
    githubVerifier: async () => ({ providerUserId: "501", login: "v3" }),
  });
  const localServer = createServer((req, res) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk) => chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk)));
    req.on("end", () => {
      if (req.method === "POST" && req.url === "/api/echo") {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ method: req.method, body: Buffer.concat(chunks).toString() }));
        return;
      }
      if (req.method === "GET" && req.url === "/api/state") {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ ok: true }));
        return;
      }
      if (req.method === "POST" && req.url?.startsWith("/api/remote-desktop/session")) {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ ticket: "signal-ticket", mediaTicket: "media-ticket" }));
        return;
      }
      res.writeHead(404, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: "not_found" }));
    });
  });
  const echoServer = new WebSocketServer({ server: localServer });
  echoServer.on("connection", (socket, req) => {
    if (req.url?.startsWith("/ws/echo")) {
      socket.send(JSON.stringify({ header: req.headers["x-mode-rpc-host-capability"] ?? null }), {
        binary: false,
      });
    }
    socket.on("message", (data, isBinary) => socket.send(data, { binary: isBinary }));
  });
  await new Promise<void>((resolve) => localServer.listen(0, "127.0.0.1", resolve));
  const localPort = (localServer.address() as AddressInfo).port;
  const { credentials } = createFakeCredentials();

  const service = await createRemoteV3HostService({
    credentials,
    env: {},
    log: () => undefined,
    version: "0.0.1-test",
    deviceName: "V3 Host",
    listLanAddresses: async () => ["192.168.1.50:45880"],
    listWorkspaces: async () => [{ path: "C:\\work", name: "work" }],
    getLocalApi: async () => ({
      baseUrl: `http://127.0.0.1:${localPort}`,
      token: "test-local-token",
    }),
  });
  const sessionToken = await login(server.url);
  try {
    const configured = await service.configure({
      serverUrl: server.url,
      webSessionToken: sessionToken,
      deviceName: "V3 Host",
    });
    assert.equal(configured.status, "online", configured.error ?? "");
    assert.ok(configured.deviceId);
    assert.equal(configured.deviceName, "V3 Host");
    assert.equal(configured.fingerprint?.length, 64);

    const client = await registerClient(server.url, sessionToken);
    const clientLink = new ControlLink({
      role: "client",
      serverUrl: server.url,
      deviceId: client.descriptor.deviceId,
      accessToken: client.accessToken,
      identity: client.identity,
      connectSocket,
      fetchPeer: (deviceId) => fetchPeer(server.url, client.accessToken, deviceId),
    });
    await clientLink.start();
    try {
      const connection = await clientLink.connect(configured.deviceId!);
      assert.equal(connection.peer.deviceId, configured.deviceId);

      const echoId = randomUUID();
      connection.send({
        v: 1,
        type: "rpc.request",
        id: echoId,
        method: "remote.echo",
        params: { value: "hello-v3" },
      });
      const echo = await waitForFrame(
        connection,
        (frame) => frame.type === "rpc.response" && frame.id === echoId,
      );
      assert.deepEqual(echo, {
        v: 1,
        type: "rpc.response",
        id: echoId,
        result: { value: "hello-v3" },
      });

      const infoId = randomUUID();
      connection.send({
        v: 1,
        type: "rpc.request",
        id: infoId,
        method: "remote.host.info",
        params: {},
      });
      const info = await waitForFrame(
        connection,
        (frame) => frame.type === "rpc.response" && frame.id === infoId,
      );
      assert.equal(info.type, "rpc.response");
      if (info.type === "rpc.response") {
        const result = info.result as { deviceId?: string; fingerprint?: string };
        assert.equal(result.deviceId, configured.deviceId);
        assert.equal(result.fingerprint, configured.fingerprint);
      }

      const unknownId = randomUUID();
      connection.send({
        v: 1,
        type: "rpc.request",
        id: unknownId,
        method: "remote.unknown",
        params: {},
      });
      const unknown = await waitForFrame(
        connection,
        (frame) => frame.type === "rpc.error" && frame.id === unknownId,
      );
      assert.equal(unknown.type, "rpc.error");
      if (unknown.type === "rpc.error") {
        assert.equal(unknown.code, "METHOD_NOT_FOUND");
      }

      await service.resetPeerPin({ deviceId: client.descriptor.deviceId });
    } finally {
      clientLink.close();
    }

    const selfInfo = await service.request({
      hostDeviceId: configured.deviceId!,
      method: "remote.host.info",
    });
    assert.equal((selfInfo.result as { deviceId?: string }).deviceId, configured.deviceId);
    const selfEcho = await service.request({
      hostDeviceId: configured.deviceId!,
      method: "remote.echo",
      params: { value: "self-loop" },
    });
    assert.deepEqual(selfEcho.result, { value: "self-loop" });

    const tunnelEvents: RemoteV3TunnelEvent[] = [];
    const tunnelSub = service.onTunnelEvent((event) => tunnelEvents.push(event));
    const localConnected = new Promise<void>((resolve) =>
      echoServer.once("connection", () => resolve()),
    );
    const { tunnelId } = await service.openTunnel({
      hostDeviceId: configured.deviceId!,
      path: "/ws/echo",
      headers: { "x-mode-rpc-host-capability": "cap-123" },
    });
    await localConnected;
    await waitFor(() =>
      tunnelEvents.some(
        (event) =>
          event.kind === "message" &&
          Buffer.from(decodeBase64Url(event.data!, "tunnel.data")).toString().includes("cap-123"),
      ),
    );
    await service.sendTunnel(
      tunnelId,
      encodeBase64Url(Buffer.concat([Buffer.from([0]), Buffer.from("hello-tunnel")])),
    );
    await waitFor(() =>
      tunnelEvents.some(
        (event) =>
          event.kind === "message" &&
          Buffer.from(decodeBase64Url(event.data!, "tunnel.data"))
            .toString()
            .includes("hello-tunnel"),
      ),
    );
    const message = tunnelEvents.find(
      (event) =>
        event.kind === "message" &&
        Buffer.from(decodeBase64Url(event.data!, "tunnel.data"))
          .toString()
          .includes("hello-tunnel"),
    )!;
    const payload = decodeBase64Url(message.data!, "tunnel.data");
    assert.equal(payload[0], 0);
    assert.equal(Buffer.from(payload.subarray(1)).toString(), "hello-tunnel");
    await service.closeTunnel(tunnelId);
    tunnelSub.dispose();

    const httpResponse = await service.httpRequest({
      hostDeviceId: configured.deviceId!,
      method: "POST",
      path: "/api/echo",
      headers: { "content-type": "application/json" },
      bodyBase64: encodeBase64Url(Buffer.from(JSON.stringify({ msg: "v3" }))),
    });
    assert.equal(httpResponse.status, 200);
    assert.deepEqual(
      JSON.parse(Buffer.from(decodeBase64Url(httpResponse.bodyBase64, "body")).toString()),
      { method: "POST", body: '{"msg":"v3"}' },
    );
    await assert.rejects(
      service.httpRequest({
        hostDeviceId: configured.deviceId!,
        method: "GET",
        path: "/api/lan/state",
      }),
      /blocked http path/,
    );

    const hosts = await service.listHosts();
    assert.equal(
      hosts.some((host) => host.deviceId === configured.deviceId),
      false,
      "个人账号体系下本机 Host 不出现在可连设备列表",
    );
    const workspaces = await service.request({
      hostDeviceId: configured.deviceId!,
      method: "remote.workspace.list",
    });
    assert.deepEqual(workspaces.result, {
      items: [{ path: "C:\\work", name: "work" }],
    });
    const remoteDesktop = await service.startRemoteDesktop({
      hostDeviceId: configured.deviceId!,
      quality: "balanced",
      codec: "h264",
    });
    const signal = new WebSocket(remoteDesktop.signalUrl);
    await new Promise<void>((resolve, reject) => {
      signal.once("open", () => resolve());
      signal.once("error", reject);
    });
    const signalEcho = new Promise<string>((resolve) =>
      signal.once("message", (data) => resolve(data.toString())),
    );
    signal.send("hello-signal");
    assert.equal(await signalEcho, "hello-signal");

    const media = new WebSocket(remoteDesktop.mediaUrl);
    await new Promise<void>((resolve, reject) => {
      media.once("open", () => resolve());
      media.once("error", reject);
    });
    const mediaEcho = new Promise<Buffer>((resolve) =>
      media.once("message", (data) => resolve(Buffer.from(data as Buffer))),
    );
    media.send(Buffer.from([1, 2, 3]), { binary: true });
    assert.deepEqual(await mediaEcho, Buffer.from([1, 2, 3]));
    media.close();
    signal.close();

    const disconnected = await service.disconnectAccount();
    assert.equal(disconnected.status, "unconfigured");
    assert.equal((await service.getState()).status, "unconfigured");
  } finally {
    await new Promise<void>((resolve) => echoServer.close(() => resolve()));
    await new Promise<void>((resolve) => localServer.close(() => resolve()));
    await server.close();
  }
});

async function waitFor(predicate: () => boolean, timeoutMs = 3000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error("等待运行时事件超时");
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

async function waitForAsync(predicate: () => Promise<boolean>, timeoutMs = 3000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!(await predicate())) {
    if (Date.now() > deadline) throw new Error("等待运行时事件超时");
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

test("v3 Host 切换 GitHub 账号时轮换设备身份", async () => {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "desktop-v3-account-switch-secret",
    githubVerifier: async (accessToken) =>
      accessToken === "gh-a"
        ? { providerUserId: "601", login: "account-a" }
        : { providerUserId: "602", login: "account-b" },
  });
  const { credentials } = createFakeCredentials();
  const service = await createRemoteV3HostService({
    credentials,
    env: {},
    log: () => undefined,
    version: "0.0.1-test",
    deviceName: "V3 Account Host",
  });
  try {
    const sessionA = await login(server.url, "gh-a");
    const first = await service.configure({
      serverUrl: server.url,
      webSessionToken: sessionA,
      deviceName: "Account A Host",
    });
    assert.equal(first.status, "online", first.error ?? "");
    assert.ok(first.deviceId);

    // 同账号重复配置不能轮换身份，否则每次登录都会让对端 pin 失效。
    const sameAccount = await service.configure({
      serverUrl: server.url,
      webSessionToken: sessionA,
      deviceName: "Account A Host",
    });
    assert.equal(sameAccount.deviceId, first.deviceId);

    // deviceId 在服务端绑定唯一账号；换账号必须轮换，否则注册会 409。
    const sessionB = await login(server.url, "gh-b");
    const second = await service.configure({
      serverUrl: server.url,
      webSessionToken: sessionB,
      deviceName: "Account B Host",
    });
    assert.equal(second.status, "online", second.error ?? "");
    assert.ok(second.deviceId);
    assert.notEqual(second.deviceId, first.deviceId);

    // 新账号的设备目录里不应出现旧账号的 Host。
    const hosts = await service.listHosts();
    assert.equal(
      hosts.some((host) => host.deviceId === first.deviceId),
      false,
    );
    await service.disconnectAccount();
  } finally {
    await server.close();
  }
});

test("v3 Host 可同时接受多个 Client 的加密连接", async () => {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "desktop-v3-multi-client-secret",
    githubVerifier: async () => ({ providerUserId: "604", login: "account-multi" }),
  });
  const { credentials } = createFakeCredentials();
  const service = await createRemoteV3HostService({
    credentials,
    env: {},
    log: () => undefined,
    version: "0.0.1-test",
    deviceName: "V3 Multi Host",
  });
  const links: ControlLink[] = [];
  try {
    const sessionToken = await login(server.url, "github-multi");
    const configured = await service.configure({
      serverUrl: server.url,
      webSessionToken: sessionToken,
      deviceName: "V3 Multi Host",
    });
    assert.equal(configured.status, "online", configured.error ?? "");

    for (let index = 0; index < 2; index += 1) {
      const client = await registerClient(server.url, sessionToken);
      const link = new ControlLink({
        role: "client",
        serverUrl: server.url,
        deviceId: client.descriptor.deviceId,
        accessToken: client.accessToken,
        identity: client.identity,
        connectSocket,
        fetchPeer: (peerDeviceId) => fetchPeer(server.url, client.accessToken, peerDeviceId),
        connectTimeoutMs: 3_000,
      });
      await link.start();
      links.push(link);
      const connection = await link.connect(configured.deviceId!);
      const echoId = randomUUID();
      connection.send({
        v: 1,
        type: "rpc.request",
        id: echoId,
        method: "remote.echo",
        params: { value: `client-${index}` },
      });
      const echo = await waitForFrame(
        connection,
        (frame) => frame.type === "rpc.response" && frame.id === echoId,
      );
      assert.deepEqual(echo, {
        v: 1,
        type: "rpc.response",
        id: echoId,
        result: { value: `client-${index}` },
      });
    }
    assert.equal((await service.getState()).activeConnections, 2);
    await service.disconnectAccount();
  } finally {
    for (const link of links) link.close();
    await server.close();
  }
});

test("v3 Host 设备被账号主撤销后可轮换身份重新配置", async () => {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "desktop-v3-revoked-secret-long-enough",
    githubVerifier: async () => ({ providerUserId: "603", login: "account-revoke" }),
  });
  const { credentials } = createFakeCredentials();
  const service = await createRemoteV3HostService({
    credentials,
    env: {},
    log: () => undefined,
    version: "0.0.1-test",
    deviceName: "V3 Revoked Host",
  });
  try {
    const sessionToken = await login(server.url, "github-revoke");
    const first = await service.configure({
      serverUrl: server.url,
      webSessionToken: sessionToken,
      deviceName: "V3 Revoked Host",
    });
    assert.equal(first.status, "online", first.error ?? "");
    assert.ok(first.deviceId);

    const revoked = await fetch(`${server.url}/api/v1/devices/${first.deviceId}`, {
      method: "DELETE",
      headers: { authorization: `Bearer ${sessionToken}` },
    });
    assert.equal(revoked.status, 200, await revoked.text());

    // 同 deviceId 再注册会 409 device_revoked；Host 应轮换身份而不是停在错误态。
    const second = await service.configure({
      serverUrl: server.url,
      webSessionToken: sessionToken,
      deviceName: "V3 Revoked Host",
    });
    assert.equal(second.status, "online", second.error ?? "");
    assert.notEqual(second.deviceId, first.deviceId);
    await service.disconnectAccount();
  } finally {
    await server.close();
  }
});

test("v3 Host 被顶替后自动重连并恢复加密 RPC", async () => {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "desktop-v3-reconnect-secret-long-enough",
    githubVerifier: async () => ({ providerUserId: "502", login: "v3-reconnect" }),
  });
  const { credentials, store } = createFakeCredentials();
  const service = await createRemoteV3HostService({
    credentials,
    env: {},
    log: () => undefined,
    version: "0.0.1-test",
    deviceName: "V3 Reconnect Host",
  });
  const sessionToken = await login(server.url);
  let duplicate: WebSocket | null = null;
  let clientLink: ControlLink | null = null;
  try {
    const configured = await service.configure({
      serverUrl: server.url,
      webSessionToken: sessionToken,
      deviceName: "V3 Reconnect Host",
    });
    assert.equal(configured.status, "online", configured.error ?? "");
    const deviceId = store.get("remote:v3:device-id");
    const accessToken = store.get("remote:v3:access-token");
    assert.ok(deviceId, "Host deviceId 未落库");
    assert.ok(accessToken, "Host access token 未落库");

    // 同设备再建一条 Control：服务端以 4003 顶掉现有 Host 连接，
    // Host 必须观察到非主动关闭并自动重连，而不是永久停在 online。
    duplicate = new WebSocket(toControlUrl(server.url));
    await new Promise<void>((resolve, reject) => {
      duplicate!.once("open", () => resolve());
      duplicate!.once("error", reject);
    });
    duplicate.send(
      JSON.stringify({
        v: 1,
        id: randomUUID(),
        timestamp: Date.now(),
        type: "hello",
        payload: {
          role: "host",
          deviceId,
          accessToken,
          protocols: [1],
          capabilities: ["secure.noise-ik.v1", "transport.relay"],
        },
      }),
    );

    await waitForAsync(async () => (await service.getState()).status === "reconnecting", 3000);
    await waitForAsync(async () => (await service.getState()).status === "online", 5000);

    // 重连后新 Client 仍能完成 Noise 握手并执行 allowlist RPC。
    const client = await registerClient(server.url, sessionToken);
    clientLink = new ControlLink({
      role: "client",
      serverUrl: server.url,
      deviceId: client.descriptor.deviceId,
      accessToken: client.accessToken,
      identity: client.identity,
      connectSocket,
      fetchPeer: (peerDeviceId) => fetchPeer(server.url, client.accessToken, peerDeviceId),
      connectTimeoutMs: 3_000,
    });
    await clientLink.start();
    const connection = await clientLink.connect(deviceId);
    const echoId = randomUUID();
    connection.send({
      v: 1,
      type: "rpc.request",
      id: echoId,
      method: "remote.echo",
      params: { value: "after-reconnect" },
    });
    const echo = await waitForFrame(
      connection,
      (frame) => frame.type === "rpc.response" && frame.id === echoId,
    );
    assert.deepEqual(echo, {
      v: 1,
      type: "rpc.response",
      id: echoId,
      result: { value: "after-reconnect" },
    });
  } finally {
    clientLink?.close();
    duplicate?.close();
    await service.disconnectAccount().catch(() => undefined);
    await server.close();
  }
});
