import assert from "node:assert/strict";
import test from "node:test";
import type { AddressInfo } from "node:net";
import { WebSocket } from "ws";
import {
  ChannelClient,
  SocketProtocol,
  ProxyChannel,
  VSBuffer,
  Emitter,
  type ISocket,
} from "@mode/rpc";
import { MODE_RPC_HOST_CAPABILITY_HEADER, lanAccessStateSchema } from "@mode/shared";
import type { ICredentialService, ServiceCollection } from "@mode/services";
import { createHttpServer } from "../src/http.js";
import { createLanAccessHost } from "../src/lanAccess.js";

// 服务端形态的局域网被连端（docs/specs/lan-host-server-mode.md）：
// 开关门控、配对码一次性、逐设备令牌与静态令牌并存、/ws/host 挂接与目录录制、
// 移除/重置的失效语义。失效模式（未实现时）：设置页动作全部拒绝、配对端点不存在。

const STATIC_TOKEN = "lan-host-static-token";
const STATIC_COOKIE = { cookie: `mode_lite_token=${encodeURIComponent(STATIC_TOKEN)}` };

function createMemoryCredentials(): ICredentialService {
  const store = new Map<string, string>();
  return {
    load: async (key: string) => store.get(key) ?? null,
    save: async (key: string, value: string) => {
      store.set(key, value);
    },
    delete: async (key: string) => {
      store.delete(key);
    },
    list: async (prefix: string) => [...store.keys()].filter((key) => key.startsWith(prefix)),
  } as ICredentialService;
}

/** 与 lan-workspace-recording.test.ts 相同：监听必须在等待 open 之前挂上，避免丢 Initialize。 */
function wrapClientSocket(ws: WebSocket): ISocket {
  const onData = new Emitter<VSBuffer>();
  const onClose = new Emitter<void>();
  const onEnd = new Emitter<void>();
  ws.on("message", (data: unknown, isBinary: boolean) => {
    if (!isBinary) {
      return;
    }
    const bytes = Buffer.isBuffer(data) ? data : Buffer.from(data as ArrayBuffer);
    onData.fire(VSBuffer.wrap(new Uint8Array(bytes)));
  });
  ws.on("close", () => {
    onClose.fire();
    onEnd.fire();
  });
  ws.on("error", () => {
    onClose.fire();
    onEnd.fire();
  });
  return {
    onData: onData.event,
    onClose: onClose.event,
    onEnd: onEnd.event,
    write(buffer: VSBuffer) {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(buffer.buffer, { binary: true });
      }
    },
    end: () => ws.close(),
    drain: () => Promise.resolve(),
    dispose: () => ws.close(),
  };
}

function servicesStub(): ServiceCollection {
  return {
    getOptional: () => undefined,
    exposeOnChannelServer: (server: {
      registerChannel: (name: string, channel: unknown) => void;
    }) => {
      server.registerChannel(
        "mode-task",
        ProxyChannel.fromService({
          list: async (_arg: unknown) => [],
          readdir: async (_arg: unknown) => [],
        }),
      );
    },
  } as unknown as ServiceCollection;
}

function withTimeout<T>(promise: Promise<T>, ms: number, label: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`${label} 超时（${ms}ms）`)), ms);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timer);
        reject(error instanceof Error ? error : new Error(String(error)));
      },
    );
  });
}

async function startFixture() {
  const credentials = createMemoryCredentials();
  let listeningPort = 0;
  const host = await createLanAccessHost({
    credentials,
    port: () => listeningPort,
    name: "test-nas",
  });
  const server = createHttpServer(servicesStub(), 0, {
    authToken: STATIC_TOKEN,
    authRequired: true,
    verifyAuthToken: (presented) => presented === STATIC_TOKEN || host.verifyToken(presented),
    lanPairing: { redeem: (request) => host.redeem(request) },
    lanAccessHost: host,
    resolveLanClientByToken: (token) => host.resolveClientId(token),
    onLanClientWorkspace: (clientId, workspacePath) =>
      host.recordClientWorkspace(clientId, workspacePath),
    serverId: host.getServerId(),
    name: "test-nas",
  });
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  listeningPort = (server.address() as AddressInfo).port;
  return {
    credentials,
    host,
    port: listeningPort,
    base: `http://127.0.0.1:${listeningPort}`,
    async close() {
      (server as unknown as { closeAllConnections?: () => void }).closeAllConnections?.();
      await new Promise<void>((resolve) => server.close(() => resolve()));
      await new Promise((resolve) => setTimeout(resolve, 100));
    },
  };
}

