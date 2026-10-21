/**
 * WSS 中继 MVP 集成测试（docs/specs/wss-relay-mvp.md）：
 * 假本地 Host + 中继 + 隧道客户端，验证 HTTP/WS 透明转发、路径前缀与失败语义。
 */
import assert from "node:assert/strict";
import { once } from "node:events";
import { createServer } from "node:http";
import test from "node:test";

import { WebSocket, WebSocketServer } from "ws";

import { startRelayServer, type RelayServer } from "../src/server.js";
import { startRelayTunnelClient, type RelayTunnelClient } from "../src/tunnelClient.js";

const TUNNEL_ID = "test-tunnel-1234567890";

async function startFakeHost(): Promise<{ port: number; close: () => Promise<void> }> {
  const httpServer = createServer((req, res) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => {
      const body = Buffer.concat(chunks).toString("utf8");
      if (req.url?.startsWith("/api/server-info")) {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ path: req.url, method: req.method, body }));
        return;
      }
      if (req.url?.startsWith("/api/remote-desktop/session")) {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ ticket: "t", mediaTicket: "m", body }));
        return;
      }
      res.writeHead(404);
      res.end();
    });
  });
  const wss = new WebSocketServer({ noServer: true });
  httpServer.on("upgrade", (req, socket, head) => {
    if (!req.url?.startsWith("/ws/echo")) {
      socket.destroy();
      return;
    }
    wss.handleUpgrade(req, socket, head, (ws) => {
      ws.on("message", (data, isBinary) => ws.send(data, { binary: isBinary }));
    });
  });
  await new Promise<void>((resolve) => httpServer.listen(0, "127.0.0.1", resolve));
  const address = httpServer.address();
  const port = typeof address === "object" && address ? address.port : 0;
  return {
    port,
    async close() {
      await new Promise<void>((resolve) => {
        wss.close(() => resolve());
        httpServer.close(() => resolve());
        (httpServer as unknown as { closeAllConnections?: () => void }).closeAllConnections?.();
      });
    },
  };
}

async function waitFor(predicate: () => boolean, timeoutMs = 5_000): Promise<void> {
  const startedAt = Date.now();
  while (!predicate()) {
    if (Date.now() - startedAt > timeoutMs) {
      throw new Error("waitFor timeout");
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

async function startFixture(options: { maxHttpBodyBytes?: number } = {}): Promise<{
  relay: RelayServer;
  tunnel: RelayTunnelClient;
  local: { port: number; close: () => Promise<void> };
  base: string;
}> {
  const local = await startFakeHost();
  const relay = await startRelayServer({
    port: 0,
    token: "test-relay-token",
    publicBaseUrl: "http://127.0.0.1",
    log: () => undefined,
    ...(options.maxHttpBodyBytes !== undefined
      ? { maxHttpBodyBytes: options.maxHttpBodyBytes }
      : {}),
  });
  const tunnel = startRelayTunnelClient({
    relayUrl: `ws://127.0.0.1:${relay.port}/_tunnel`,
    token: "test-relay-token",
    tunnelId: TUNNEL_ID,
    localBaseUrl: `http://127.0.0.1:${local.port}`,
    log: () => undefined,
    reconnect: false,
  });
  await waitFor(() => relay.getTunnelIds().includes(TUNNEL_ID));
  return {
    relay,
    tunnel,
    local,
    base: `http://127.0.0.1:${relay.port}/d/${TUNNEL_ID}`,
  };
}

async function closeFixture(fixture: {
  relay: RelayServer;
  tunnel: RelayTunnelClient;
  local: { close: () => Promise<void> };
}): Promise<void> {
  fixture.tunnel.close();
  await fixture.relay.close();
  await fixture.local.close();
}

test("中继：HTTP GET/POST 与 WS 文本/二进制经路径前缀透传", async () => {
  const fixture = await startFixture();
  try {
    const info = (await (await fetch(`${fixture.base}/api/server-info?token=abc`)).json()) as {
      path?: string;
    };
    assert.equal(info.path, "/api/server-info?token=abc");

    const session = (await (
      await fetch(`${fixture.base}/api/remote-desktop/session`, {
        method: "POST",
        body: "hello",
      })
    ).json()) as { ticket?: string; body?: string };
    assert.equal(session.ticket, "t");
    assert.equal(session.body, "hello");

    const ws = new WebSocket(`${fixture.base.replace("http://", "ws://")}/ws/echo`);
    await once(ws, "open");
    ws.send("hello");
    const [text] = (await once(ws, "message")) as [Buffer, boolean];
    assert.equal(String(text), "hello");
    ws.send(Buffer.from([1, 2, 3]));
    const [binary] = (await once(ws, "message")) as [Buffer, boolean];
    assert.deepEqual([...binary], [1, 2, 3]);
    ws.close();
  } finally {
    await closeFixture(fixture);
  }
});

test("中继：无隧道 503，隧道断开后回到 503", async () => {
  const relay = await startRelayServer({
    port: 0,
    token: "test-relay-token",
    publicBaseUrl: "http://127.0.0.1",
    log: () => undefined,
  });
  try {
    const missing = await fetch(
      `http://127.0.0.1:${relay.port}/d/unknown-tunnel-1234567890/api/server-info`,
    );
    assert.equal(missing.status, 503);
  } finally {
    await relay.close();
  }

  const fixture = await startFixture();
  try {
    fixture.tunnel.close();
    await waitFor(() => !fixture.relay.getTunnelIds().includes(TUNNEL_ID));
    const offline = await fetch(`${fixture.base}/api/server-info`);
    assert.equal(offline.status, 503);
  } finally {
    await closeFixture(fixture);
  }
});

test("中继：超过 HTTP body 上限返回 413", async () => {
  const fixture = await startFixture({ maxHttpBodyBytes: 16 });
  try {
    const response = await fetch(`${fixture.base}/api/remote-desktop/session`, {
      method: "POST",
      body: "x".repeat(64),
    });
    assert.equal(response.status, 413);
  } finally {
    await closeFixture(fixture);
  }
});

test("中继：健康检查", async () => {
  const relay = await startRelayServer({
    port: 0,
    token: "test-relay-token",
    publicBaseUrl: "http://127.0.0.1",
    log: () => undefined,
  });
  try {
    const response = await fetch(`http://127.0.0.1:${relay.port}/_health`);
    assert.equal(response.status, 200);
    assert.equal(await response.text(), "ok");
  } finally {
    await relay.close();
  }
});
