/**
 * WebRTC/P2P 第一阶段：ICE 下发与 p2p.* 信令转发（docs/specs/remote-control-v3.md）。
 *
 * 用裸 Control WS 验证：
 * 1. connect.accepted/incoming 携带 ICE 服务器（含 coturn 临时凭证）；
 * 2. p2p.offer/candidate 在连接双方之间转发，错误 connectionId 被丢弃。
 */
import assert from "node:assert/strict";
import { createHmac, randomUUID } from "node:crypto";
import test from "node:test";
import { WebSocket } from "ws";

import {
  decodeControlFrame,
  encodeControlFrame,
  generateIdentity,
  type ControlFrame,
  type DeviceDescriptor,
} from "@mode/remote-core";

import { buildTurnCredentials } from "../src/webrtc.js";
import { startRemoteServer } from "../src/index.js";

const HOST_ID = "019a0000-0000-7000-8000-0000000000c1";
const CLIENT_ID = "019a0000-0000-7000-8000-0000000000c2";

function frame<T extends ControlFrame["type"]>(
  type: T,
  payload: Extract<ControlFrame, { type: T }>["payload"],
): ControlFrame {
  return { v: 1, id: randomUUID(), timestamp: Date.now(), type, payload } as ControlFrame;
}

class Wire {
  readonly ws: WebSocket;
  private readonly frames: ControlFrame[] = [];
  private readonly waiters: Array<{
    predicate: (frame: ControlFrame) => boolean;
    resolve: (frame: ControlFrame) => void;
    reject: (error: Error) => void;
    timer: NodeJS.Timeout;
  }> = [];

  constructor(url: string) {
    this.ws = new WebSocket(url);
    this.ws.on("message", (data, isBinary) => {
      if (isBinary) return;
      const bytes = Buffer.isBuffer(data) ? data : Buffer.from(data as ArrayBuffer);
      const decoded = decodeControlFrame(bytes);
      const index = this.waiters.findIndex((waiter) => waiter.predicate(decoded));
      if (index >= 0) {
        const [waiter] = this.waiters.splice(index, 1);
        clearTimeout(waiter!.timer);
        waiter!.resolve(decoded);
      } else {
        this.frames.push(decoded);
      }
    });
  }

  async open(): Promise<void> {
    await new Promise<void>((resolve, reject) => {
      this.ws.once("open", () => resolve());
      this.ws.once("error", reject);
    });
  }

  send(value: ControlFrame): void {
    this.ws.send(Buffer.from(encodeControlFrame(value)).toString("utf8"));
  }

  next(predicate: (frame: ControlFrame) => boolean, timeoutMs = 2000): Promise<ControlFrame> {
    const index = this.frames.findIndex(predicate);
    if (index >= 0) return Promise.resolve(this.frames.splice(index, 1)[0]!);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        const waiterIndex = this.waiters.findIndex((waiter) => waiter.timer === timer);
        if (waiterIndex >= 0) this.waiters.splice(waiterIndex, 1);
        reject(new Error("等待 control frame 超时"));
      }, timeoutMs);
      this.waiters.push({ predicate, resolve, reject, timer });
    });
  }

  received(predicate: (frame: ControlFrame) => boolean): boolean {
    return this.frames.some(predicate);
  }

  close(): void {
    for (const waiter of this.waiters.splice(0)) {
      clearTimeout(waiter.timer);
      waiter.reject(new Error("wire closed"));
    }
    this.ws.close();
  }
}

test("coturn 临时凭证：username 为过期时间戳与设备，credential 为 HMAC-SHA1", () => {
  const now = 1_700_000_000;
  const { username, credential } = buildTurnCredentials("turn-secret", 600, "device-1", now);
  assert.equal(username, `${now + 600}:device-1`);
  const expected = createHmac("sha1", "turn-secret").update(username).digest("base64");
  assert.equal(credential, expected);
});

