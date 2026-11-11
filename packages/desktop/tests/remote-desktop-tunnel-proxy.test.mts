import assert from "node:assert/strict";
import test from "node:test";
import { WebSocket } from "ws";

import type { RemoteStream } from "@mode/remote-core";

import { RemoteDesktopTunnelProxy } from "../src/host/remoteV3/rdProxy.js";

function createStream(): {
  stream: RemoteStream;
  sent: Buffer[];
  deliver(data: Uint8Array): void;
  close(reason?: string): void;
} {
  const dataListeners = new Set<(data: Uint8Array) => void>();
  const closeListeners = new Set<(reason: string) => void>();
  const sent: Buffer[] = [];
  let closed = false;
  return {
    sent,
    stream: {
      streamId: 1,
      service: "remote.ws",
      metadata: {},
      send: (data) => sent.push(Buffer.from(data)),
      close: (reason = "closed") => {
        if (closed) return;
        closed = true;
        for (const listener of closeListeners) listener(reason);
      },
      onData: (callback) => {
        dataListeners.add(callback);
        return () => dataListeners.delete(callback);
      },
      onClose: (callback) => {
        closeListeners.add(callback);
        return () => closeListeners.delete(callback);
      },
    },
    deliver: (data) => {
      for (const listener of dataListeners) listener(data);
    },
    close: (reason = "closed") => {
      if (closed) return;
      closed = true;
      for (const listener of closeListeners) listener(reason);
    },
  };
}

function waitOpen(ws: WebSocket): Promise<void> {
  return new Promise((resolve, reject) => {
    ws.once("open", () => resolve());
    ws.once("error", reject);
  });
}

test("远程桌面代理在媒体隧道断开后按 media-ticket 重开同一回环 URL", async () => {
  const proxy = new RemoteDesktopTunnelProxy();
  const signal = createStream();
  const firstMedia = createStream();
  const reopenedMedia = createStream();
  const reopenTickets: string[] = [];
  try {
    const { mediaUrl } = await proxy.register(signal.stream, firstMedia.stream, {
      reopenMedia: async (ticket) => {
        reopenTickets.push(ticket);
        return reopenedMedia.stream;
      },
    });

    const first = new WebSocket(mediaUrl);
    await waitOpen(first);
    first.send(Buffer.from([1, 2, 3]), { binary: true });
    await new Promise((resolve) => setTimeout(resolve, 10));
    assert.equal(firstMedia.sent.length, 1);
    assert.deepEqual(firstMedia.sent[0], Buffer.from([1, 1, 2, 3]));

    // 被控端通过 signal 下发新票据后，媒体隧道断开。
    signal.deliver(
      Buffer.from(
        [0, ...Buffer.from(JSON.stringify({ type: "media-ticket", mediaTicket: "ticket-2" }))],
      ),
    );
    const firstClosed = new Promise<void>((resolve) => first.once("close", () => resolve()));
    firstMedia.close("simulated drop");
    await firstClosed;

    // renderer 重连同一 URL（不带 query），代理应使用 signal 收到的新票据。
    const second = new WebSocket(mediaUrl);
    await waitOpen(second);
    second.send(Buffer.from([9]), { binary: true });
    await new Promise((resolve) => setTimeout(resolve, 20));
    assert.deepEqual(reopenTickets, ["ticket-2"]);
    assert.equal(reopenedMedia.sent.length, 1);
    assert.deepEqual(reopenedMedia.sent[0], Buffer.from([1, 9]));
    second.close();
  } finally {
    proxy.close();
  }
});
