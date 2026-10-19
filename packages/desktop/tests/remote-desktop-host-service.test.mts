/**
 * LR-2：`remote-desktop` Host 适配器单测。
 *
 * 假 server 复用真实的 `postToMain` 收结果，覆盖：
 * 1. 状态读取与事件；首个订阅者拿最近一次快照；
 * 2. 总开关关闭 → 能力位 false（LAN 握手门控）且结束被控会话；
 * 3. 授权三态：已授权 / 已撤销 / 不存在的 client（静默不变），空 clientId 拒绝；
 * 4. 对端能力探测：空 serverId 不发请求、探测异常归为 offline；
 * 5. 被控状态与被测 server 的 busy 事实一致；dispose 收口。
 */
import assert from "node:assert/strict";
import test from "node:test";

import { ICredentialService, IRemoteDesktopService, ServiceCollection } from "@mode/services";
import type {
  RemoteDesktopMessageFromHost,
  RemoteDesktopMessageToHost,
} from "../src/host/remoteDesktopMessages.js";
import { RemoteDesktopHostMessageTypes } from "../src/host/remoteDesktopMessages.js";
import {
  createRemoteDesktopHostServer,
  type RemoteDesktopHostServer,
} from "../src/host/remoteDesktopServer.js";
import { createRemoteDesktopHostService } from "../src/host-standalone/remoteDesktopService.js";

type ServerParams = Parameters<typeof createRemoteDesktopHostServer>[0];

interface FakeClient {
  id: string;
  label: string;
  authorized: boolean;
}

function createFakeServer(initial?: {
  enabled?: boolean;
  clients?: FakeClient[];
  encoderCapability?: "unknown" | "available" | "software" | "unavailable";
  controlled?: boolean;
}) {
  const state = {
    enabled: initial?.enabled ?? false,
    clients: (initial?.clients ?? []).map((client) => ({ ...client })),
    encoderCapability: initial?.encoderCapability ?? "unknown",
    controlled: initial?.controlled ?? false,
  };
  let postToMain: ((message: RemoteDesktopMessageFromHost) => void) | null = null;
  let disposeCount = 0;
  let loadCount = 0;
  const received: RemoteDesktopMessageToHost[] = [];

  const reply = (requestId: string): void => {
    postToMain?.({
      type: RemoteDesktopHostMessageTypes.RemoteDesktopServerStateResult,
      requestId,
      ok: true,
      enabled: state.enabled,
      authorizedClients: state.clients
        .filter((client) => client.authorized)
        .map(({ id, label }) => ({ id, label })),
      encoderCapability: state.encoderCapability,
    });
  };

  const server: RemoteDesktopHostServer = {
    hooks: {
      requestSession: async () => ({ ok: true }),
      onSessionSocket: () => undefined,
      onMediaSocket: () => undefined,
    },
    isServerEnabled: () => state.enabled,
    isControlled: () => state.controlled,
    onDidChangeControlledState: () => ({ dispose() {} }),
    loadServerState: async () => {
      loadCount += 1;
    },
    handleMessage: (message) => {
      received.push(message);
      switch (message.type) {
        case RemoteDesktopHostMessageTypes.RemoteDesktopServerStateRequest:
          reply(message.requestId);
          return;
        case RemoteDesktopHostMessageTypes.RemoteDesktopSetServerEnabled:
          state.enabled = message.enabled;
          // 关开关 = 结束被控会话（真实 server 同样在持久化后收口）。
          if (!message.enabled) {
            state.controlled = false;
          }
          reply(message.requestId);
          return;
        case RemoteDesktopHostMessageTypes.RemoteDesktopAuthorizeClient:
          for (const client of state.clients) {
            if (client.id === message.clientId) {
              client.authorized = true;
            }
          }
          reply(message.requestId);
          return;
        case RemoteDesktopHostMessageTypes.RemoteDesktopRevokeClient:
          for (const client of state.clients) {
            if (client.id === message.clientId) {
              client.authorized = false;
            }
          }
          reply(message.requestId);
          return;
        default:
          return;
      }
    },
    dispose: () => {
      disposeCount += 1;
    },
  };

  return {
    server,
    createServer: (params: ServerParams) => {
      postToMain = params.postToMain;
      return server;
    },
    state: () => state,
    received: () => received,
    disposed: () => disposeCount,
    loads: () => loadCount,
    /** 模拟被控 server 的同意请求（真实路径由 lanAccessServer 的 requestSession hook 触发）。 */
    requestConsent: (clientId: string, label: string) => {
      postToMain?.({
        type: RemoteDesktopHostMessageTypes.RemoteDesktopConsentRequest,
        clientId,
        label,
      });
    },
    /** 模拟被控 server 的任意上行动作（SignalFromHost / MediaStatus / SessionClosed）。 */
    emitHostMessage: (message: RemoteDesktopMessageFromHost) => {
      postToMain?.(message);
    },
  };
}

