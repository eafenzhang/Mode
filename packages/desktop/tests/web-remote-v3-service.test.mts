/**
 * 浏览器 Client 角色的 `IRemoteV3Service`：设备目录、RPC、隧道与状态收口。
 *
 * 覆盖 docs/specs/remote-control-v3-web-workspace.md 的接入前提：
 * 1. 向导只读 `listHosts()`，映射不能丢 identityKey/online；
 * 2. RPC/HTTPS 桥/隧道共用同一 Host 连接，不重复建链；
 * 3. Host 管理类操作在浏览器明确拒绝，不伪造成功。
 */
import assert from "node:assert/strict";
import test from "node:test";

import { createWebRemoteV3Service } from "@mode/client";
import { encodeBase64Url, type BusinessFrame } from "@mode/remote-core";
import {
  exportWebIdentity,
  generateWebIdentity,
  type WebAccountClient,
  type WebControlLink,
  type WebDeviceProfile,
  type WebDeviceSummary,
} from "@mode/remote-core/web";

const PROFILE: WebDeviceProfile = {
  serverUrl: "https://remote.example",
  deviceId: "client-1",
  deviceName: "Mode Web",
  privateKey: exportWebIdentity(generateWebIdentity()),
  accessToken: "access-1",
  refreshToken: "refresh-1",
};

const DEVICES: WebDeviceSummary[] = [
  {
    deviceId: "host-1",
    name: "Desk",
    role: "host",
    platform: "desktop",
    identityKey: "pk-host-1",
    online: true,
    addresses: ["192.168.1.5:45880"],
  },
  {
    deviceId: "peer-1",
    name: "Other Web",
    role: "client",
    platform: "web",
    identityKey: "pk-peer-1",
    online: false,
    addresses: [],
  },
];

interface FakeStream {
  service: string;
  metadata: Record<string, unknown>;
  sent: Uint8Array[];
  closed: number;
  emit(bytes: Uint8Array): void;
  emitClose(reason?: string): void;
}

function createFakeStream(): FakeStream {
  const data = new Set<(bytes: Uint8Array) => void>();
  const close = new Set<(reason: string) => void>();
  const stream: FakeStream = {
    service: "",
    metadata: {},
    sent: [],
    closed: 0,
    send(bytes) {
      stream.sent.push(bytes);
    },
    close(reason = "closed") {
      stream.closed += 1;
      for (const listener of close) listener(reason);
    },
    onData(callback) {
      data.add(callback);
      return () => data.delete(callback);
    },
    onClose(callback) {
      close.add(callback);
      return () => close.delete(callback);
    },
    emit(bytes) {
      for (const listener of data) listener(bytes);
    },
    emitClose(reason = "peer closed") {
      for (const listener of close) listener(reason);
    },
  };
  return stream;
}

interface FakeLink {
  started: number;
  closed: number;
  connectedHost: string | null;
  requests: { method: string; params: Record<string, unknown> }[];
  sent: BusinessFrame[];
  streams: FakeStream[];
  emitFrame(frame: BusinessFrame): void;
  connectionClosed: boolean;
}

function createFakeLink(): { link: WebControlLink; state: FakeLink } {
  const frameListeners = new Set<(frame: BusinessFrame) => void>();
  const closeListeners = new Set<() => void>();
  const state: FakeLink = {
    started: 0,
    closed: 0,
    connectedHost: null,
    requests: [],
    sent: [],
    streams: [],
    connectionClosed: false,
    emitFrame(frame) {
      for (const listener of frameListeners) listener(frame);
    },
  };
  const connection = {
    send(frame: BusinessFrame) {
      state.sent.push(frame);
    },
    onFrame(callback: (frame: BusinessFrame) => void) {
      frameListeners.add(callback);
      return () => frameListeners.delete(callback);
    },
    onClose(callback: () => void) {
      closeListeners.add(callback);
      return () => closeListeners.delete(callback);
    },
    close() {
      state.connectionClosed = true;
      for (const listener of closeListeners) listener();
    },
  };
  const link = {
    async start() {
      state.started += 1;
    },
    async connect(hostDeviceId: string) {
      state.connectedHost = hostDeviceId;
      return connection;
    },
    async request(method: string, params: Record<string, unknown>) {
      state.requests.push({ method, params });
      return { status: 200, headers: {}, bodyBase64: "" };
    },
    openStream(service: string, metadata: Record<string, unknown>) {
      const stream = createFakeStream();
      stream.service = service;
      stream.metadata = metadata;
      state.streams.push(stream);
      return stream;
    },
    close() {
      state.closed += 1;
    },
  } as unknown as WebControlLink;
  return { link, state };
}