async function postJson(base: string, path: string, body: unknown, headers = STATIC_COOKIE) {
  return fetch(`${base}${path}`, {
    method: "POST",
    headers: { ...headers, "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

async function readState(response: Response) {
  const parsed = lanAccessStateSchema.safeParse(await response.json());
  assert.ok(parsed.success, "响应应为合法 LanAccessState");
  return parsed.data;
}

test("默认关闭：/api/lan/* 要求静态令牌；配对拒绝且不签发", async () => {
  const fx = await startFixture();
  try {
    const unauth = await fetch(`${fx.base}/api/lan/state`);
    assert.equal(unauth.status, 401, "无令牌访问状态端点必须 401");

    const state = await readState(
      await fetch(`${fx.base}/api/lan/state`, { headers: STATIC_COOKIE }),
    );
    assert.equal(state.enabled, false);
    assert.equal(state.port, null);
    assert.equal(state.pairCode, null);

    const pair = await postJson(
      fx.base,
      "/api/lan/pair",
      { code: "AAAAAA" },
      {},
    );
    assert.equal(pair.status, 401, "关闭态配对必须拒绝");

    const badBody = await postJson(fx.base, "/api/lan/enable", { enabled: "yes" });
    assert.equal(badBody.status, 400, "非法 body 必须 400");
  } finally {
    await fx.close();
  }
});

test("配对端点限速：同一分钟超过 30 次返回 429", async () => {
  const fx = await startFixture();
  try {
    for (let attempt = 0; attempt < 30; attempt += 1) {
      const response = await postJson(fx.base, "/api/lan/pair", { code: "000000" }, {});
      assert.equal(response.status, 401, `第 ${attempt + 1} 次应为码无效而不是限速`);
    }
    const limited = await postJson(fx.base, "/api/lan/pair", { code: "000000" }, {});
    assert.equal(limited.status, 429, "超过窗口上限必须限速");
  } finally {
    await fx.close();
  }
});

test("配对 → server-info → capability → /ws/host 挂接与目录录制；静态令牌并存", async () => {
  const fx = await startFixture();
  let ws: WebSocket | undefined;
  try {
    const enabled = await readState(await postJson(fx.base, "/api/lan/enable", { enabled: true }));
    assert.equal(enabled.enabled, true);
    assert.equal(enabled.port, fx.port, "开启后应回填监听端口");

    const codeState = await readState(
      await fetch(`${fx.base}/api/lan/pair-code`, { method: "POST", headers: STATIC_COOKIE }),
    );
    assert.ok(codeState.pairCode, "开启态应签发配对码");

    const wrong = await postJson(fx.base, "/api/lan/pair", { code: "000000" }, {});
    assert.equal(wrong.status, 401, "错误配对码必须拒绝");

    const pairResponse = await postJson(
      fx.base,
      "/api/lan/pair",
      { code: codeState.pairCode.code, label: "desktop-a" },
      {},
    );
    assert.equal(pairResponse.status, 200);
    const pair = (await pairResponse.json()) as { token?: string; serverId?: string; name?: string };
    assert.ok(pair.token, "配对响应必须带令牌");
    assert.equal(pair.serverId, fx.host.getServerId(), "serverId 应为持久安装标识");
    assert.equal(pair.name, "test-nas");

    const replay = await postJson(
      fx.base,
      "/api/lan/pair",
      { code: codeState.pairCode.code, label: "desktop-a" },
      {},
    );
    assert.equal(replay.status, 401, "同一配对码二次兑换必须拒绝（一次性）");

    const lanCookie = { cookie: `mode_lite_token=${encodeURIComponent(pair.token!)}` };
    const infoResponse = await fetch(`${fx.base}/api/server-info?token=${pair.token}`);
    assert.equal(infoResponse.status, 200, "长期令牌应可读 server-info");
    const info = (await infoResponse.json()) as { serverId?: string; name?: string };
    assert.equal(info.serverId, fx.host.getServerId());
    assert.equal(info.name, "test-nas");

    const capabilityResponse = await fetch(`${fx.base}/api/rpc-host-capability`, {
      method: "POST",
      headers: lanCookie,
    });
    assert.equal(capabilityResponse.status, 200);
    const { capability } = (await capabilityResponse.json()) as { capability?: string };
    assert.ok(capability, "capability 缺失");

    ws = new WebSocket(`ws://127.0.0.1:${fx.port}/ws/host`, {
      headers: { [MODE_RPC_HOST_CAPABILITY_HEADER]: capability, ...lanCookie },
    });
    const client = new ChannelClient(new SocketProtocol(wrapClientSocket(ws)));
    await new Promise<void>((resolve, reject) => {
      ws?.once("open", () => resolve());
      ws?.once("error", reject);
    });
    const channel = client.getChannel<{ call: (name: string, arg?: unknown) => Promise<unknown> }>(
      "mode-task",
    );
    await withTimeout(channel.call("list", { workspacePath: "/vol1/data/demo" }), 5_000, "list");

    const state = fx.host.getState();
    assert.equal(state.clients.length, 1);
    assert.equal(state.clients[0]?.label, "desktop-a");
    assert.equal(state.clients[0]?.lastWorkspacePath, "/vol1/data/demo");
    assert.notEqual(state.clients[0]?.lastUsedAt, null, "挂接应更新 lastUsedAt");

    const staticState = await fetch(`${fx.base}/api/lan/state`, { headers: STATIC_COOKIE });
    assert.equal(staticState.status, 200, "静态令牌（浏览器/网关）全程可用");
  } finally {
    ws?.close();
    await fx.close();
  }
});

test("关闭门控与移除/重置的失效语义", async () => {
  const fx = await startFixture();
  try {
    await postJson(fx.base, "/api/lan/enable", { enabled: true });
    const code = await readState(
      await fetch(`${fx.base}/api/lan/pair-code`, { method: "POST", headers: STATIC_COOKIE }),
    );
    const token = (
      (await (
        await postJson(fx.base, "/api/lan/pair", { code: code.pairCode!.code }, {})
      ).json()) as { token: string }
    ).token;

    // 关闭：lan 令牌失效、配对码作废；静态令牌不受影响。
    const off = await readState(await postJson(fx.base, "/api/lan/enable", { enabled: false }));
    assert.equal(off.enabled, false);
    assert.equal(off.pairCode, null);
    assert.equal(
      await (async () =>
        (await fetch(`${fx.base}/api/server-info?token=${token}`)).status)(),
      401,
      "关闭后长期令牌必须失效",
    );
    const afterOff = await postJson(fx.base, "/api/lan/pair", { code: "AAAAAA" }, {});
    assert.equal(afterOff.status, 401);
    assert.equal(
      (await fetch(`${fx.base}/api/lan/state`, { headers: STATIC_COOKIE })).status,
      200,
    );

    // 重新开启：开关只门控不吊销，原令牌恢复可用。
    await postJson(fx.base, "/api/lan/enable", { enabled: true });
    assert.equal((await fetch(`${fx.base}/api/server-info?token=${token}`)).status, 200);

    // 移除单个客户端：其令牌失效、列表清空。
    const clientId = fx.host.getState().clients[0]!.id;
    const removed = await readState(
      await fetch(`${fx.base}/api/lan/clients/${clientId}`, {
        method: "DELETE",
        headers: STATIC_COOKIE,
      }),
    );
    assert.equal(removed.clients.length, 0);
    assert.equal((await fetch(`${fx.base}/api/server-info?token=${token}`)).status, 401);

    // 重置：再配对一台后整体清空。
    const code2 = await readState(
      await fetch(`${fx.base}/api/lan/pair-code`, { method: "POST", headers: STATIC_COOKIE }),
    );
    const token2 = (
      (await (
        await postJson(fx.base, "/api/lan/pair", { code: code2.pairCode!.code }, {})
      ).json()) as { token: string }
    ).token;
    const reset = await readState(
      await fetch(`${fx.base}/api/lan/reset`, { method: "POST", headers: STATIC_COOKIE }),
    );
    assert.equal(reset.clients.length, 0);
    assert.equal(fx.host.verifyToken(token2), false);
    assert.equal((await fetch(`${fx.base}/api/server-info?token=${token2}`)).status, 401);
  } finally {
    await fx.close();
  }
});
