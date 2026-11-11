import assert from "node:assert/strict";
import test from "node:test";
import { WebSocket, WebSocketServer } from "ws";

import {
  ControlLink,
  decodeControlFrame,
  generateIdentity,
  type BusinessFrame,
  type DeviceDescriptor,
  type RemoteConnection,
  type RemoteSocket,
} from "@mode/remote-core";

import { startRemoteServer } from "../src/index.js";

const HOST_ID = "019a0000-0000-7000-8000-000000000021";
const CLIENT_ID = "019a0000-0000-7000-8000-000000000022";

function connectSocket(url: string): Promise<RemoteSocket> {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url);
    ws.once("open", () => resolve(wrapWebSocket(ws)));
    ws.once("error", reject);
  });
}

function wrapWebSocket(ws: WebSocket): RemoteSocket {
  return {
    send: (text) => ws.send(text),
    close: (code, reason) => ws.close(code, reason),
    onMessage: (callback) =>
      ws.on("message", (data, isBinary) => {
        if (!isBinary) callback(data.toString());
      }),
    onClose: (callback) =>
      ws.on("close", (code, reason) => callback(code, reason.toString())),
  };
}

/** 测试用 LAN 直连适配器：首发 attach，等 relay.ready 后返回。 */
function connectDirect(
  address: string,
  connectionId: string,
  ticket: string,
): Promise<RemoteSocket> {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`ws://${address}/ws/v1/direct`);
    let ready = false;
    let onMessage: ((text: string) => void) | null = null;
    const buffered: string[] = [];
    const timer = setTimeout(() => {
      ws.terminate();
      reject(new Error("direct ready timeout"));
    }, 2_000);
    ws.on("message", (data, isBinary) => {
      if (isBinary) return;
      const text = data.toString();
      if (!ready) {
        try {
          const frame = decodeControlFrame(Buffer.from(text, "utf8"));
          if (frame.type === "relay.ready" && frame.payload.connectionId === connectionId) {
            ready = true;
            clearTimeout(timer);
            resolve({
              send: (value) => ws.send(value),
              close: (code, reason) => ws.close(code, reason),
              onMessage: (callback) => {
                onMessage = callback;
                for (const item of buffered.splice(0, buffered.length)) callback(item);
              },
              onClose: (callback) =>
                ws.on("close", (code, reason) => callback(code, reason.toString())),
            });
            return;
          }
        } catch {
          // 继续缓冲。
        }
      }
      if (onMessage) onMessage(text);
      else buffered.push(text);
    });
    ws.once("open", () => {
      ws.send(JSON.stringify({ type: "direct.attach", connectionId, ticket }));
    });
    ws.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
  });
}

async function register(
  serverUrl: string,
  descriptors: DeviceDescriptor[],
  addressesById?: Map<string, string[]>,
): Promise<Map<string, string>> {
  const login = await fetch(`${serverUrl}/api/v1/auth/github`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ accessToken: "github-runtime" }),
  });
  const loginText = await login.text();
  assert.equal(login.status, 200, loginText);
  const { sessionToken } = JSON.parse(loginText) as { sessionToken: string };
  const result = new Map<string, string>();
  for (const descriptor of descriptors) {
    const response = await fetch(`${serverUrl}/api/v1/devices`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization: `Bearer ${sessionToken}`,
      },
      body: JSON.stringify({
        device: descriptor,
        ...(addressesById?.get(descriptor.deviceId)?.length
          ? { addresses: addressesById.get(descriptor.deviceId) }
          : {}),
      }),
    });
    const text = await response.text();
    assert.equal(response.status, 201, text);
    result.set(descriptor.deviceId, (JSON.parse(text) as { accessToken: string }).accessToken);
  }
  return result;
}