test("connect 下发 ICE 服务器，p2p 信令在双方之间转发", async () => {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "p2p-signaling-secret-long-enough",
    githubVerifier: async () => ({ providerUserId: "p2p", login: "p2p" }),
    webrtc: {
      stunUrls: ["stun:stun.example:3478"],
      turn: {
        urls: ["turn:turn.example:3478?transport=udp"],
        secret: "turn-secret",
        ttlSeconds: 600,
      },
    },
  });
  const hostIdentity = generateIdentity();
  const clientIdentity = generateIdentity();
  const hostDesc: DeviceDescriptor = {
    deviceId: HOST_ID,
    name: "P2P Host",
    role: "host",
    platform: "desktop",
    identityKey: hostIdentity.identityKey,
  };
  const clientDesc: DeviceDescriptor = {
    deviceId: CLIENT_ID,
    name: "P2P Client",
    role: "client",
    platform: "desktop",
    identityKey: clientIdentity.identityKey,
  };
  const login = await fetch(`${server.url}/api/v1/auth/github`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ accessToken: "github-p2p" }),
  });
  const { sessionToken } = (await login.json()) as { sessionToken: string };
  const tokens = new Map<string, string>();
  for (const device of [hostDesc, clientDesc]) {
    const response = await fetch(`${server.url}/api/v1/devices`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization: `Bearer ${sessionToken}`,
      },
      body: JSON.stringify({ device }),
    });
    const body = (await response.json()) as { accessToken: string };
    assert.equal(response.status, 201);
    tokens.set(device.deviceId, body.accessToken);
  }

  const hostWire = new Wire(`ws://127.0.0.1:${server.port}/ws/v1/connect`);
  const clientWire = new Wire(`ws://127.0.0.1:${server.port}/ws/v1/connect`);
  try {
    await Promise.all([hostWire.open(), clientWire.open()]);
    hostWire.send(
      frame("hello", {
        role: "host",
        deviceId: HOST_ID,
        accessToken: tokens.get(HOST_ID)!,
        protocols: [1],
        capabilities: ["secure.noise-ik.v1", "transport.relay", "transport.p2p"],
      }),
    );
    clientWire.send(
      frame("hello", {
        role: "client",
        deviceId: CLIENT_ID,
        accessToken: tokens.get(CLIENT_ID)!,
        protocols: [1],
        capabilities: ["secure.noise-ik.v1", "transport.relay", "transport.p2p"],
      }),
    );
    await Promise.all([
      hostWire.next((value) => value.type === "hello.ack"),
      clientWire.next((value) => value.type === "hello.ack"),
    ]);

    clientWire.send(
      frame("connect.request", { hostDeviceId: HOST_ID, preferredTransports: ["p2p", "relay"] }),
    );
    const incoming = await hostWire.next((value) => value.type === "connect.incoming");
    const accepted = await clientWire.next((value) => value.type === "connect.accepted");
    assert.equal(
      accepted.type === "connect.accepted" ? accepted.payload.p2pPreferred : undefined,
      true,
      "Host 宣告 transport.p2p 后，accept 必须确认 p2pPreferred",
    );
    const connectionId = incoming.type === "connect.incoming" ? incoming.payload.connectionId : "";
    assert.equal(
      connectionId,
      accepted.type === "connect.accepted" ? accepted.payload.connectionId : "",
    );

    const assertIce = (
      payload: import("@mode/remote-core").IceServerPayload[] | undefined,
      label: string,
    ) => {
      assert.ok(payload && payload.length >= 2, `${label}: ${JSON.stringify(payload)}`);
      const stun = payload.find((server) => server.urls[0] === "stun:stun.example:3478");
      assert.ok(stun && !stun.username, label);
      const turn = payload.find((server) => server.urls[0]?.startsWith("turn:"));
      assert.ok(turn?.username && turn.credential, label);
      assert.match(turn.username, /^\d+:/u, label);
      const expected = createHmac("sha1", "turn-secret").update(turn.username).digest("base64");
      assert.equal(turn.credential, expected, label);
    };
    assertIce(
      incoming.type === "connect.incoming" ? incoming.payload.iceServers : undefined,
      "host",
    );
    assertIce(
      accepted.type === "connect.accepted" ? accepted.payload.iceServers : undefined,
      "client",
    );

    clientWire.send(frame("p2p.offer", { connectionId, sdp: "offer-sdp" }));
    const offer = await hostWire.next(
      (value) => value.type === "p2p.offer" && value.payload.sdp === "offer-sdp",
    );
    assert.ok(offer.type === "p2p.offer");

    hostWire.send(frame("p2p.candidate", { connectionId, candidate: "candidate-1" }));
    const candidate = await clientWire.next(
      (value) => value.type === "p2p.candidate" && value.payload.candidate === "candidate-1",
    );
    assert.ok(candidate.type === "p2p.candidate");

    const before = clientWire.received(() => true);
    clientWire.send(frame("p2p.answer", { connectionId: "bogus", sdp: "x" }));
    await new Promise((resolve) => setTimeout(resolve, 400));
    assert.equal(
      clientWire.received(() => true),
      before,
      "错误 connectionId 的信令不得转发",
    );
    void hostWire;
  } finally {
    hostWire.close();
    clientWire.close();
    await server.close();
  }
});