function createFakeAccount(devices: WebDeviceSummary[]): WebAccountClient {
  return {
    async listHosts() {
      return devices;
    },
    async logout() {
      return undefined;
    },
  } as unknown as WebAccountClient;
}

async function waitFor(predicate: () => boolean, what: string): Promise<void> {
  const deadline = Date.now() + 2_000;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(`等待 ${what} 超时`);
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}

test("listHosts 映射目录并只返回 Host", async () => {
  const { link } = createFakeLink();
  const service = createWebRemoteV3Service({
    store: { get: async () => null, set: async () => undefined, delete: async () => undefined },
    loadProfile: async () => PROFILE,
    createAccountClient: () => createFakeAccount(DEVICES),
    createLink: () => link,
  });
  assert.deepEqual(await service.listHosts(), [
    {
      deviceId: "host-1",
      name: "Desk",
      role: "host",
      platform: "desktop",
      identityKey: "pk-host-1",
      online: true,
      addresses: ["192.168.1.5:45880"],
    },
  ]);
  assert.equal((await service.listDevices()).length, 2);
  assert.equal((await service.getState()).status, "online");
});

test("request 复用同一连接并按 id 匹配响应", async () => {
  const { link, state } = createFakeLink();
  const service = createWebRemoteV3Service({
    store: { get: async () => null, set: async () => undefined, delete: async () => undefined },
    loadProfile: async () => PROFILE,
    createAccountClient: () => createFakeAccount(DEVICES),
    createLink: () => link,
  });

  const first = service.request({ hostDeviceId: "host-1", method: "remote.host.info" });
  await waitFor(() => state.sent.length === 1, "首条帧");
  assert.equal(state.connectedHost, "host-1");
  const frame = state.sent[0]!;
  assert.equal(frame.type, "rpc.request");
  state.emitFrame({ v: 1, type: "rpc.response", id: frame.id, result: { ok: true } });
  assert.deepEqual(await first, { result: { ok: true } });

  const second = service.request({ hostDeviceId: "host-1", method: "remote.workspace.list" });
  await waitFor(() => state.sent.length === 2, "第二条帧");
  state.emitFrame({ v: 1, type: "rpc.response", id: state.sent[1]!.id, result: [] });
  await second;
  assert.equal(state.started, 1, "同一 Host 只建一次连接");
});

test("隧道事件封装 base64url，send/close 走同一 stream", async () => {
  const { link, state } = createFakeLink();
  const events: { kind: string; tunnelId: string; data?: string; reason?: string }[] = [];
  const service = createWebRemoteV3Service({
    store: { get: async () => null, set: async () => undefined, delete: async () => undefined },
    loadProfile: async () => PROFILE,
    createAccountClient: () => createFakeAccount(DEVICES),
    createLink: () => link,
  });
  service.onTunnelEvent((event) => events.push(event));

  const { tunnelId } = await service.openTunnel({ hostDeviceId: "host-1", path: "/ws/host" });
  assert.deepEqual(events[0], { tunnelId, kind: "open" });
  const stream = state.streams[0]!;
  assert.deepEqual(stream.metadata, { path: "/ws/host" });

  stream.emit(new Uint8Array([1, 2, 3]));
  assert.equal(events[1]!.kind, "message");
  assert.deepEqual(
    Buffer.from(events[1]!.data!, "base64url"),
    Buffer.from([1, 2, 3]),
  );

  await service.sendTunnel(tunnelId, encodeBase64Url(new Uint8Array([9, 8])));
  assert.deepEqual([...stream.sent[0]!], [9, 8]);

  await service.closeTunnel(tunnelId);
  assert.equal(stream.closed, 1);
});