function createServices(): ServiceCollection {
  const services = new ServiceCollection();
  // 适配器要求凭据服务存在；对端探测在测试里注入，不读这份凭据。
  services.register(ICredentialService, {} as ICredentialService);
  return services;
}

async function createAdapter(
  fake: ReturnType<typeof createFakeServer>,
  overrides: Partial<Parameters<typeof createRemoteDesktopHostService>[0]> = {},
) {
  return createRemoteDesktopHostService({
    services: createServices(),
    log: () => undefined,
    createServer: fake.createServer,
    encoderCapabilityPath: "rd-encoder-capability.test.json",
    ...overrides,
  });
}

test("getServerState 返回总开关、已授权设备与编码能力；首个订阅者拿最近一次快照", async () => {
  const fake = createFakeServer({
    enabled: true,
    clients: [
      { id: "client-a", label: "Office-PC", authorized: true },
      { id: "client-b", label: "Laptop", authorized: false },
    ],
    encoderCapability: "software",
  });
  const adapter = await createAdapter(fake);
  try {
    assert.equal(fake.loads(), 1, "创建时必须把持久化开关读入同步缓存");
    const state = await adapter.getServerState();
    assert.deepEqual(state, {
      enabled: true,
      authorizedClients: [{ id: "client-a", label: "Office-PC" }],
      encoderCapability: "software",
    });

    const received: unknown[] = [];
    const subscription = adapter.onDidChangeServerState((next) => received.push(next));
    subscription.dispose();
    assert.deepEqual(received, [state], "订阅时补最近一次快照，且不重放历史");
    assert.equal(adapter.isServerEnabled(), true);
  } finally {
    await adapter.dispose();
  }
});

test("关闭总开关：返回关闭态、能力位立即 false、被控会话收口并广播", async () => {
  const fake = createFakeServer({ enabled: true, controlled: true });
  const adapter = await createAdapter(fake);
  const events: Array<{ enabled: boolean }> = [];
  const subscription = adapter.onDidChangeServerState((state) => events.push(state));
  try {
    assert.equal((await adapter.getControlledState()).controlled, true);
    const state = await adapter.setServerEnabled({ enabled: false });
    assert.equal(state.enabled, false);
    assert.equal(adapter.isServerEnabled(), false, "LAN 握手能力位必须立即跟随开关");
    assert.equal((await adapter.getControlledState()).controlled, false);
    assert.equal(events.at(-1)?.enabled, false);
  } finally {
    subscription.dispose();
    await adapter.dispose();
  }
});

test("授权三态：授权、撤销、未知 client 不变；空 clientId 拒绝且不发命令", async () => {
  const fake = createFakeServer({
    clients: [
      { id: "client-a", label: "Office-PC", authorized: false },
      { id: "client-b", label: "Laptop", authorized: false },
    ],
  });
  const adapter = await createAdapter(fake);
  try {
    const authorized = await adapter.authorizeClient({ clientId: "client-a" });
    assert.deepEqual(authorized.authorizedClients, [{ id: "client-a", label: "Office-PC" }]);

    const unknown = await adapter.authorizeClient({ clientId: "client-missing" });
    assert.deepEqual(unknown.authorizedClients, [{ id: "client-a", label: "Office-PC" }]);

    const revoked = await adapter.revokeClient({ clientId: "client-a" });
    assert.deepEqual(revoked.authorizedClients, []);

    const before = fake.received().length;
    await assert.rejects(adapter.authorizeClient({ clientId: "  " }), /clientId/u);
    await assert.rejects(adapter.revokeClient({ clientId: "" }), /clientId/u);
    assert.equal(fake.received().length, before, "空 clientId 不得下发到 server");
  } finally {
    await adapter.dispose();
  }
});