test("ControlLink Host/Client 运行时完成授权、Noise 握手与加密 RPC", async () => {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "runtime-secret-that-is-long-enough",
    githubVerifier: async () => ({ providerUserId: "77", login: "runtime" }),
  });
  const hostIdentity = generateIdentity();
  const clientIdentity = generateIdentity();
  const host: DeviceDescriptor = {
    deviceId: HOST_ID,
    name: "Runtime Host",
    role: "host",
    platform: "desktop",
    identityKey: hostIdentity.identityKey,
  };
  const client: DeviceDescriptor = {
    deviceId: CLIENT_ID,
    name: "Runtime Client",
    role: "client",
    platform: "desktop",
    identityKey: clientIdentity.identityKey,
  };
  const tokens = await register(server.url, [host, client]);

  const makeFetchPeer =
    (token: string) =>
    async (deviceId: string): Promise<DeviceDescriptor> => {
      const response = await fetch(`${server.url}/api/v1/devices/${deviceId}`, {
        headers: { authorization: `Bearer ${token}` },
      });
      const text = await response.text();
      assert.equal(response.status, 200, text);
      return (JSON.parse(text) as { device: DeviceDescriptor }).device;
    };

  const hostBusiness: BusinessFrame[] = [];
  let hostConnection: RemoteConnection | null = null;
  const hostLink = new ControlLink({
    role: "host",
    serverUrl: server.url,
    deviceId: HOST_ID,
    accessToken: tokens.get(HOST_ID)!,
    identity: hostIdentity,
    connectSocket,
    fetchPeer: makeFetchPeer(tokens.get(HOST_ID)!),
    onConnection: (connection) => {
      hostConnection = connection;
      connection.onFrame((frame) => hostBusiness.push(frame));
    },
  });
  const clientLink = new ControlLink({
    role: "client",
    serverUrl: server.url,
    deviceId: CLIENT_ID,
    accessToken: tokens.get(CLIENT_ID)!,
    identity: clientIdentity,
    connectSocket,
    fetchPeer: makeFetchPeer(tokens.get(CLIENT_ID)!),
  });
  try {
    await Promise.all([hostLink.start(), clientLink.start()]);
    const connection = await clientLink.connect(HOST_ID);
    assert.equal(connection.peer.deviceId, HOST_ID);
    await waitFor(() => hostConnection !== null);
    const hostSide = hostConnection as RemoteConnection | null;
    assert.ok(hostSide);

    const clientFrames: BusinessFrame[] = [];
    connection.onFrame((frame) => clientFrames.push(frame));
    connection.send({
      v: 1,
      type: "rpc.request",
      id: "runtime-req",
      method: "remote.echo",
      params: { value: "runtime-secret" },
    });
    await waitFor(() => hostBusiness.length > 0);
    assert.equal(hostBusiness[0]?.type, "rpc.request");

    hostSide.send({ v: 1, type: "rpc.response", id: "runtime-req", result: { ok: true } });
    await waitFor(() => clientFrames.length > 0);
    assert.deepEqual(clientFrames[0], {
      v: 1,
      type: "rpc.response",
      id: "runtime-req",
      result: { ok: true },
    });
  } finally {
    clientLink.close();
    hostLink.close();
    await server.close();
  }
});

test("ControlLink 优先 LAN 直连并只把 Noise 密文放到直连 socket", async () => {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "runtime-direct-secret-long-enough",
    githubVerifier: async () => ({ providerUserId: "78", login: "runtime-direct" }),
  });
  const directServer = new WebSocketServer({ host: "127.0.0.1", port: 0 });
  await new Promise<void>((resolve, reject) => {
    directServer.once("listening", () => resolve());
    directServer.once("error", reject);
  });
  const directPort = (directServer.address() as { port: number }).port;

  const hostIdentity = generateIdentity();
  const clientIdentity = generateIdentity();
  const host: DeviceDescriptor = {
    deviceId: HOST_ID,
    name: "Direct Host",
    role: "host",
    platform: "desktop",
    identityKey: hostIdentity.identityKey,
  };
  const client: DeviceDescriptor = {
    deviceId: CLIENT_ID,
    name: "Direct Client",
    role: "client",
    platform: "desktop",
    identityKey: clientIdentity.identityKey,
  };
  const tokens = await register(
    server.url,
    [host, client],
    new Map([[HOST_ID, [`127.0.0.1:${directPort}`]]]),
  );
  const makeFetchPeer =
    (token: string) =>
    async (deviceId: string): Promise<DeviceDescriptor> => {
      const response = await fetch(`${server.url}/api/v1/devices/${deviceId}`, {
        headers: { authorization: `Bearer ${token}` },
      });
      const text = await response.text();
      assert.equal(response.status, 200, text);
      return (JSON.parse(text) as { device: DeviceDescriptor }).device;
    };

  let hostConnection: RemoteConnection | null = null;
  let directPayloadMessages = 0;
  const hostLink = new ControlLink({
    role: "host",
    serverUrl: server.url,
    deviceId: HOST_ID,
    accessToken: tokens.get(HOST_ID)!,
    identity: hostIdentity,
    connectSocket,
    fetchPeer: makeFetchPeer(tokens.get(HOST_ID)!),
    onConnection: (connection) => {
      hostConnection = connection;
    },
  });
  directServer.on("connection", (ws) => {
    let attached = false;
    ws.on("message", (data, isBinary) => {
      if (isBinary) return;
      const text = data.toString();
      if (!attached) {
        const frame = JSON.parse(text) as {
          type?: string;
          connectionId?: string;
          ticket?: string;
        };
        attached =
          frame.type === "direct.attach" &&
          typeof frame.connectionId === "string" &&
          typeof frame.ticket === "string" &&
          hostLink.acceptDirectSocket(
            frame.connectionId,
            frame.ticket,
            wrapWebSocket(ws),
          );
        if (!attached) ws.close(1008, "invalid direct attach");
        return;
      }
      directPayloadMessages += 1;
    });
  });

  const clientLink = new ControlLink({
    role: "client",
    serverUrl: server.url,
    deviceId: CLIENT_ID,
    accessToken: tokens.get(CLIENT_ID)!,
    identity: clientIdentity,
    connectSocket,
    fetchPeer: makeFetchPeer(tokens.get(CLIENT_ID)!),
    openDirectSocket: async (direct, connectionId) => {
      let lastError: unknown = null;
      for (const address of direct.addresses) {
        try {
          return await connectDirect(address, connectionId, direct.ticket);
        } catch (error) {
          lastError = error;
        }
      }
      throw lastError instanceof Error ? lastError : new Error("direct failed");
    },
  });
  try {
    await Promise.all([hostLink.start(), clientLink.start()]);
    const connection = await clientLink.connect(HOST_ID);
    await waitFor(() => hostConnection !== null);
    const hostSide = hostConnection as RemoteConnection | null;
    assert.ok(hostSide);

    const clientFrames: BusinessFrame[] = [];
    connection.onFrame((frame) => clientFrames.push(frame));
    connection.send({
      v: 1,
      type: "rpc.request",
      id: "direct-req",
      method: "remote.echo",
      params: { value: "direct-secret" },
    });
    await waitFor(() => directPayloadMessages > 0);
    hostSide.send({ v: 1, type: "rpc.response", id: "direct-req", result: { ok: true } });
    await waitFor(() => clientFrames.length > 0);
    assert.deepEqual(clientFrames[0], {
      v: 1,
      type: "rpc.response",
      id: "direct-req",
      result: { ok: true },
    });
  } finally {
    clientLink.close();
    hostLink.close();
    await new Promise<void>((resolve) => directServer.close(() => resolve()));
    await server.close();
  }
});

