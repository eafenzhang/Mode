import assert from "node:assert/strict";
import test from "node:test";
import type { AddressInfo } from "node:net";
import { WebSocket } from "ws";
import type { ServiceCollection } from "@mode/services";
import { createHttpServer, type RemoteDesktopHooks } from "../src/http.js";

// 局域网远程桌面被控端点（spec: docs/specs/lan-remote-desktop.md 接口变更 2）：
// POST /api/remote-desktop/session（设备令牌 → 被控裁决 → 一次性票据）
// GET  /ws/remote-desktop?ticket=（票据单次消费升级 → 宿主转发 JSON 单帧信令通道）。
// 失效模式（未实现时）：令牌未校验即可开会话、票据可重放、或升级后的帧不可达。

const TOKEN_A = "lan-rd-token-a";
const TOKEN_B = "lan-rd-token-b";
const CLIENT_A = "lan-client-a";
const CLIENT_B = "lan-client-b";
const clientByToken: Record<string, string> = { [TOKEN_A]: CLIENT_A, [TOKEN_B]: CLIENT_B };

const cookieFor = (token: string) => ({ cookie: `mode_lite_token=${encodeURIComponent(token)}` });

async function startServer(
  options: {
    remoteDesktop?: RemoteDesktopHooks;
    remoteDesktopCapability?: boolean;
    withClientResolution?: boolean;
  } = {},
) {
  const { withClientResolution = true, ...httpOptions } = options;
  const server = createHttpServer({} as unknown as ServiceCollection, 0, {
    host: "127.0.0.1",
    authRequired: true,
    verifyAuthToken: (token) => token === TOKEN_A || token === TOKEN_B,
    ...(withClientResolution
      ? { resolveLanClientByToken: (token: string) => clientByToken[token] ?? null }
      : {}),
    ...httpOptions,
  });
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  const { port } = server.address() as AddressInfo;
  return { server, port, baseUrl: `http://127.0.0.1:${port}` };
}

async function stopServer(server: {
  close: (callback?: () => void) => void;
  closeAllConnections?: () => void;
}): Promise<void> {
  server.closeAllConnections?.();
  await new Promise<void>((resolve) => server.close(() => resolve()));
  await new Promise((resolve) => setTimeout(resolve, 50));
}

async function waitFor(condition: () => boolean, label: string, timeoutMs = 5_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!condition()) {
    if (Date.now() > deadline) {
      throw new Error(`${label} 超时（${timeoutMs}ms）`);
    }
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

/**
 * 以非升级结果结束的连接：读回 HTTP 状态。node-ws 适配器对未升级的响应统一写
 * `Content-Length: 0`（/ws/host 同样如此），因此升级路径只能断言状态码；
 * 响应体契约用同路径的普通 HTTP GET 断言（Hono 正常响应路径）。
 */
function connectUntilRejected(url: string, headers: Record<string, string>): Promise<number> {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url, { headers });
    ws.on("unexpected-response", (_request, response) => {
      response.on("data", () => {});
      response.on("end", () => {
        ws.terminate();
        resolve(response.statusCode ?? 0);
      });
    });
    ws.on("open", () => {
      ws.close();
      reject(new Error("升级不应成功"));
    });
    ws.on("error", () => {
      // 拒绝路径由 unexpected-response 收口；连接层错误在这里忽略。
    });
  });
}

test("未带令牌 POST → 401（既有 /api/* 鉴权语义）", async () => {
  const { server, baseUrl } = await startServer({
    remoteDesktop: { requestSession: async () => ({ ok: true }), onSessionSocket: () => {} },
  });
  try {
    const response = await fetch(`${baseUrl}/api/remote-desktop/session`, { method: "POST" });
    assert.equal(response.status, 401);
  } finally {
    await stopServer(server);
  }
});

test("未配置 hooks 的 server：POST / GET 端点均返回稳定错误且不抛出", async () => {
  const { server, baseUrl, port } = await startServer();
  try {
    const post = await fetch(`${baseUrl}/api/remote-desktop/session`, {
      method: "POST",
      headers: cookieFor(TOKEN_A),
    });
    assert.equal(post.status, 501);
    assert.deepEqual(await post.json(), { error: "Remote desktop is not available" });

    const get = await fetch(`${baseUrl}/ws/remote-desktop?ticket=whatever`, {
      headers: cookieFor(TOKEN_A),
    });
    assert.equal(get.status, 501);
    assert.deepEqual(await get.json(), { error: "Remote desktop is not available" });

    const upgrade = await connectUntilRejected(
      `ws://127.0.0.1:${port}/ws/remote-desktop?ticket=whatever`,
      cookieFor(TOKEN_A),
    );
    assert.equal(upgrade, 501);

    // media 端点同口径：缺 hooks → 501（不升级连接）。
    const mediaUpgrade = await connectUntilRejected(
      `ws://127.0.0.1:${port}/ws/remote-desktop-media?mediaTicket=whatever`,
      cookieFor(TOKEN_A),
    );
    assert.equal(mediaUpgrade, 501);
  } finally {
    await stopServer(server);
  }
});