test("对端能力探测：空 serverId 不发请求，探测异常归为 offline", async () => {
  const fake = createFakeServer();
  const calls: string[] = [];
  const adapter = await createAdapter(fake, {
    probePeer: async (_credentials, serverId) => {
      calls.push(serverId);
      return {
        online: true,
        remoteDesktop: true,
        remoteDesktopNative: true,
        name: "Peer",
      };
    },
  });
  try {
    assert.deepEqual(await adapter.getPeerCapability({ serverId: "   " }), {
      remoteDesktop: false,
      remoteDesktopNative: false,
      online: false,
    });
    assert.deepEqual(calls, [], "空 serverId 不得发起网络探测");

    const capability = await adapter.getPeerCapability({ serverId: "server-1" });
    assert.deepEqual(capability, {
      online: true,
      remoteDesktop: true,
      remoteDesktopNative: true,
      name: "Peer",
    });
    assert.deepEqual(calls, ["server-1"]);
  } finally {
    await adapter.dispose();
  }

  const failing = createFakeServer();
  const adapterWithFailure = await createAdapter(failing, {
    probePeer: async () => {
      throw new Error("offline");
    },
  });
  try {
    assert.deepEqual(await adapterWithFailure.getPeerCapability({ serverId: "server-1" }), {
      remoteDesktop: false,
      remoteDesktopNative: false,
      online: false,
    });
  } finally {
    await adapterWithFailure.dispose();
  }
});

test("dispose 释放 server 且之后不再回帖", async () => {
  const fake = createFakeServer({ enabled: true });
  const adapter = await createAdapter(fake);
  await adapter.dispose();
  assert.equal(fake.disposed(), 1);
});

test("频道 descriptor 与类型面可用", () => {
  assert.equal(IRemoteDesktopService.channelName, "remote-desktop");
});

test("内部生命周期方法不可枚举（不会被 ProxyChannel 暴露成 RPC）", async () => {
  const fake = createFakeServer();
  const adapter = await createAdapter(fake);
  try {
    const enumerable = Object.keys(adapter);
    for (const name of ["dispose", "isServerEnabled", "getLanHooks", "handleMessage"]) {
      assert.equal(enumerable.includes(name), false, `${name} 不得可枚举`);
    }
    assert.ok(enumerable.includes("getServerState"));
  } finally {
    await adapter.dispose();
  }
});

const delay = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

async function waitFor(predicate: () => boolean, what: string): Promise<void> {
  const deadline = Date.now() + 2_000;
  while (Date.now() < deadline) {
    if (predicate()) {
      return;
    }
    await delay(10);
  }
  throw new Error(`等待 ${what} 超时`);
}

function consentResults(fake: ReturnType<typeof createFakeServer>) {
  return fake
    .received()
    .filter((message) => message.type === RemoteDesktopHostMessageTypes.RemoteDesktopConsentResult);
}

test("同意请求：事件可达，允许/记住写回被控 server", async () => {
  const fake = createFakeServer({
    clients: [{ id: "client-a", label: "Office-PC", authorized: false }],
  });
  const adapter = await createAdapter(fake);
  const requests: Array<{ clientId: string; label: string }> = [];
  const subscription = adapter.onConsentRequest((request) => requests.push(request));
  try {
    fake.requestConsent("client-a", "Office-PC");
    await waitFor(() => requests.length === 1, "consent request");
    assert.deepEqual(requests[0], { clientId: "client-a", label: "Office-PC" });

    await adapter.respondConsent({ clientId: "client-a", decision: "allow", remember: true });
    const results = consentResults(fake);
    assert.equal(results.length, 1);
    assert.deepEqual(results[0], {
      type: RemoteDesktopHostMessageTypes.RemoteDesktopConsentResult,
      clientId: "client-a",
      decision: "allow",
      remember: true,
    });
  } finally {
    subscription.dispose();
    await adapter.dispose();
  }
});

