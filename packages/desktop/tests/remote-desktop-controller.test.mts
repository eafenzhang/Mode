/**
 * LR-3：Host 主控 controller 的状态机（真 ws server，不碰外网）。
 *
 * 覆盖 Electron 线 controllerSession 的同一套语义：
 * 单会话互斥、connecting→connected→closed、对端信令透传、bye/error 收口、
 * 传输断开带 retryable、本地 end 先发 bye、dispose 收口。
 */
import assert from "node:assert/strict";
import test from "node:test";
import type { AddressInfo } from "node:net";

import { WebSocket, WebSocketServer } from "ws";

import type { RemoteDesktopSessionEvent } from "@mode/shared/remote-desktop";
import { createRemoteDesktopHostController } from "../src/host/remoteDesktopController.js";

const delay = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

interface FakePeer {
  url: string;
  sockets: Set<WebSocket>;
  received: string[];
  close: () => Promise<void>;
}

async function startFakePeer(): Promise<FakePeer> {
  const wss = new WebSocketServer({ port: 0 });
  await new Promise<void>((resolve) => wss.once("listening", () => resolve()));
  const port = (wss.address() as AddressInfo).port;
  const sockets = new Set<WebSocket>();
  const received: string[] = [];
  wss.on("connection", (socket) => {
    sockets.add(socket);
    socket.on("message", (data) => received.push(String(data)));
    socket.on("close", () => sockets.delete(socket));
  });
  return {
    url: `ws://127.0.0.1:${port}/ws/remote-desktop?ticket=test`,
    sockets,
    received,
    async close() {
      for (const socket of sockets) {
        socket.terminate();
      }
      await new Promise<void>((resolve) => wss.close(() => resolve()));
    },
  };
}

async function waitForEvent(
  events: RemoteDesktopSessionEvent[],
  predicate: (event: RemoteDesktopSessionEvent) => boolean,
  what: string,
): Promise<RemoteDesktopSessionEvent> {
  const deadline = Date.now() + 3_000;
  while (Date.now() < deadline) {
    const found = events.find(predicate);
    if (found) {
      return found;
    }
    await delay(10);
  }
  throw new Error(`等待 ${what} 超时；已收到：${JSON.stringify(events)}`);
}

function createController(
  open: () => Promise<{ ok: true; wsUrl: string; mediaUrl?: string }>,
  options?: { handshakeTimeoutMs?: number },
) {
  const events: RemoteDesktopSessionEvent[] = [];
  const controller = createRemoteDesktopHostController({
    openSession: open,
    logger: { info: () => undefined, warn: () => undefined },
    push: (event) => events.push(event),
    ...(options?.handshakeTimeoutMs !== undefined
      ? { handshakeTimeoutMs: options.handshakeTimeoutMs }
      : {}),
  });
  return { controller, events };
}

test("start → connected → 对端信令 → bye 收口", async () => {
  const peer = await startFakePeer();
  const { controller, events } = createController(async () => ({
    ok: true,
    wsUrl: peer.url,
    mediaUrl: "ws://127.0.0.1:1/ws/remote-desktop-media?mediaTicket=t",
  }));
  try {
    const result = await controller.start({ serverId: "server-1" });
    assert.deepEqual(result, {
      ok: true,
      mediaUrl: "ws://127.0.0.1:1/ws/remote-desktop-media?mediaTicket=t",
    });
    await waitForEvent(
      events,
      (event) => event.kind === "state" && event.state === "connected",
      "connected",
    );

    const socket = [...peer.sockets][0];
    assert.ok(socket, "对端应已建立 WS");
    socket.send(
      JSON.stringify({
        type: "monitors",
        monitors: [
          {
            displayId: "1",
            label: "Primary",
            widthPx: 1920,
            heightPx: 1080,
            scaleFactor: 1,
            primary: true,
          },
        ],
      }),
    );
    await waitForEvent(events, (event) => event.kind === "signal", "signal");

    socket.send(JSON.stringify({ type: "bye" }));
    await waitForEvent(
      events,
      (event) => event.kind === "state" && event.state === "closed",
      "closed",
    );

    assert.deepEqual(
      events.map((event) =>
        event.kind === "state" ? event.state : `signal:${event.message.type}`,
      ),
      ["connecting", "connected", "signal:monitors", "signal:bye", "closed"],
    );
  } finally {
    await controller.dispose();
    await peer.close();
  }
});

test("单会话互斥：活动会话期间第二次 start 返回 busy", async () => {
  const peer = await startFakePeer();
  const { controller, events } = createController(async () => ({ ok: true, wsUrl: peer.url }));
  try {
    assert.deepEqual(await controller.start({ serverId: "server-1" }), { ok: true });
    await waitForEvent(
      events,
      (event) => event.kind === "state" && event.state === "connected",
      "connected",
    );
    assert.deepEqual(await controller.start({ serverId: "server-2" }), { error: "busy" });

    await controller.end("user-end");
    await waitForEvent(
      events,
      (event) => event.kind === "state" && event.state === "closed",
      "closed",
    );
    const deadline = Date.now() + 1_000;
    while (
      !peer.received.some((frame) => frame.includes('"type":"bye"')) &&
      Date.now() < deadline
    ) {
      await delay(10);
    }
    assert.ok(
      peer.received.some((frame) => frame.includes('"type":"bye"')),
      `本地结束必须带出 bye；实际：${peer.received.join(",")}`,
    );
  } finally {
    await controller.dispose();
    await peer.close();
  }
});