test("hooks 提供但缺 onMediaSocket：media 端点 501（信令端点语义不变）", async () => {
  const { server, baseUrl, port } = await startServer({
    remoteDesktop: { requestSession: async () => ({ ok: true }), onSessionSocket: () => {} },
  });
  try {
    const media = await connectUntilRejected(
      `ws://127.0.0.1:${port}/ws/remote-desktop-media?mediaTicket=whatever`,
      cookieFor(TOKEN_A),
    );
    assert.equal(media, 501, "缺 onMediaSocket 时不升级媒体连接");

    const post = await fetch(`${baseUrl}/api/remote-desktop/session`, {
      method: "POST",
      headers: cookieFor(TOKEN_A),
    });
    assert.equal(post.status, 200, "信令 POST 不受 onMediaSocket 缺失影响");
    const { ticket } = (await post.json()) as { ticket: string };
    // 升级成功用直连断言（connectUntilRejected 成功即 reject）。
    const signal = new WebSocket(
      `ws://127.0.0.1:${port}/ws/remote-desktop?ticket=${encodeURIComponent(ticket)}`,
      { headers: cookieFor(TOKEN_A) },
    );
    try {
      await new Promise<void>((resolve, reject) => {
        signal.once("open", () => resolve());
        signal.once("error", reject);
      });
    } finally {
      signal.close();
    }
  } finally {
    await stopServer(server);
  }
});

test("POST 响应带 mediaTicket；media 端点凭 mediaTicket 单次升级", async () => {
  const sockets: Array<{ clientId: string }> = [];
  const { server, baseUrl, port } = await startServer({
    remoteDesktop: {
      requestSession: async () => ({ ok: true }),
      onSessionSocket: () => {},
      onMediaSocket: (clientId) => {
        sockets.push({ clientId });
      },
    },
  });
  const mediaClients: WebSocket[] = [];
  try {
    const post = await fetch(`${baseUrl}/api/remote-desktop/session`, {
      method: "POST",
      headers: cookieFor(TOKEN_A),
    });
    assert.equal(post.status, 200);
    const { ticket, mediaTicket } = (await post.json()) as {
      ticket: string;
      mediaTicket: string;
    };
    assert.ok(ticket && mediaTicket && ticket !== mediaTicket, "两张票据各自独立");

    // 票据即凭证（spec：票据单次消费、30s、绑定 clientId）：media 端点豁免全局令牌鉴权，
    // renderer 不带任何 cookie/token 直连——这是本次修复的核心口径。
    // 注意 connectUntilRejected 在升级成功时会 reject("升级不应成功")，
    // 因此下面的拒绝路径必须以 401 收口而不是抛出。
    const wrongOnMedia = await connectUntilRejected(
      `ws://127.0.0.1:${port}/ws/remote-desktop-media?mediaTicket=${encodeURIComponent(ticket)}`,
      {},
    );
    assert.equal(wrongOnMedia, 401, "信令票不能升级 media 端点");
    // 信令端点的令牌语义保持不变：mediaTicket 冒充信令票时仍需出示合法令牌才到票据校验。
    const wrongOnSignal = await connectUntilRejected(
      `ws://127.0.0.1:${port}/ws/remote-desktop?ticket=${encodeURIComponent(mediaTicket)}`,
      cookieFor(TOKEN_A),
    );
    assert.equal(wrongOnSignal, 401, "mediaTicket 不能升级信令端点");
    const unknownMedia = await connectUntilRejected(
      `ws://127.0.0.1:${port}/ws/remote-desktop-media?mediaTicket=not-issued`,
      {},
    );
    assert.equal(unknownMedia, 401, "未知 mediaTicket 必须 401");

    // 升级成功路径（renderer 真实形态：无 cookie、无 token，仅 mediaTicket）：
    // 直连 + open 断言（connectUntilRejected 成功即 reject，不能复用）。
    const client = new WebSocket(
      `ws://127.0.0.1:${port}/ws/remote-desktop-media?mediaTicket=${encodeURIComponent(mediaTicket)}`,
    );
    mediaClients.push(client);
    await new Promise<void>((resolve, reject) => {
      client.once("open", () => resolve());
      client.once("error", reject);
    });
    await waitFor(() => sockets.length === 1, "onMediaSocket 挂接");
    // clientId 由票据反解（签发时绑定），不是从请求令牌推导。
    assert.equal(sockets[0]?.clientId, CLIENT_A, "media socket 绑定签发时的 clientId");

    // 重放：mediaTicket 已消费 → 401（同样不带任何令牌）。
    const replay = await connectUntilRejected(
      `ws://127.0.0.1:${port}/ws/remote-desktop-media?mediaTicket=${encodeURIComponent(mediaTicket)}`,
      {},
    );
    assert.equal(replay, 401, "mediaTicket 重放必须 401");
    assert.equal(sockets.length, 1, "onMediaSocket 只挂接一次");
  } finally {
    for (const ws of mediaClients) {
      ws.close();
    }
    await stopServer(server);
  }
});

