import assert from "node:assert/strict";
import test from "node:test";

import { generateIdentity } from "../src/adapters/identity.js";
import { ControlLink } from "../src/app/controlLink.js";
import type { RemoteSocket } from "../src/app/controlTypes.js";
import { encodeControlFrame, type ControlFrame } from "../src/domain/controlFrames.js";
import type { DeviceDescriptor } from "../src/domain/descriptor.js";

class FakeSocket implements RemoteSocket {
  readonly sent: string[] = [];
  closeCode: number | null = null;
  closeReason = "";
  private messageHandler: ((text: string) => void) | null = null;
  private closeHandler: ((code: number, reason: string) => void) | null = null;

  send(text: string): void {
    this.sent.push(text);
  }

  close(code = 1000, reason = ""): void {
    this.closeCode = code;
    this.closeReason = reason;
    this.closeHandler?.(code, reason);
  }

  onMessage(callback: (text: string) => void): void {
    this.messageHandler = callback;
  }

  onClose(callback: (code: number, reason: string) => void): void {
    this.closeHandler = callback;
  }

  emit(frame: ControlFrame): void {
    this.messageHandler?.(Buffer.from(encodeControlFrame(frame)).toString("utf8"));
  }
}

function frame(type: ControlFrame["type"], payload: unknown): ControlFrame {
  return {
    v: 1,
    id: `frame-${type}`,
    timestamp: Date.now(),
    type,
    payload,
  } as ControlFrame;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

test("ControlLink 连续一个心跳周期没有 pong 时主动断开并回调 onClose", async () => {
  const identity = generateIdentity();
  const peer: DeviceDescriptor = {
    deviceId: "peer-1",
    name: "peer",
    role: "client",
    platform: "desktop",
    identityKey: identity.identityKey,
  };
  const socket = new FakeSocket();
  const closes: Array<{ code: number; reason: string }> = [];
  const link = new ControlLink({
    role: "host",
    serverUrl: "http://127.0.0.1:1",
    deviceId: "host-1",
    accessToken: "token",
    identity,
    connectSocket: async () => socket,
    fetchPeer: async () => peer,
    onClose: (details) => closes.push(details),
  });

  const start = link.start();
  await sleep(0);
  socket.emit(
    frame("hello.ack", {
      protocol: 1,
      connectionSessionId: "session-1",
      heartbeatIntervalMs: 20,
      maxControlFrameBytes: 64 * 1024,
      maxRelayFrameBytes: 1024 * 1024,
      capabilities: ["transport.relay"],
    }),
  );
  await start;

  // 第一个周期发 ping；不回应 pong，第二个周期应判定半开并 close(4001)。
  await sleep(30);
  const ping = socket.sent
    .map((text) => JSON.parse(text) as ControlFrame)
    .find((item) => item.type === "ping");
  assert.ok(ping, "心跳周期内没有发出 ping");

  await sleep(30);
  assert.equal(socket.closeCode, 4001);
  assert.equal(closes.at(-1)?.code, 4001);
  assert.match(closes.at(-1)?.reason ?? "", /heartbeat timeout/);
  link.close();
});
