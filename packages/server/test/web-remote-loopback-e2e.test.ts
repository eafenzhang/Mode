import assert from "node:assert/strict";
import test from "node:test";
import type { AddressInfo } from "node:net";
import { WebSocket } from "ws";
import {
  ChannelClient,
  Emitter,
  SocketProtocol,
  VSBuffer,
  type IChannel,
  type ISocket,
} from "@mode/rpc";
import { IModeAgentService, ServiceCollection } from "@mode/services";
import type { IModeAgentService as IModeAgentServiceShape } from "@mode/services";
import { V4_WIRE_PROTOCOL_VERSION } from "@mode/shared/mode-protocol-v4";
import type { SessionsIndexTopicWireCandidate } from "@mode/shared/mode-protocol-v4";
import { lanAccessStateSchema } from "@mode/shared";

import { createHttpServer } from "../src/http.js";
import { createLanAccessHost } from "../src/lanAccess.js";

// Web 单机回环 E2E（不需要第二台设备）：
//   发布形态 HTTP/WS 启动器 + 真 LAN 配对 → 设备令牌连 /ws（terminal-client）→
//   web-remote-replayable 握手 → sessions-index 订阅的 initial 帧回放。
// 另证两条边界：replayable 连接**不能**消费 continuous-only 事件；/ws 在线数
// 会经 onWebSessionChange 映射到宿主状态。
//
// 用假 agent 基座而不是真 runtime：本用例要守的是**传输/连接模式**语义，
// 起真 Agent 需要模型与 provider，会把本用例变成环境依赖。

const STATIC_TOKEN = "web-loopback-static-token";
const STATIC_COOKIE = { cookie: `mode_lite_token=${encodeURIComponent(STATIC_TOKEN)}` };
const WORKSPACE_PATH = "/vol1/data/demo";
const WORKSPACE_IDENTITY = "remote:lan:127.0.0.1:45880:/vol1/data/demo";
const SUBSCRIPTION_ID = "sub-e2e";

function createMemoryCredentials() {
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
  };
}

