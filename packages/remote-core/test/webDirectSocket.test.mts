/**
 * 浏览器 LAN 直连：地址构造、direct.attach 首帧与 relay.ready 交握、失败回落。
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  buildWebDirectSocketUrl,
  openWebDirectSocket,
} from "../src/web/directSocket.js";
import type { WebSocketLike } from "../src/web/transport.js";

class FakeSocket implements WebSocketLike {
  readyState = 1;
  readonly sent: string[] = [];
  private readonly messageListeners: Array<(event: { data?: unknown }) => void> = [];
  private readonly closeListeners: Array<(event: { code?: number }) => void> = [];

  constructor(private readonly connectionId: string) {}

  send(data: string): void {
    this.sent.push(data);
    if (data.includes("direct.attach")) {
      queueMicrotask(() => this.emitMessage(this.readyFrame()));
    }
  }

  close(): void {
    this.readyState = 3;
    for (const listener of this.closeListeners) listener({ code: 1000 });
  }

  addEventListener(type: string, listener: (event: never) => void): void {
    if (type === "message") this.messageListeners.push(listener as never);
    if (type === "close") this.closeListeners.push(listener as never);
  }

  emitMessage(text: string): void {
    for (const listener of this.messageListeners) listener({ data: text });
  }

  private readyFrame(): string {
    return JSON.stringify({
      v: 1,
      id: "ready-1",
      timestamp: Date.now(),
      type: "relay.ready",
      payload: { connectionId: this.connectionId },
    });
  }
}

test("buildWebDirectSocketUrl 保留 wss 并补齐 ws", () => {
  assert.equal(
    buildWebDirectSocketUrl("192.168.1.2:45880"),
    "ws://192.168.1.2:45880/ws/v1/direct",
  );
  assert.equal(
    buildWebDirectSocketUrl("wss://host.lan:45881/"),
    "wss://host.lan:45881/ws/v1/direct",
  );
});

test("直连首帧 direct.attach，收到 relay.ready 后返回可写 socket", async () => {
  const fake = new FakeSocket("conn-1");
  const socket = await openWebDirectSocket({
    direct: { ticket: "ticket-1", addresses: ["192.168.1.2:45880"] },
    connectionId: "conn-1",
    createSocket: async () => fake,
  });
  assert.ok(socket);
  assert.deepEqual(JSON.parse(fake.sent[0]!), {
    type: "direct.attach",
    connectionId: "conn-1",
    ticket: "ticket-1",
  });

  const frames: string[] = [];
  socket.onMessage((text) => frames.push(text));
  fake.emitMessage('{"v":1,"type":"relay"}');
  assert.deepEqual(frames, ['{"v":1,"type":"relay"}']);
  socket.send("hello");
  assert.equal(fake.sent.at(-1), "hello");
});

test("首个地址失败后继续尝试下一个；全部失败返回 undefined", async () => {
  const calls: string[] = [];
  const fake = new FakeSocket("conn-2");
  const socket = await openWebDirectSocket({
    direct: { ticket: "ticket-2", addresses: ["bad.host:1", "192.168.1.3:45880"] },
    connectionId: "conn-2",
    createSocket: async (url) => {
      calls.push(url);
      if (url.includes("bad.host")) throw new Error("refused");
      return fake;
    },
  });
  assert.ok(socket);
  assert.deepEqual(calls, ["ws://bad.host:1/ws/v1/direct", "ws://192.168.1.3:45880/ws/v1/direct"]);

  const failed = await openWebDirectSocket({
    direct: { ticket: "ticket-3", addresses: ["192.168.1.4:1"] },
    connectionId: "conn-3",
    createSocket: async () => {
      throw new Error("refused");
    },
  });
  assert.equal(failed, undefined);
});

test("HTTPS 页面跳过明文 ws 地址", async () => {
  const original = (globalThis as { location?: unknown }).location;
  (globalThis as { location?: unknown }).location = { protocol: "https:" };
  try {
    let opened = false;
    const socket = await openWebDirectSocket({
      direct: { ticket: "ticket-4", addresses: ["192.168.1.5:45880"] },
      connectionId: "conn-4",
      createSocket: async () => {
        opened = true;
        return new FakeSocket("conn-4");
      },
    });
    assert.equal(socket, undefined);
    assert.equal(opened, false);
  } finally {
    (globalThis as { location?: unknown }).location = original;
  }
});