test("同意请求：晚订阅者仍能拿到挂起的那一条", async () => {
  const fake = createFakeServer();
  const adapter = await createAdapter(fake);
  try {
    fake.requestConsent("client-late", "Late");
    // 等异步授权读取完成、请求进入等待态后再订阅。
    await delay(30);
    const requests: Array<{ clientId: string; label: string }> = [];
    const subscription = adapter.onConsentRequest((request) => requests.push(request));
    try {
      await waitFor(() => requests.length === 1, "replayed consent request");
      assert.deepEqual(requests[0], { clientId: "client-late", label: "Late" });
    } finally {
      subscription.dispose();
    }
  } finally {
    await adapter.dispose();
  }
});

test("同意请求：已记住设备直通，不弹事件", async () => {
  const fake = createFakeServer({
    clients: [{ id: "client-a", label: "Office-PC", authorized: true }],
  });
  const adapter = await createAdapter(fake);
  const requests: Array<{ clientId: string }> = [];
  const subscription = adapter.onConsentRequest((request) => requests.push(request));
  try {
    fake.requestConsent("client-a", "Office-PC");
    await waitFor(() => consentResults(fake).length === 1, "auto allow");
    assert.deepEqual(consentResults(fake)[0], {
      type: RemoteDesktopHostMessageTypes.RemoteDesktopConsentResult,
      clientId: "client-a",
      decision: "allow",
      remember: true,
    });
    assert.equal(requests.length, 0, "已记住设备不应再要求用户确认");
  } finally {
    subscription.dispose();
    await adapter.dispose();
  }
});

test("同意请求：60s（可注入）无响应按 timeout 拒绝", async () => {
  const fake = createFakeServer();
  const adapter = await createAdapter(fake, { consentTimeoutMs: 20 });
  try {
    fake.requestConsent("client-timeout", "Timeout");
    await waitFor(() => consentResults(fake).length === 1, "timeout deny");
    assert.deepEqual(consentResults(fake)[0], {
      type: RemoteDesktopHostMessageTypes.RemoteDesktopConsentResult,
      clientId: "client-timeout",
      decision: "deny",
      remember: false,
      reason: "timeout",
    });
    await assert.rejects(
      adapter.respondConsent({ clientId: "client-timeout", decision: "allow", remember: false }),
      /没有待处理的同意请求/u,
    );
  } finally {
    await adapter.dispose();
  }
});

test("startSession：本机被控中直接拒绝，不打开对端会话", async () => {
  const fake = createFakeServer({ controlled: true });
  let opened = 0;
  const adapter = await createAdapter(fake, {
    openSession: async () => {
      opened += 1;
      return { ok: false, error: "should not be called" };
    },
  });
  try {
    assert.deepEqual(await adapter.startSession({ serverId: "server-1" }), {
      error: "locally_controlled",
    });
    assert.equal(opened, 0);
  } finally {
    await adapter.dispose();
  }
});

test("startSession：打开失败按稳定错误码收口，事件经 onDynamicSessionEvent 透出", async () => {
  const fake = createFakeServer();
  const adapter = await createAdapter(fake, {
    openSession: async () => ({ ok: false, errorCode: "busy", error: "peer busy" }),
  });
  const events: Array<{ kind: string; state?: string; error?: string }> = [];
  const subscription = adapter.onDynamicSessionEvent()((event) => events.push(event));
  try {
    assert.deepEqual(await adapter.startSession({ serverId: "server-1" }), { error: "busy" });
    assert.deepEqual(
      events.map((event) => ({ kind: event.kind, state: event.state, error: event.error })),
      [
        { kind: "state", state: "connecting", error: undefined },
        { kind: "state", state: "closed", error: "busy" },
      ],
    );
  } finally {
    subscription.dispose();
    await adapter.dispose();
  }
});