test("令牌合法但解析不出 clientId（无法绑定票据）→ 401 unauthorized", async () => {
  const { server, baseUrl } = await startServer({
    withClientResolution: false,
    remoteDesktop: { requestSession: async () => ({ ok: true }), onSessionSocket: () => {} },
  });
  try {
    const response = await fetch(`${baseUrl}/api/remote-desktop/session`, {
      method: "POST",
      headers: cookieFor(TOKEN_A),
    });
    assert.equal(response.status, 401);
    assert.deepEqual(await response.json(), { code: "unauthorized" });
  } finally {
    await stopServer(server);
  }
});

test("requestSession 返回错误码 → 对应 HTTP 状态与 {code}（拒绝/超时/busy/开关关）", async () => {
  const cases = [
    { error: "busy", status: 409 },
    { error: "consent_denied", status: 403 },
    { error: "consent_timeout", status: 403 },
    { error: "disabled", status: 403 },
    { error: "unsupported", status: 501 },
    { error: "internal", status: 500 },
  ] as const;
  for (const entry of cases) {
    const { server, baseUrl } = await startServer({
      remoteDesktop: {
        requestSession: async () => ({ error: entry.error }),
        onSessionSocket: () => {},
      },
    });
    try {
      const response = await fetch(`${baseUrl}/api/remote-desktop/session`, {
        method: "POST",
        headers: cookieFor(TOKEN_A),
      });
      assert.equal(response.status, entry.status, `${entry.error} 的状态码`);
      assert.deepEqual(await response.json(), { code: entry.error });
    } finally {
      await stopServer(server);
    }
  }
});

test("正常路径：放行 → 200 票据；票据升级后 JSON 单帧双向可达；重放被拒", async () => {
  const requestedClients: string[] = [];
  const sessions: Array<{ clientId: string; socket: WebSocket }> = [];
  const { server, baseUrl, port } = await startServer({
    remoteDesktopCapability: true,
    remoteDesktop: {
      requestSession: async (clientId) => {
        requestedClients.push(clientId);
        return { ok: true };
      },
      onSessionSocket: (clientId, socket) => {
        sessions.push({ clientId, socket });
      },
    },
  });

  let client: WebSocket | undefined;
  try {
    const response = await fetch(`${baseUrl}/api/remote-desktop/session`, {
      method: "POST",
      headers: cookieFor(TOKEN_A),
    });
    assert.equal(response.status, 200);
    const issued = (await response.json()) as { ticket?: string; expiresAt?: number };
    assert.ok(issued.ticket, "必须返回票据");
    assert.equal(typeof issued.expiresAt, "number");
    assert.deepEqual(requestedClients, [CLIENT_A], "发起方身份必须来自令牌解析的 clientId");

    const url = `ws://127.0.0.1:${port}/ws/remote-desktop?ticket=${encodeURIComponent(issued.ticket)}`;
    client = new WebSocket(url, { headers: cookieFor(TOKEN_A) });
    await new Promise<void>((resolve, reject) => {
      client?.once("open", () => resolve());
      client?.once("error", reject);
    });
    await waitFor(() => sessions.length === 1, "宿主拿到信令 socket");
    const session = sessions[0];
    assert.ok(session);
    assert.equal(session.clientId, CLIENT_A);

    // 主控 → 被控：UTF-8 JSON 单帧（内容由宿主转发，本模块不解析）
    const receivedByHost: string[] = [];
    session.socket.on("message", (data: Buffer) => receivedByHost.push(data.toString("utf8")));
    client.send(JSON.stringify({ type: "hello", protocolVersion: 1 }));
    await waitFor(() => receivedByHost.length === 1, "被控收到 hello");
    assert.deepEqual(JSON.parse(receivedByHost[0] as string), {
      type: "hello",
      protocolVersion: 1,
    });

    // 被控 → 主控
    const receivedByClient: string[] = [];
    client.on("message", (data: Buffer) => receivedByClient.push(data.toString("utf8")));
    session.socket.send(JSON.stringify({ type: "bye", reason: "user-end" }));
    await waitFor(() => receivedByClient.length === 1, "主控收到 bye");
    assert.deepEqual(JSON.parse(receivedByClient[0] as string), {
      type: "bye",
      reason: "user-end",
    });

    // 单次消费：同一票据再来一次 → 401 stale_ticket（响应体经普通 GET 同路径核验）
    const replay = await connectUntilRejected(url, cookieFor(TOKEN_A));
    assert.equal(replay, 401);
    const replayGet = await fetch(`${baseUrl}/ws/remote-desktop?ticket=${encodeURIComponent(issued.ticket)}`, {
      headers: cookieFor(TOKEN_A),
    });
    assert.equal(replayGet.status, 401);
    assert.deepEqual(await replayGet.json(), { code: "stale_ticket" });
  } finally {
    client?.close();
    for (const session of sessions) {
      session.socket.close();
    }
    await stopServer(server);
  }
});