/** 与 lan-access-host.test.ts 同款：监听必须在 await open 之前挂上，避免丢 Initialize。 */
function wrapClientSocket(ws: WebSocket): ISocket {
  const onData = new Emitter<VSBuffer>();
  const onClose = new Emitter<void>();
  const onEnd = new Emitter<void>();
  ws.on("message", (data: unknown, isBinary: boolean) => {
    if (!isBinary) return;
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
      if (ws.readyState === WebSocket.OPEN) ws.send(buffer.buffer, { binary: true });
    },
    end: () => ws.close(),
    drain: () => Promise.resolve(),
    dispose: () => ws.close(),
  };
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

const delay = (ms: number): Promise<void> =>
  new Promise((resolveDelay) => setTimeout(resolveDelay, ms));

interface FakeAgentHarness {
  service: IModeAgentServiceShape;
  fireSessionsFrame(): void;
  fireCuaObservation(): void;
}

/**
 * 假 agent 基座：只实现本用例会触发的成员。
 *
 * `subscribeSessionsIndexV4` 在返回 ACK **之前**把 initial 帧推给 `onDynamicSessionsIndexFrame`，
 * 复现宿主「先发 snapshot、后回 ACK」的真实时序；connection facade 会把 pending 帧按 ACK 归属
 * 释放给订阅者（这就是 replayable 的初始回放路径）。
 */
function createFakeAgent(): FakeAgentHarness {
  const sessionsFrames = new Emitter<SessionsIndexTopicWireCandidate>();
  const cuaObservations = new Emitter<Record<string, unknown>>();
  const service = {
    // facade 会覆盖这两个方法；基座只需提供可枚举的键（ProxyChannel 按 for...in 建频道）。
    helloConversationV4: async () => {
      throw new Error("facade should answer hello");
    },
    initializeConversationV4: async () => {
      throw new Error("facade should answer initialize");
    },
    onDynamicSessionsIndexFrame: () => sessionsFrames.event,
    onDynamicCuaPermissionObservation: () => cuaObservations.event,
    subscribeSessionsIndexV4: async (params: {
      workspacePath: string;
      workspaceIdentity?: string;
    }) => {
      const workspaceKey = params.workspaceIdentity?.trim() || params.workspacePath;
      sessionsFrames.fire({
        wireVersion: V4_WIRE_PROTOCOL_VERSION,
        kind: "complete",
        deliveryKind: "initial",
        logicalFrameId: "lf-e2e-1",
        logicalFrameOrdinal: 1,
        topic: `sessions-index/${workspaceKey}`,
        subscriptionId: SUBSCRIPTION_ID,
        frame: { seq: 1, snapshot: { sessions: [] } },
      });
      return { ack: { subscriptionId: SUBSCRIPTION_ID, mode: "snapshot", logEpoch: "epoch-e2e" } };
    },
    unsubscribeSessionsIndexV4: async () => undefined,
  };
  return {
    service: service as unknown as IModeAgentServiceShape,
    fireSessionsFrame: () =>
      sessionsFrames.fire({
        wireVersion: V4_WIRE_PROTOCOL_VERSION,
        kind: "complete",
        deliveryKind: "online",
        logicalFrameId: "lf-e2e-2",
        logicalFrameOrdinal: 2,
        topic: `sessions-index/${WORKSPACE_IDENTITY}`,
        subscriptionId: SUBSCRIPTION_ID,
        frame: { seq: 2, snapshot: { sessions: [] } },
      }),
    fireCuaObservation: () => cuaObservations.fire({ reason: "e2e" }),
  };
}

async function startFixture() {
  const credentials = createMemoryCredentials();
  let listeningPort = 0;
  const host = await createLanAccessHost({
    credentials,
    port: () => listeningPort,
    name: "web-loopback",
  });
  const agent = createFakeAgent();
  const services = new ServiceCollection().register(IModeAgentService, agent.service);
  let activeWebSessions = 0;
  const server = createHttpServer(services, 0, {
    authToken: STATIC_TOKEN,
    authRequired: true,
    verifyAuthToken: (presented) => presented === STATIC_TOKEN || host.verifyToken(presented),
    lanPairing: { redeem: (request) => host.redeem(request) },
    lanAccessHost: host,
    resolveLanClientByToken: (token) => host.resolveClientId(token),
    onLanClientWorkspace: (clientId, workspacePath) =>
      host.recordClientWorkspace(clientId, workspacePath),
    serverId: host.getServerId(),
    name: "web-loopback",
    onWebSessionChange: (delta) => {
      activeWebSessions = Math.max(0, activeWebSessions + delta);
    },
  });
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  listeningPort = (server.address() as AddressInfo).port;
  return {
    host,
    agent,
    port: listeningPort,
    base: `http://127.0.0.1:${listeningPort}`,
    activeWebSessions: () => activeWebSessions,
    async close() {
      (server as unknown as { closeAllConnections?: () => void }).closeAllConnections?.();
      await new Promise<void>((resolve) => server.close(() => resolve()));
      await delay(100);
    },
  };
}

async function postJson(
  base: string,
  path: string,
  body: unknown,
  headers: Record<string, string> = STATIC_COOKIE,
): Promise<Response> {
  return fetch(`${base}${path}`, {
    method: "POST",
    headers: { ...headers, "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

async function enableAndPair(fx: Awaited<ReturnType<typeof startFixture>>): Promise<string> {
  const enabled = await postJson(fx.base, "/api/lan/enable", { enabled: true });
  assert.equal(enabled.status, 200);
  const codeResponse = await postJson(fx.base, "/api/lan/pair-code", {});
  assert.equal(codeResponse.status, 200);
  const state = lanAccessStateSchema.safeParse(await codeResponse.json());
  assert.ok(state.success, "配对码响应应为合法 LanAccessState");
  assert.ok(state.data.pairCode, "开启态应签发配对码");

  const paired = await postJson(
    fx.base,
    "/api/lan/pair",
    { code: state.data.pairCode.code, label: "web-loopback" },
    {},
  );
  assert.equal(paired.status, 200, "配对码应一次性兑换成功");
  const payload = (await paired.json()) as { token?: string };
  assert.ok(payload.token, "配对响应必须带设备令牌");
  return payload.token!;
}

test("Web 单机回环：配对 → /ws replayable 握手 → 初始帧回放；continuous-only 事件被拒", async () => {
  const fx = await startFixture();
  const token = await enableAndPair(fx);
  const lanCookie = { cookie: `mode_lite_token=${encodeURIComponent(token)}` };
  const ws = new WebSocket(`ws://127.0.0.1:${fx.port}/ws`, { headers: lanCookie });
  // 监听必须在 await open **之前**挂上：服务端在 onOpen 里立刻发 Initialize，
  // open 兑现后再挂监听可能错过首帧（与 lan-access-host.test 的同一纪律）。
  const client = new ChannelClient(new SocketProtocol(wrapClientSocket(ws)));
  const agent = client.getChannel<IChannel>("mode-agent");
  try {
    await withTimeout(
      new Promise<void>((resolve, reject) => {
        ws.once("open", () => resolve());
        ws.once("error", reject);
      }),
      5_000,
      "/ws open",
    );
    assert.equal(fx.activeWebSessions(), 1, "已鉴权的 /ws 连接必须计入在线 web 会话");

    // 1) replayable 握手：hello 由 connection facade 生成，profile 与 clientMode 必须配对。
    const hello = await withTimeout(
      agent.call<{ clientMode: string; deliveryProfile: string; connectionId: string }>(
        "helloConversationV4",
        [],
      ),
      5_000,
      "helloConversationV4",
    );
    assert.equal(hello.clientMode, "web-remote-replayable");
    assert.equal(hello.deliveryProfile, "replayable", "replayable 连接必须声明 replayable profile");
    assert.ok(hello.connectionId.length > 0);

    await withTimeout(
      agent.call("initializeConversationV4", [
        {
          kind: "clientHello",
          protocolVersion: V4_WIRE_PROTOCOL_VERSION,
          clientId: "web-loopback-e2e",
          clientKind: "web",
          appVersion: "0.0.0-test",
        },
      ]),
      5_000,
      "initializeConversationV4",
    );

    // 2) 订阅先装、再 subscribe：initial 帧是「响应之后」的有主通知，早订阅才能收到。
    const workspace = { workspacePath: WORKSPACE_PATH, workspaceIdentity: WORKSPACE_IDENTITY };
    const frames: SessionsIndexTopicWireCandidate[] = [];
    const framesEvent = agent.listen<SessionsIndexTopicWireCandidate>(
      "onDynamicSessionsIndexFrame",
      workspace,
    );
    framesEvent((frame) => frames.push(frame));

    const subscribed = await withTimeout(
      agent.call<{ ack: { subscriptionId: string; mode: string } }>("subscribeSessionsIndexV4", [
        workspace,
      ]),
      5_000,
      "subscribeSessionsIndexV4",
    );
    assert.equal(subscribed.ack.subscriptionId, SUBSCRIPTION_ID);
    assert.equal(subscribed.ack.mode, "snapshot", "无 base 水位必须是 snapshot 回放");

    await delay(50);
    assert.equal(frames.length, 1, `订阅应回放 initial 帧，实际 ${JSON.stringify(frames)}`);
    assert.equal(frames[0]?.topic, `sessions-index/${WORKSPACE_IDENTITY}`);
    assert.equal(frames[0]?.subscriptionId, SUBSCRIPTION_ID);
    assert.equal(frames[0]?.kind, "complete");

    // 3) continuous-only 事件：facade 对 replayable 返回 Event.None，基座即使发射也不得到达。
    const denied: unknown[] = [];
    const deniedEvent = agent.listen("onDynamicCuaPermissionObservation", {});
    deniedEvent((event) => denied.push(event));
    fx.agent.fireCuaObservation();
    await delay(100);
    assert.deepEqual(denied, [], "replayable 连接不得消费 continuous-only 的 CUA 权限观察");

    // 4) 同一 workspace 的 live 帧仍应到达（拒绝的是事件族，不是整条连接）。
    const before = frames.length;
    fx.agent.fireSessionsFrame();
    await delay(50);
    assert.equal(frames.length, before + 1, "owned sessions-index 的 live 帧必须继续投递");

    client.dispose();
  } finally {
    ws.close();
    await delay(150);
    assert.equal(fx.activeWebSessions(), 0, "连接关闭必须把在线 web 会话数收回 0");
    await fx.close();
  }
});
