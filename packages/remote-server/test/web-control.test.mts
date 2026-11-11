import assert from "node:assert/strict";
import test from "node:test";
import { WebSocket } from "ws";

import {
  ControlLink,
  generateIdentity,
  type BusinessFrame,
  type DeviceDescriptor,
  type RemoteConnection,
  type RemoteSocket,
} from "@mode/remote-core";
import { WebControlLink, generateWebIdentity } from "@mode/remote-core/web";

import { startRemoteServer } from "../src/index.js";

const HOST_ID = "019a0000-0000-7000-8000-0000000000a1";
const CLIENT_ID = "019a0000-0000-7000-8000-0000000000a2";

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
      onClose: (callback) =>
        ws.on("close", (code, reason) => callback(code, reason.toString())),
    };
    ws.once("open", () => resolve(socket));
    ws.once("error", reject);
  });
}

async function register(
  serverUrl: string,
  descriptors: DeviceDescriptor[],
): Promise<Map<string, string>> {
  const login = await fetch(`${serverUrl}/api/v1/auth/github`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ accessToken: "web-control-github" }),
  });
  const loginText = await login.text();
  assert.equal(login.status, 200, loginText);
  const { sessionToken } = JSON.parse(loginText) as { sessionToken: string };
  const tokens = new Map<string, string>();
  for (const descriptor of descriptors) {
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
    tokens.set(descriptor.deviceId, (JSON.parse(text) as { accessToken: string }).accessToken);
  }
  return tokens;
}

async function waitFor(predicate: () => boolean, timeoutMs = 3_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error("等待 Web 客户端 RPC 超时");
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

test("浏览器 WebControlLink 通过 Relay 与 Node Host 完成 Noise RPC", async () => {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "web-control-secret-long-enough",
    githubVerifier: async () => ({ providerUserId: "web", login: "web" }),
  });
  const hostIdentity = generateIdentity();
  const webIdentity = generateWebIdentity();
  const host: DeviceDescriptor = {
    deviceId: HOST_ID,
    name: "Web Host",
    role: "host",
    platform: "desktop",
    identityKey: hostIdentity.identityKey,
  };
  const client: DeviceDescriptor = {
    deviceId: CLIENT_ID,
    name: "Web Client",
    role: "client",
    platform: "web",
    identityKey: webIdentity.identityKey,
  };
  const tokens = await register(server.url, [host, client]);
  const fetchPeer =
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
  const handleHostConnection = (connection: RemoteConnection): void => {
    connection.onFrame((frame) => {
      if (frame.type !== "rpc.request") return;
      connection.send({
        v: 1,
        type: "rpc.response",
        id: frame.id,
        result: { ok: true, value: frame.params.value },
      });
    });
    hostConnection = connection;
  };
  const createHostLink = () =>
    new ControlLink({
      role: "host",
      serverUrl: server.url,
      deviceId: HOST_ID,
      accessToken: tokens.get(HOST_ID)!,
      identity: hostIdentity,
      connectSocket,
      fetchPeer: fetchPeer(tokens.get(HOST_ID)!),
      onConnection: handleHostConnection,
    });
  let hostLink = createHostLink();
  const webLink = new WebControlLink({
    serverUrl: server.url,
    deviceId: CLIENT_ID,
    accessToken: tokens.get(CLIENT_ID)!,
    identity: webIdentity,
    reconnectBaseMs: 100,
  });
  try {
    await Promise.all([hostLink.start(), webLink.start()]);
    const connection = await webLink.connect(HOST_ID);
    await waitFor(() => hostConnection !== null);
    const frames: BusinessFrame[] = [];
    connection.onFrame((frame) => frames.push(frame));
    connection.send({
      v: 1,
      type: "rpc.request",
      id: "web-req",
      method: "remote.echo",
      params: { value: "from-browser" },
    });
    await waitFor(() => frames.some((frame) => frame.type === "rpc.response" && frame.id === "web-req"));
    assert.deepEqual(frames[0], {
      v: 1,
      type: "rpc.response",
      id: "web-req",
      result: { ok: true, value: "from-browser" },
    });

    // Host 退出后浏览器自动重连；Host 恢复后同一 connection 对象继续可用。
    hostLink.close();
    hostConnection = null;
    await new Promise((resolve) => setTimeout(resolve, 150));
    hostLink = createHostLink();
    await hostLink.start();
    await waitFor(() => hostConnection !== null, 5_000);
    connection.send({
      v: 1,
      type: "rpc.request",
      id: "web-req-2",
      method: "remote.echo",
      params: { value: "after-reconnect" },
    });
    await waitFor(
      () => frames.some((frame) => frame.type === "rpc.response" && frame.id === "web-req-2"),
      5_000,
    );
    const second = frames.find((frame) => frame.type === "rpc.response" && frame.id === "web-req-2");
    assert.deepEqual(second, {
      v: 1,
      type: "rpc.response",
      id: "web-req-2",
      result: { ok: true, value: "after-reconnect" },
    });
  } finally {
    webLink.close();
    hostLink.close();
    await server.close();
  }
});