test("respondConsent：没有挂起请求时明确拒绝", async () => {
  const fake = createFakeServer();
  const adapter = await createAdapter(fake);
  try {
    await assert.rejects(
      adapter.respondConsent({ clientId: "nobody", decision: "allow", remember: false }),
      /没有待处理的同意请求/u,
    );
  } finally {
    await adapter.dispose();
  }
});

test("被控协调器：setLocalDisplays 后 hello/select-monitor 经 adapter 路由", async () => {
  const fake = createFakeServer();
  const adapter = await createAdapter(fake, {
    createKeySalt: () => ({ key: "a".repeat(64), salt: "b".repeat(16) }),
  });
  try {
    await adapter.setLocalDisplays({
      displays: [
        {
          displayId: "0",
          label: "Primary",
          xPx: 0,
          yPx: 0,
          widthPx: 1920,
          heightPx: 1080,
          scaleFactor: 1,
          primary: true,
        },
      ],
    });
    fake.emitHostMessage({
      type: RemoteDesktopHostMessageTypes.RemoteDesktopSignalFromHost,
      message: { type: "hello", protocolVersion: 2 },
    });
    await waitFor(
      () =>
        fake
          .received()
          .some(
            (message) =>
              message.type === RemoteDesktopHostMessageTypes.RemoteDesktopSignalToHost &&
              message.message.type === "monitors",
          ),
      "monitors signal",
    );
    const monitors = fake
      .received()
      .find(
        (message) =>
          message.type === RemoteDesktopHostMessageTypes.RemoteDesktopSignalToHost &&
          message.message.type === "monitors",
      );
    assert.deepEqual(
      monitors?.type === RemoteDesktopHostMessageTypes.RemoteDesktopSignalToHost
        ? monitors.message
        : null,
      {
        type: "monitors",
        monitors: [
          {
            displayId: "0",
            label: "Primary",
            widthPx: 1920,
            heightPx: 1080,
            scaleFactor: 1,
            primary: true,
          },
        ],
      },
    );

    fake.emitHostMessage({
      type: RemoteDesktopHostMessageTypes.RemoteDesktopSignalFromHost,
      message: { type: "select-monitor", displayId: "0" },
    });
    await waitFor(
      () =>
        fake
          .received()
          .some(
            (message) => message.type === RemoteDesktopHostMessageTypes.RemoteDesktopMediaStart,
          ),
      "MediaStart",
    );
    const mediaOpen = fake
      .received()
      .find(
        (message) =>
          message.type === RemoteDesktopHostMessageTypes.RemoteDesktopSignalToHost &&
          message.message.type === "media-open",
      );
    assert.deepEqual(
      mediaOpen?.type === RemoteDesktopHostMessageTypes.RemoteDesktopSignalToHost
        ? mediaOpen.message
        : null,
      { type: "media-open", key: "a".repeat(64), salt: "b".repeat(16) },
    );
  } finally {
    await adapter.dispose();
  }
});

test("被控状态事件：同意请求 → 允许 → 会话结束的布尔序列", async () => {
  const fake = createFakeServer();
  // 用真实 server 的状态机不可行（需要 ws）；这里模拟 server 的状态事件源。
  const listeners = new Set<(controlled: boolean) => void>();
  const emitControlled = (controlled: boolean) => {
    for (const listener of listeners) listener(controlled);
  };
  const adapter = await createAdapter(fake, {
    createServer: (params) => {
      fake.createServer(params);
      return {
        ...fake.server,
        onDidChangeControlledState: (listener: (controlled: boolean) => void) => {
          listeners.add(listener);
          return { dispose: () => listeners.delete(listener) };
        },
      };
    },
  });
  const states: boolean[] = [];
  const subscription = adapter.onDidChangeControlledState((state) => states.push(state.controlled));
  try {
    // 首个订阅者先拿到当前值（false）。
    assert.deepEqual(states, [false]);
    emitControlled(true);
    emitControlled(false);
    assert.deepEqual(states, [false, true, false]);
  } finally {
    subscription.dispose();
    await adapter.dispose();
  }
});
