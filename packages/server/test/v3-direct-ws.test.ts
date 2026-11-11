import assert from "node:assert/strict";
import test from "node:test";
import type { AddressInfo } from "node:net";
import { WebSocket } from "ws";
import type { ServiceCollection } from "@mode/services";

import { createHttpServer, type V3DirectSocket } from "../src/http.js";

/** /ws/v1/direct：一次性 ticket 首个握手帧、绕过 LAN Bearer、后续文本帧交给 attach 方。 */
test("v3 direct WS 首帧校验 ticket 后转发后续帧，且不要求 LAN 令牌", async () => {
  const attached: Array<{ connectionId: string; ticket: string }> = [];
  const received: string[] = [];
  const server = createHttpServer({} as ServiceCollection, 0, {
    host: "127.0.0.1",
    authRequired: true,
    authToken: "lan-token-that-should-not-be-required",
    v3Direct: {
      attach(input, socket: V3DirectSocket) {
        attached.push(input);
        socket.onMessage((text) => received.push(text));
        socket.send(
          JSON.stringify({
            v: 1,
            id: "ready-1",
            timestamp: Date.now(),
            type: "relay.ready",
            payload: { connectionId: input.connectionId },
          }),
        );
        return input.ticket === "ticket-1";
      },
    },
  });
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  const port = (server.address() as AddressInfo).port;
  const ws = new WebSocket(`ws://127.0.0.1:${port}/ws/v1/direct`);
  try {
    await new Promise<void>((resolve, reject) => {
      ws.once("open", () => resolve());
      ws.once("error", reject);
    });
    const ready = new Promise<string>((resolve) => {
      ws.on("message", (data, isBinary) => {
        if (!isBinary) resolve(data.toString());
      });
    });
    ws.send(JSON.stringify({ type: "direct.attach", connectionId: "conn-1", ticket: "ticket-1" }));
    const frame = JSON.parse(await ready) as { type: string };
    assert.equal(frame.type, "relay.ready");
    assert.deepEqual(attached, [{ connectionId: "conn-1", ticket: "ticket-1" }]);

    ws.send("noise-frame");
    const deadline = Date.now() + 2_000;
    while (received.length === 0 && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    assert.deepEqual(received, ["noise-frame"]);
  } finally {
    ws.close();
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});

test("v3 direct WS ticket 不匹配时以 1008 拒绝", async () => {
  const server = createHttpServer({} as ServiceCollection, 0, {
    host: "127.0.0.1",
    authRequired: true,
    authToken: "unused",
    v3Direct: {
      attach: () => false,
    },
  });
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  const port = (server.address() as AddressInfo).port;
  const ws = new WebSocket(`ws://127.0.0.1:${port}/ws/v1/direct`);
  try {
    await new Promise<void>((resolve, reject) => {
      ws.once("open", () => resolve());
      ws.once("error", reject);
    });
    const closed = new Promise<number>((resolve) => ws.once("close", (code) => resolve(code)));
    ws.send(JSON.stringify({ type: "direct.attach", connectionId: "c", ticket: "bad" }));
    assert.equal(await closed, 1008);
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});