test("LAN 直连不可达时回落 Relay 且连接仍可用", async () => {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "runtime-direct-fallback-secret",
    githubVerifier: async () => ({ providerUserId: "79", login: "runtime-fallback" }),
  });
  const hostIdentity = generateIdentity();
  const clientIdentity = generateIdentity();
  const host: DeviceDescriptor = {
    deviceId: HOST_ID,
    name: "Fallback Host",
    role: "host",
    platform: "desktop",
    identityKey: hostIdentity.identityKey,
  };
  const client: DeviceDescriptor = {
    deviceId: CLIENT_ID,
    name: "Fallback Client",
    role: "client",
    platform: "desktop",
    identityKey: clientIdentity.identityKey,
  };
  const tokens = await register(
    server.url,
    [host, client],
    new Map([[HOST_ID, ["127.0.0.1:1"]]]),
  );
  const makeFetchPeer =
    (token: string) =>
    async (deviceId: string): Promise<DeviceDescriptor> => {
      const response = await fetch(`${server.url}/api/v1/devices/${deviceId}`, {
        headers: { authorization: `Bearer ${token}` },
      });
      const text = await response.text();
      assert.equal(response.status, 200, text);
      return (JSON.parse(text) as { device: DeviceDescriptor }).device;
    };

  let hostConnection: RemoteConnection | null = null;
  const hostLink = new ControlLink({
    role: "host",
    serverUrl: server.url,
    deviceId: HOST_ID,
    accessToken: tokens.get(HOST_ID)!,
    identity: hostIdentity,
    connectSocket,
    fetchPeer: makeFetchPeer(tokens.get(HOST_ID)!),
    onConnection: (connection) => {
      hostConnection = connection;
    },
  });
  let directAttempts = 0;
  const clientLink = new ControlLink({
    role: "client",
    serverUrl: server.url,
    deviceId: CLIENT_ID,
    accessToken: tokens.get(CLIENT_ID)!,
    identity: clientIdentity,
    connectSocket,
    fetchPeer: makeFetchPeer(tokens.get(CLIENT_ID)!),
    openDirectSocket: async (direct, connectionId) => {
      directAttempts += 1;
      return await connectDirect(direct.addresses[0]!, connectionId, direct.ticket);
    },
  });
  try {
    await Promise.all([hostLink.start(), clientLink.start()]);
    const connection = await clientLink.connect(HOST_ID);
    assert.equal(directAttempts, 1);
    await waitFor(() => hostConnection !== null);
    const hostSide = hostConnection as RemoteConnection | null;
    assert.ok(hostSide);

    const clientFrames: BusinessFrame[] = [];
    connection.onFrame((frame) => clientFrames.push(frame));
    connection.send({
      v: 1,
      type: "rpc.request",
      id: "fallback-req",
      method: "remote.echo",
      params: { value: "relay" },
    });
    hostSide.send({ v: 1, type: "rpc.response", id: "fallback-req", result: { ok: true } });
    await waitFor(() => clientFrames.length > 0);
    assert.deepEqual(clientFrames[0], {
      v: 1,
      type: "rpc.response",
      id: "fallback-req",
      result: { ok: true },
    });
  } finally {
    clientLink.close();
    hostLink.close();
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
