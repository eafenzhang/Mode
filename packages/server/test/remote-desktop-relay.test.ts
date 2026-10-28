/**
 * Web 远程桌面中继（docs/specs/web-remote-desktop.md）。
 *
 * 用 fake 对端信令/媒体 WS 验证：
 * NAS 信令 WS 信封 → 对端 JSON 转发、对端信令 → 浏览器信封、
 * 浏览器媒体 ready/输入 → 对端、对端二进制帧 → 浏览器，以及关闭收口。
 */
import assert from "node:assert/strict";
import type { AddressInfo } from "node:net";
import test from "node:test";
import { WebSocket, WebSocketServer } from "ws";

import type { ICredentialService } from "@mode/services";
import { ServiceCollection } from "@mode/services";
import { createHttpServer } from "../src/http.js";
import { RemoteDesktopPeerController } from "../src/remote/remoteDesktopPeerController.js";

const TOKEN = "rd-test-token";

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitFor(predicate: () => boolean, timeoutMs = 3000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) {
      return;
    }
    await delay(20);
  }
  throw new Error("waitFor timeout");
}

async function startWsServer(): Promise<{
  server: WebSocketServer;
  url: string;
  sockets: WebSocket[];
  close: () => Promise<void>;
}> {
  const server = new WebSocketServer({ port: 0, host: "127.0.0.1" });
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  const port = (server.address() as AddressInfo).port;
  const sockets: WebSocket[] = [];
  server.on("connection", (socket) => sockets.push(socket));
  return {
    server,
    url: `ws://127.0.0.1:${String(port)}`,
    sockets,
    close: () =>
      new Promise<void>((resolve) => {
        for (const socket of sockets) {
          try {
            socket.terminate();
          } catch {
            // ignore
          }
        }
        server.close(() => resolve());
      }),
  };
}

test("远程桌面中继：信令信封/媒体字节双向转发与关闭收口", async () => {
  const peerSignal = await startWsServer();
  const peerMedia = await startWsServer();
  const peerSignalMessages: unknown[] = [];
  const peerMediaInputs: string[] = [];
  peerSignal.server.on("connection", (socket) => {
    socket.on("message", (data) => {
      try {
        peerSignalMessages.push(JSON.parse(String(data)));
      } catch {
        // ignore
      }
    });
  });
  peerMedia.server.on("connection", (socket) => {
    socket.on("message", (data) => {
      peerMediaInputs.push(String(data));
    });
  });

  const controller = new RemoteDesktopPeerController(
    {} as ICredentialService,
    () => undefined,
    async () => ({ ok: true, wsUrl: peerSignal.url, mediaUrl: peerMedia.url }),
    async () => ({ online: true, remoteDesktop: true, remoteDesktopNative: true }),
  );

  const server = createHttpServer({} as unknown as ServiceCollection, 0, {
    authToken: TOKEN,
    authRequired: true,
    remoteDesktopPeers: controller,
  });
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  const port = (server.address() as AddressInfo).port;
  const base = `http://127.0.0.1:${String(port)}`;

  let browserSignal: WebSocket | null = null;
  let browserMedia: WebSocket | null = null;
  try {
    const response = await fetch(`${base}/api/remote-desktop/peer-session?token=${TOKEN}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ serverId: "peer-1" }),
    });
    assert.equal(response.status, 200);
    const session = (await response.json()) as {
      sessionId: string;
      signalPath: string;
      mediaPath: string;
    };
    assert.ok(session.sessionId);

    const signalEvents: unknown[] = [];
    browserSignal = new WebSocket(
      `ws://127.0.0.1:${String(port)}${session.signalPath}?token=${TOKEN}`,
    );
    browserSignal.on("message", (data, isBinary) => {
      if (!isBinary) {
        try {
          signalEvents.push(JSON.parse(String(data)));
        } catch {
          // ignore
        }
      }
    });
    await waitFor(() =>
      signalEvents.some(
        (event) => (event as { kind?: string; state?: string }).kind === "state" &&
          (event as { state?: string }).state === "connected",
      ),
    );

    browserSignal.send(
      JSON.stringify({ kind: "signal", message: { type: "hello", protocolVersion: 2 } }),
    );
    await waitFor(() => peerSignalMessages.length >= 1);
    assert.deepEqual(peerSignalMessages[0], { type: "hello", protocolVersion: 2 });

    for (const socket of peerSignal.sockets) {
      socket.send(
        JSON.stringify({
          type: "monitors",
          monitors: [
            {
              displayId: "1",
              label: "Display",
              widthPx: 1920,
              heightPx: 1080,
              scaleFactor: 1,
              primary: true,
            },
          ],
        }),
      );
    }
    await waitFor(() =>
      signalEvents.some((event) => (event as { kind?: string }).kind === "signal"),
    );

    const mediaFrames: Buffer[] = [];
    browserMedia = new WebSocket(
      `ws://127.0.0.1:${String(port)}${session.mediaPath}?token=${TOKEN}`,
    );
    browserMedia.on("message", (data, isBinary) => {
      if (isBinary) {
        mediaFrames.push(Buffer.isBuffer(data) ? data : Buffer.from(data as ArrayBuffer));
      }
    });
    await waitFor(() => peerMedia.sockets.length >= 1);
    browserMedia.send(JSON.stringify({ t: "ready" }));
    await waitFor(() => peerMediaInputs.some((text) => text.includes('"ready"')));

    for (const socket of peerMedia.sockets) {
      socket.send(Buffer.from([9, 8, 7]));
    }
    await waitFor(() => mediaFrames.length >= 1);
    assert.deepEqual([...mediaFrames[0]!], [9, 8, 7]);

    browserMedia.send(JSON.stringify({ t: "pm", x: 0.5, y: 0.5 }));
    await waitFor(() => peerMediaInputs.some((text) => text.includes('"pm"')));

    // 媒体瞬断重连窗口：浏览器媒体 WS 断开后对端媒体 WS 不立即关闭，
    // 新浏览器媒体 WS 复用它（不新建对端连接），并继续收到帧。
    const peerMediaSocket = peerMedia.sockets[0]!;
    browserMedia.close();
    await waitFor(() => browserMedia?.readyState === WebSocket.CLOSED);
    const reattachedFrames: Buffer[] = [];
    const browserMedia2 = new WebSocket(
      `ws://127.0.0.1:${String(port)}${session.mediaPath}?token=${TOKEN}`,
    );
    browserMedia2.on("message", (data, isBinary) => {
      if (isBinary) {
        reattachedFrames.push(Buffer.isBuffer(data) ? data : Buffer.from(data as ArrayBuffer));
      }
    });
    await waitFor(() => browserMedia2.readyState === WebSocket.OPEN);
    assert.equal(peerMedia.sockets.length, 1);
    browserMedia2.send(JSON.stringify({ t: "ready" }));
    await waitFor(() => peerMediaInputs.filter((text) => text.includes('"ready"')).length >= 2);
    peerMediaSocket.send(Buffer.from([1, 2]));
    await waitFor(() => reattachedFrames.length >= 1);
    assert.deepEqual([...reattachedFrames[0]!], [1, 2]);
    browserMedia2.close();

    browserSignal.close();
    await waitFor(() => peerSignal.sockets.every((socket) => socket.readyState !== WebSocket.OPEN));
  } finally {
    browserSignal?.close();
    browserMedia?.close();
    await controller.disposeAll();
    await new Promise<void>((resolve) => server.close(() => resolve()));
    await peerSignal.close();
    await peerMedia.close();
  }
});