test("Host 未宣告 transport.p2p 时不下发 p2pPreferred，避免无谓协商超时", async () => {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "p2p-gating-secret-long-enough",
    githubVerifier: async () => ({ providerUserId: "gating", login: "gating" }),
  });
  const hostDesc: DeviceDescriptor = {
    deviceId: HOST_ID,
    name: "Relay Host",
    role: "host",
    platform: "desktop",
    identityKey: generateIdentity().identityKey,
  };
  const clientDesc: DeviceDescriptor = {
    deviceId: CLIENT_ID,
    name: "P2P Client",
    role: "client",
    platform: "desktop",
    identityKey: generateIdentity().identityKey,
  };
  const login = await fetch(`${server.url}/api/v1/auth/github`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ accessToken: "github-gating" }),
  });
  const { sessionToken } = (await login.json()) as { sessionToken: string };
  const tokens = new Map<string, string>();
  for (const device of [hostDesc, clientDesc]) {
    const response = await fetch(`${server.url}/api/v1/devices`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${sessionToken}` },
      body: JSON.stringify({ device }),
    });
    const body = (await response.json()) as { accessToken: string };
    assert.equal(response.status, 201);
    tokens.set(device.deviceId, body.accessToken);
  }

  const hostWire = new Wire(`ws://127.0.0.1:${server.port}/ws/v1/connect`);
  const clientWire = new Wire(`ws://127.0.0.1:${server.port}/ws/v1/connect`);
  try {
    await Promise.all([hostWire.open(), clientWire.open()]);
    hostWire.send(
      frame("hello", {
        role: "host",
        deviceId: HOST_ID,
        accessToken: tokens.get(HOST_ID)!,
        protocols: [1],
        capabilities: ["secure.noise-ik.v1", "transport.relay"],
      }),
    );
    clientWire.send(
      frame("hello", {
        role: "client",
        deviceId: CLIENT_ID,
        accessToken: tokens.get(CLIENT_ID)!,
        protocols: [1],
        capabilities: ["secure.noise-ik.v1", "transport.relay", "transport.p2p"],
      }),
    );
    await Promise.all([
      hostWire.next((value) => value.type === "hello.ack"),
      clientWire.next((value) => value.type === "hello.ack"),
    ]);

    clientWire.send(
      frame("connect.request", { hostDeviceId: HOST_ID, preferredTransports: ["p2p", "relay"] }),
    );
    const incoming = await hostWire.next((value) => value.type === "connect.incoming");
    const accepted = await clientWire.next((value) => value.type === "connect.accepted");
    assert.equal(
      accepted.type === "connect.accepted" ? accepted.payload.p2pPreferred : undefined,
      undefined,
    );
    assert.equal(
      incoming.type === "connect.incoming" ? incoming.payload.p2pPreferred : undefined,
      undefined,
    );
  } finally {
    hostWire.close();
    clientWire.close();
    await server.close();
  }
});