test("票据绑定 clientId：A 的票据用 B 的令牌升级 → 401 stale_ticket", async () => {
  const sessions: Array<{ clientId: string; socket: WebSocket }> = [];
  const { server, baseUrl, port } = await startServer({
    remoteDesktop: {
      requestSession: async () => ({ ok: true }),
      onSessionSocket: (clientId, socket) => {
        sessions.push({ clientId, socket });
      },
    },
  });
  try {
    const response = await fetch(`${baseUrl}/api/remote-desktop/session`, {
      method: "POST",
      headers: cookieFor(TOKEN_A),
    });
    assert.equal(response.status, 200);
    const { ticket } = (await response.json()) as { ticket: string };
    assert.ok(ticket);

    const upgrade = await connectUntilRejected(
      `ws://127.0.0.1:${port}/ws/remote-desktop?ticket=${encodeURIComponent(ticket)}`,
      cookieFor(TOKEN_B),
    );
    assert.equal(upgrade, 401);
    assert.deepEqual(sessions, [], "错客户端不得建立信令连接");
  } finally {
    await stopServer(server);
  }
});

test("升级失败形态：缺票据 / 未知票据 / 无令牌 → 401（不升级连接）", async () => {
  const { server, baseUrl, port } = await startServer({
    remoteDesktop: { requestSession: async () => ({ ok: true }), onSessionSocket: () => {} },
  });
  try {
    // 无令牌：落在既有鉴权中间件（先于票据校验）
    const noToken = await connectUntilRejected(
      `ws://127.0.0.1:${port}/ws/remote-desktop?ticket=whatever`,
      {},
    );
    assert.equal(noToken, 401);

    // 有令牌但无票据
    const noTicket = await connectUntilRejected(
      `ws://127.0.0.1:${port}/ws/remote-desktop`,
      cookieFor(TOKEN_A),
    );
    assert.equal(noTicket, 401);

    // 有令牌但票据不存在；响应体契约经普通 GET 同路径核验
    const unknownTicket = await connectUntilRejected(
      `ws://127.0.0.1:${port}/ws/remote-desktop?ticket=not-issued`,
      cookieFor(TOKEN_A),
    );
    assert.equal(unknownTicket, 401);
    const unknownTicketGet = await fetch(`${baseUrl}/ws/remote-desktop?ticket=not-issued`, {
      headers: cookieFor(TOKEN_A),
    });
    assert.equal(unknownTicketGet.status, 401);
    assert.deepEqual(await unknownTicketGet.json(), { code: "stale_ticket" });

    // 端点之外的既有行为不受影响：/api/server-info 仍带能力位
    const info = await fetch(`${baseUrl}/api/server-info`, { headers: cookieFor(TOKEN_A) });
    assert.equal(info.status, 200);
  } finally {
    await stopServer(server);
  }
});