test("传输断开（无 bye）标记 retryable；对端 error 带稳定错误码", async () => {
  const peer = await startFakePeer();
  const { controller, events } = createController(async () => ({ ok: true, wsUrl: peer.url }));
  try {
    await controller.start({ serverId: "server-1" });
    await waitForEvent(
      events,
      (event) => event.kind === "state" && event.state === "connected",
      "connected",
    );
    const socket = [...peer.sockets][0];
    assert.ok(socket);
    socket.terminate();
    const closed = await waitForEvent(
      events,
      (event) => event.kind === "state" && event.state === "closed",
      "closed",
    );
    assert.equal(closed.kind === "state" ? closed.retryable : undefined, true);
    assert.equal(closed.kind === "state" ? closed.error : undefined, undefined);
  } finally {
    await controller.dispose();
    await peer.close();
  }

  const secondPeer = await startFakePeer();
  const second = createController(async () => ({ ok: true, wsUrl: secondPeer.url }));
  try {
    await second.controller.start({ serverId: "server-1" });
    await waitForEvent(
      second.events,
      (event) => event.kind === "state" && event.state === "connected",
      "connected",
    );
    const socket = [...secondPeer.sockets][0];
    assert.ok(socket);
    socket.send(JSON.stringify({ type: "error", code: "busy" }));
    const closed = await waitForEvent(
      second.events,
      (event) => event.kind === "state" && event.state === "closed",
      "closed",
    );
    assert.equal(closed.kind === "state" ? closed.error : undefined, "busy");
    const signals = second.events.filter((event) => event.kind === "signal");
    assert.equal(signals.length, 1, "error 帧先作为 signal 透传，再收口");
  } finally {
    await second.controller.dispose();
    await secondPeer.close();
  }
});

test("dispose 先带出 bye 再关闭 WS", async () => {
  const peer = await startFakePeer();
  const { controller, events } = createController(async () => ({ ok: true, wsUrl: peer.url }));
  try {
    await controller.start({ serverId: "server-1" });
    await waitForEvent(
      events,
      (event) => event.kind === "state" && event.state === "connected",
      "connected",
    );
    await controller.dispose();
    const deadline = Date.now() + 1_000;
    while (
      !peer.received.some((frame) => frame.includes('"type":"bye"')) &&
      Date.now() < deadline
    ) {
      await delay(10);
    }
    assert.ok(peer.received.some((frame) => frame.includes('"type":"bye"')));
  } finally {
    await peer.close();
  }
});

test("握手窗口内对端一帧不回 → unsupported 收口（不再永久挂着）", async () => {
  // 实测症状（2026-10-26）：对端跑旧版本，信令 schema 把新版本 hello 当非法帧静默丢弃，
  // 主控停在 connected 永远等——用户只看到「连不上」。窗口到期必须收口且不可重试。
  const peer = await startFakePeer();
  const { controller, events } = createController(
    async () => ({ ok: true, wsUrl: peer.url }),
    { handshakeTimeoutMs: 80 },
  );
  try {
    const result = await controller.start({ serverId: "server-silent" });
    assert.deepEqual(result, { ok: true });
    await waitForEvent(
      events,
      (event) => event.kind === "state" && event.state === "connected",
      "connected",
    );
    const closed = await waitForEvent(
      events,
      (event) => event.kind === "state" && event.state === "closed",
      "closed",
    );
    assert.equal(closed.kind === "state" ? closed.error : undefined, "unsupported");
    assert.equal(
      closed.kind === "state" ? (closed.retryable ?? false) : "missing",
      false,
      "版本不匹配重试无意义，不得标记 retryable",
    );
  } finally {
    await controller.dispose();
    await peer.close();
  }
});

test("对端及时回一帧则不触发握手超时", async () => {
  const peer = await startFakePeer();
  const { controller, events } = createController(
    async () => ({ ok: true, wsUrl: peer.url }),
    { handshakeTimeoutMs: 120 },
  );
  try {
    await controller.start({ serverId: "server-alive" });
    await waitForEvent(
      events,
      (event) => event.kind === "state" && event.state === "connected",
      "connected",
    );
    const socket = [...peer.sockets][0]!;
    socket.send(JSON.stringify({ type: "hello", protocolVersion: 2 }));
    await delay(250);
    assert.ok(
      !events.some((event) => event.kind === "state" && event.state === "closed"),
      `对端有回帧时不得因握手超时收口；实际：${JSON.stringify(events)}`,
    );
  } finally {
    await controller.dispose();
    await peer.close();
  }
});