test("Device Flow 轮询瞬时失败后继续等待，不重开授权", async () => {
  const { link } = createFakeLink();
  let polls = 0;
  const errors: string[] = [];
  const account = {
    async beginGithubDeviceLogin() {
      return {
        deviceCode: "device-code",
        userCode: "ABCD-1234",
        verificationUri: "https://github.example/device",
        expiresIn: 60,
        interval: 0,
      };
    },
    async pollGithubDeviceLogin() {
      polls += 1;
      if (polls === 1) throw new Error("network down");
      return { status: "ok", profile: PROFILE };
    },
    async listHosts() {
      return DEVICES;
    },
    async logout() {
      return undefined;
    },
  } as unknown as WebAccountClient;
  const service = createWebRemoteV3Service({
    store: { get: async () => null, set: async () => undefined, delete: async () => undefined },
    loadProfile: async () => null,
    createAccountClient: () => account,
    createLoginClient: () => account,
    createLink: () => link,
  });
  service.onDidChange((state) => {
    if (state.error) errors.push(state.error);
  });

  const challenge = await service.beginGithubLogin({ serverUrl: "https://remote.example" });
  assert.equal(challenge.userCode, "ABCD-1234");
  await waitFor(() => polls >= 2, "第二次轮询");
  for (let attempt = 0; attempt < 200; attempt += 1) {
    if ((await service.getState()).status === "online") break;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.deepEqual(errors, ["network down"], "失败只记录一次，随后恢复");
  assert.equal((await service.getState()).status, "online");
});

test("Host 管理类操作在浏览器明确拒绝", async () => {
  const { link } = createFakeLink();
  const service = createWebRemoteV3Service({
    store: { get: async () => null, set: async () => undefined, delete: async () => undefined },
    loadProfile: async () => PROFILE,
    createAccountClient: () => createFakeAccount(DEVICES),
    createLink: () => link,
  });
  await assert.rejects(service.resetPeerPin({ deviceId: "host-1" }), /peer pin/u);
  await assert.rejects(
    service.startRemoteDesktop({ hostDeviceId: "host-1" }),
    /\/v3/u,
  );
});

test("disconnectAccount 关闭连接并回到未配置", async () => {
  const { link, state } = createFakeLink();
  const service = createWebRemoteV3Service({
    store: { get: async () => null, set: async () => undefined, delete: async () => undefined },
    loadProfile: async () => PROFILE,
    createAccountClient: () => createFakeAccount(DEVICES),
    createLink: () => link,
  });
  await service.openTunnel({ hostDeviceId: "host-1", path: "/ws/host" });
  const next = await service.disconnectAccount();
  assert.equal(next.status, "unconfigured");
  assert.equal(state.closed, 1, "disconnect 必须关闭 Control 链路");
});

test("浏览器配对：claim/redeem/cancel 委托账号客户端并刷新状态", async () => {
  const { link } = createFakeLink();
  const calls: string[] = [];
  let redeemed = false;
  let profile = PROFILE;
  const account = {
    async claimPairing(ticket: string) {
      calls.push(`claim:${ticket}`);
      return { pairingId: "p-1", verificationCode: "01234567", hostName: "Desk", expiresAt: 1 };
    },
    async getPairing() {
      return {
        status: redeemed ? "confirmed" : "claimed",
        expiresAt: 1,
        hostName: "Desk",
        clientName: "Web",
      };
    },
    async redeemPairing() {
      calls.push("redeem");
      redeemed = true;
      profile = { ...PROFILE, accessToken: "access-2", refreshToken: "refresh-2" };
      return {
        accessToken: "access-2",
        refreshToken: "refresh-2",
        accessExpiresAt: 1,
        refreshExpiresAt: 2,
      };
    },
    async cancelPairing() {
      calls.push("cancel");
    },
    async listHosts() {
      return DEVICES;
    },
    async logout() {
      return undefined;
    },
  } as unknown as WebAccountClient;
  const service = createWebRemoteV3Service({
    store: { get: async () => null, set: async () => undefined, delete: async () => undefined },
    loadProfile: async () => profile,
    createAccountClient: () => account,
    createLink: () => link,
  });

  assert.deepEqual(await service.claimPairing!("ticket-1"), {
    pairingId: "p-1",
    verificationCode: "01234567",
    hostName: "Desk",
    expiresAt: 1,
  });
  assert.equal((await service.getPairing!("p-1")).status, "claimed");
  assert.equal((await service.redeemPairing!("p-1")).status, "confirmed");
  assert.equal((await service.getState()).status, "online");
  assert.deepEqual(await service.cancelPairing!("p-1"), {
    status: "confirmed",
    expiresAt: 1,
    hostName: "Desk",
    clientName: "Web",
  });
  assert.deepEqual(calls, ["claim:ticket-1", "redeem", "cancel"]);
});
