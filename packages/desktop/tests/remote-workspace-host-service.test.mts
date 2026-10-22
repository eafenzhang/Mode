/**
 * LR-4：远程工作区 Host 适配器单测（假 LAN transport + 真 scoped attachment 路由）。
 *
 * 守：connect 事件/descriptor、bind 后 attach 的 scope 与三元组、scoped 服务真能经端口调用、
 * detach 回收、session dispose 释放 transport。真实 LAN HTTP/WS 与两台机器验收不在单测内。
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  ChannelClient,
  MessagePortProtocol,
  VSBuffer,
  createFrameDemultiplexer,
  type IChannel,
  type IMessagePassingProtocol,
} from "@mode/rpc";
import {
  IClientConfigService,
  ICredentialService,
  ServiceCollection,
  createServiceDescriptor,
  type IServiceAccessor,
} from "@mode/services";

import type { WindowRemoteConnectionHandle } from "../src/host/windowRemoteConnectionRegistry.js";
import { createRemoteWorkspaceHostService } from "../src/host-standalone/remoteWorkspaceService.js";
import { createScopedAttachmentRegistry } from "../src/host-standalone/scopedAttachments.js";

const delay = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

function transportPair(): { server: IMessagePassingProtocol; client: IMessagePassingProtocol } {
  const aListeners = new Set<(buffer: VSBuffer) => void>();
  const bListeners = new Set<(buffer: VSBuffer) => void>();
  const server: IMessagePassingProtocol = {
    send: (buffer) => {
      for (const listener of aListeners) listener(buffer);
    },
    onMessage: (listener) => {
      bListeners.add(listener);
      return { dispose: () => bListeners.delete(listener) };
    },
  };
  const client: IMessagePassingProtocol = {
    send: (buffer) => {
      for (const listener of bListeners) listener(buffer);
    },
    onMessage: (listener) => {
      aListeners.add(listener);
      return { dispose: () => aListeners.delete(listener) };
    },
  };
  return { server, client };
}

interface EchoService {
  echo(arg: unknown): Promise<unknown>;
  fail(): Promise<never>;
}

const TestEcho = createServiceDescriptor<EchoService>("test-echo");

function createRemoteServices(scopeId: string): ServiceCollection {
  const services = new ServiceCollection();
  services.register(TestEcho, {
    async echo(arg: unknown) {
      return { scopeId, arg };
    },
    async fail() {
      throw new Error("boom");
    },
  });
  return services;
}

test("connect → bind → attach → scoped 调用 → detach/dispose", async () => {
  const transport = transportPair();
  const demux = createFrameDemultiplexer(transport.server);
  const scopedAttachments = createScopedAttachmentRegistry({
    demux,
    log: () => undefined,
  });
  let handleDisposed = 0;
  const closeState: {
    listener: ((event: { exitCode: number | null; signal: string | null }) => void) | null;
  } = { listener: null };
  const handle: WindowRemoteConnectionHandle<ServiceCollection, { computerUse?: boolean }> = {
    services: createRemoteServices("s1"),
    capabilities: { computerUse: true },
    onDidClose(listener) {
      closeState.listener = listener;
      return {
        dispose: () => {
          closeState.listener = null;
        },
      };
    },
    async dispose() {
      handleDisposed += 1;
    },
  };
  const adapter = await createRemoteWorkspaceHostService({
    services: new ServiceCollection(),
    log: () => undefined,
    scopedAttachments,
    guardChannel: (_channelName, channel) => channel,
    connectLan: async () => handle,
    createId: (() => {
      let next = 0;
      return () => `id-${++next}`;
    })(),
  });
  // Host 内部生命周期方法不得进入 ProxyChannel 的 RPC 面。
  assert.equal(Object.keys(adapter).includes("disposeHost"), false);
  try {
    const events: string[] = [];
    const subscription = adapter.onDynamicConnectionEvent()((event) => {
      if (event.kind === "log") events.push(`log:${event.entry.level}`);
      else events.push(event.kind);
    });
    const connected = await adapter.connect({
      target: { kind: "lan", host: "127.0.0.1", port: 45880 },
      requestId: "request-1",
      workspacePath: "/w",
      workspaceIdentity: "remote:lan:127.0.0.1:45880:/w",
    });
    assert.equal(connected.ok, true);
    const sessionId = connected.ok ? connected.descriptor.remoteSessionId : "";
    assert.ok(sessionId);
    assert.ok(events.includes("connected"));
    assert.ok(events.includes("log:info"));

    const session = await adapter.getSession({ sessionId });
    assert.equal(session?.state, "online");
    assert.equal(session?.workspacePath, "/w");
    assert.deepEqual(
      (await adapter.listSessions()).map((entry) => entry.remoteSessionId),
      [sessionId],
    );

    const attachment = await adapter.attach({ sessionId });
    assert.equal(attachment.sessionId, sessionId);
    assert.deepEqual(attachment.capabilities, { computerUse: true });

    // 页面端口就绪：客户端先订阅，再 start（Initialize 不能被未知 scope 丢掉）。
    const clientDemux = createFrameDemultiplexer(transport.client);
    const client = new ChannelClient(clientDemux.attachScope(attachment.scopeId));
    scopedAttachments.start(attachment.scopeId);
    const echo = await client
      .getChannel<IChannel>("test-echo")
      .call<{ scopeId: string }>("echo", [{ n: 1 }]);
    assert.deepEqual(echo, { scopeId: "s1", arg: { n: 1 } });
    await assert.rejects(client.getChannel<IChannel>("test-echo").call("fail"), /boom/u);

    // bind 换代：新 context 下 attach 得到新 scope；旧 attachment 由上层释放。
    await adapter.bindContext({ sessionId, workspacePath: "/w2" });
    const rebound = await adapter.attach({ sessionId });
    assert.notEqual(rebound.scopeId, attachment.scopeId);

    await adapter.detach({ attachmentId: attachment.attachmentId });
    assert.equal(scopedAttachments.count(), 1, "只回收被 detach 的那个 scope");
    await adapter.detach({ attachmentId: "missing" });

    // 连接关闭事件 + session dispose 释放 transport。
    closeState.listener?.({ exitCode: 1, signal: null });
    await delay(10);
    assert.ok(events.includes("closed"));
    await adapter.dispose({ sessionId });
    assert.equal(handleDisposed, 1);
    assert.deepEqual(await adapter.listSessions(), []);

    subscription.dispose();
  } finally {
    await adapter.disposeHost();
  }
});

test("SSH/WSL/Docker 目标走 connectStdio，不再被 Tauri Host 拒绝", async () => {
  const transport = transportPair();
  const demux = createFrameDemultiplexer(transport.server);
  const scopedAttachments = createScopedAttachmentRegistry({
    demux,
    log: () => undefined,
  });
  let receivedKind = "";
  let receivedRemoteAssets: unknown = null;
  const handle: WindowRemoteConnectionHandle<ServiceCollection, { computerUse?: boolean }> = {
    services: createRemoteServices("ssh"),
    async dispose() {},
  };
  const adapter = await createRemoteWorkspaceHostService({
    services: new ServiceCollection(),
    log: () => undefined,
    scopedAttachments,
    guardChannel: (_channelName, channel) => channel,
    connectStdio: async (target, remoteAssets) => {
      receivedKind = target.kind;
      receivedRemoteAssets = remoteAssets;
      return handle;
    },
    remoteAssets: { remoteCacheDir: "C:/tmp/mode-remote-assets" },
    createId: (() => {
      let next = 0;
      return () => `stdio-${++next}`;
    })(),
  });
  try {
    const result = await adapter.connect({
      target: {
        kind: "ssh",
        host: "example.test",
        port: 22,
        username: "dev",
      },
      workspacePath: "/home/dev/proj",
      workspaceIdentity: "remote:ssh:example.test:22:dev:/home/dev/proj",
    });
    assert.equal(result.ok, true);
    assert.equal(receivedKind, "ssh");
    assert.deepEqual(receivedRemoteAssets, { remoteCacheDir: "C:/tmp/mode-remote-assets" });
  } finally {
    await adapter.disposeHost();
  }
});

test("scoped services 的 guard 与事件：listen 抛错不逃逸", async () => {
  // 该用例只验证 guardChannel 被用于 scoped 频道：传入一个把 listen 包成抛错的 guard，
  // 事件订阅应被 Host 的 channel guard 拦住，而不是带走进程。这里直接断言 guard 被调用。
  const transport = transportPair();
  const demux = createFrameDemultiplexer(transport.server);
  const scopedAttachments = createScopedAttachmentRegistry({ demux, log: () => undefined });
  const guarded: string[] = [];
  const adapter = await createRemoteWorkspaceHostService({
    services: new ServiceCollection(),
    log: () => undefined,
    scopedAttachments,
    guardChannel: (channelName, channel) => {
      guarded.push(channelName);
      return channel;
    },
    connectLan: async () => ({
      services: createRemoteServices("s2"),
      onDidClose: () => ({ dispose() {} }),
      dispose: async () => undefined,
    }),
    createId: () => "id-guard",
  });
  try {
    await adapter.connect({
      target: { kind: "lan", host: "127.0.0.1", port: 1 },
      workspacePath: "/w",
      workspaceIdentity: "remote:lan:127.0.0.1:1:/w",
    });
    const session = (await adapter.listSessions())[0];
    assert.ok(session);
    await adapter.attach({ sessionId: session.remoteSessionId });
    assert.ok(guarded.includes("test-echo"), "scoped 频道必须经过同一份 listen 守卫");
  } finally {
    await adapter.disposeHost();
  }
});

test("连接失败：返回可读错误并发出 connectFailed", async () => {
  const transport = transportPair();
  const demux = createFrameDemultiplexer(transport.server);
  const scopedAttachments = createScopedAttachmentRegistry({ demux, log: () => undefined });
  const adapter = await createRemoteWorkspaceHostService({
    services: new ServiceCollection(),
    log: () => undefined,
    scopedAttachments,
    guardChannel: (_name, channel) => channel,
    connectLan: async () => {
      throw new Error("对端离线");
    },
  });
  const events: string[] = [];
  const subscription = adapter.onDynamicConnectionEvent()((event) => {
    events.push(event.kind);
  });
  try {
    const result = await adapter.connect({
      target: { kind: "lan", host: "127.0.0.1", port: 1 },
      requestId: "request-fail",
    });
    assert.equal(result.ok, false);
    assert.match(result.ok ? "" : result.error, /对端离线/u);
    assert.ok(events.includes("connectFailed"));
  } finally {
    subscription.dispose();
    await adapter.disposeHost();
  }
});

test("stale session / 未绑定 workspace 的 attach 明确失败", async () => {
  const transport = transportPair();
  const demux = createFrameDemultiplexer(transport.server);
  const scopedAttachments = createScopedAttachmentRegistry({ demux, log: () => undefined });
  const adapter = await createRemoteWorkspaceHostService({
    services: new ServiceCollection(),
    log: () => undefined,
    scopedAttachments,
    guardChannel: (_name, channel) => channel,
    connectLan: async () => ({
      services: createRemoteServices("stale"),
      onDidClose: () => ({ dispose() {} }),
      dispose: async () => undefined,
    }),
    createId: () => "id-stale",
  });
  try {
    await assert.rejects(adapter.attach({ sessionId: "missing" }), /未找到/u);
    await assert.rejects(
      adapter.bindContext({ sessionId: "missing", workspacePath: "/w" }),
      /未找到/u,
    );
    const result = await adapter.connect({
      target: { kind: "lan", host: "127.0.0.1", port: 1 },
    });
    assert.equal(result.ok, true);
    const sessionId = result.ok ? result.descriptor.remoteSessionId : "";
    // connect 未带 workspacePath：bind 之前 attach 必须明确失败，而不是拿空 scope。
    await assert.rejects(adapter.attach({ sessionId }), /尚未绑定上下文/u);
  } finally {
    await adapter.disposeHost();
  }
});

test("连接关闭：活动 attachment 被回收，之后 attach 明确失败", async () => {
  const transport = transportPair();
  const demux = createFrameDemultiplexer(transport.server);
  const scopedAttachments = createScopedAttachmentRegistry({ demux, log: () => undefined });
  const closeState: {
    listener: ((event: { exitCode: number | null; signal: string | null }) => void) | null;
  } = { listener: null };
  const adapter = await createRemoteWorkspaceHostService({
    services: new ServiceCollection(),
    log: () => undefined,
    scopedAttachments,
    guardChannel: (_name, channel) => channel,
    connectLan: async () => ({
      services: createRemoteServices("closing"),
      onDidClose(listener) {
        closeState.listener = listener;
        return { dispose: () => (closeState.listener = null) };
      },
      dispose: async () => undefined,
    }),
    createId: () => "id-closing",
  });
  try {
    const result = await adapter.connect({
      target: { kind: "lan", host: "127.0.0.1", port: 1 },
      workspacePath: "/w",
      workspaceIdentity: "remote:lan:127.0.0.1:1:/w",
    });
    assert.equal(result.ok, true);
    const sessionId = result.ok ? result.descriptor.remoteSessionId : "";
    await adapter.attach({ sessionId });
    assert.equal(scopedAttachments.count(), 1);
    closeState.listener?.({ exitCode: 1, signal: null });
    await delay(20);
    assert.equal(scopedAttachments.count(), 0, "连接关闭必须回收 scoped attachment");
    await assert.rejects(adapter.attach({ sessionId }), /不可用|未找到/u);
  } finally {
    await adapter.disposeHost();
  }
});

test("默认 LAN 连接：WS close 必须经 onDidClose 上报为 closed", async () => {
  // 回归：defaultConnectLan 曾漏传 onClose，断链后 renderer 永远停在「已连接」，
  // modelSelectionService.getView() 等 RPC 永久 pending（表现为「正在加载模型…」）。
  const transport = transportPair();
  const demux = createFrameDemultiplexer(transport.server);
  const scopedAttachments = createScopedAttachmentRegistry({ demux, log: () => undefined });
  const services = new ServiceCollection();
  services.register(IClientConfigService, {} as IClientConfigService);
  services.register(ICredentialService, {
    load: async () => null,
    save: async () => undefined,
    delete: async () => undefined,
    list: async () => [],
  } as unknown as ICredentialService);

  let captured: { onClose?: () => void } | null = null;
  let attachDisposed = 0;
  const connectionServices = {
    modeAgentService: {},
    fileService: {},
  } as unknown as IServiceAccessor;
  const adapter = await createRemoteWorkspaceHostService({
    services,
    log: () => undefined,
    scopedAttachments,
    guardChannel: (_name, channel) => channel,
    attachLan: async (params) => {
      captured = params;
      return {
        services: connectionServices,
        info: { capabilities: { computerUse: true } } as never,
        dispose: async () => {
          attachDisposed += 1;
        },
      };
    },
    createRemoteWorkspaceServices: () => new ServiceCollection(),
    createId: () => "id-default-close",
  });
  const events: string[] = [];
  const subscription = adapter.onDynamicConnectionEvent()((event) => {
    if (event.kind !== "log") events.push(event.kind);
  });
  try {
    const result = await adapter.connect({
      target: {
        kind: "lan",
        host: "127.0.0.1",
        port: 45880,
        serverId: "peer-1",
        token: "token-1",
      },
      workspacePath: "/w",
      workspaceIdentity: "remote:lan:127.0.0.1:45880:/w",
    });
    assert.equal(result.ok, true);
    const onClose = (captured as { onClose?: () => void } | null)?.onClose;
    assert.equal(typeof onClose, "function", "WS 关闭必须接到 handle");
    onClose?.();
    await delay(20);
    assert.ok(events.includes("closed"), "WS 关闭必须产出 closed 事件");
    const sessionId = result.ok ? result.descriptor.remoteSessionId : "";
    assert.equal((await adapter.getSession({ sessionId }))?.state, "disconnected");
  } finally {
    subscription.dispose();
    await adapter.disposeHost();
  }
  assert.equal(attachDisposed, 1, "Host 退出必须释放底层连接");
});

test("默认 LAN 连接：公网端点优先，attach 收到 https + path", async () => {
  const transport = transportPair();
  const demux = createFrameDemultiplexer(transport.server);
  const scopedAttachments = createScopedAttachmentRegistry({ demux, log: () => undefined });
  const services = new ServiceCollection();
  services.register(IClientConfigService, {} as IClientConfigService);
  services.register(ICredentialService, {
    load: async () => null,
    save: async () => undefined,
    delete: async () => undefined,
    list: async () => [],
  } as unknown as ICredentialService);
  let captured: { scheme?: "http" | "https"; host?: string; path?: string; token?: string } | null =
    null;
  const adapter = await createRemoteWorkspaceHostService({
    services,
    log: () => undefined,
    scopedAttachments,
    guardChannel: (_name, channel) => channel,
    attachLan: async (params) => {
      captured = params;
      return {
        services: { modeAgentService: {}, fileService: {} } as unknown as IServiceAccessor,
        info: { capabilities: {} } as never,
        dispose: async () => undefined,
      };
    },
    createRemoteWorkspaceServices: () => new ServiceCollection(),
    createId: () => "id-public-first",
  });
  try {
    const result = await adapter.connect({
      target: {
        kind: "lan",
        host: "192.168.1.20",
        port: 45880,
        serverId: "peer-1",
        token: "token-1",
        scheme: "https",
        path: "/d/tunnel-1",
        endpoints: [
          {
            scheme: "https",
            host: "relay.example.com",
            port: 443,
            kind: "public",
            path: "/d/tunnel-1",
          },
          { scheme: "http", host: "192.168.1.20", port: 45880, kind: "lan" },
        ],
      },
      workspacePath: "/w",
      workspaceIdentity: "remote:lan:192.168.1.20:45880:/w",
    });
    assert.equal(result.ok, true);
    const cap = captured as {
      scheme?: "http" | "https";
      host?: string;
      path?: string;
    } | null;
    assert.deepEqual(
      { scheme: cap?.scheme, host: cap?.host, path: cap?.path },
      { scheme: "https", host: "relay.example.com", path: "/d/tunnel-1" },
    );
  } finally {
    await adapter.disposeHost();
  }
});

test("Host 退出：活动会话与 scoped attachment 一并回收", async () => {
  const transport = transportPair();
  const demux = createFrameDemultiplexer(transport.server);
  const scopedAttachments = createScopedAttachmentRegistry({ demux, log: () => undefined });
  let disposed = 0;
  const adapter = await createRemoteWorkspaceHostService({
    services: new ServiceCollection(),
    log: () => undefined,
    scopedAttachments,
    guardChannel: (_name, channel) => channel,
    connectLan: async () => ({
      services: createRemoteServices("exit"),
      onDidClose: () => ({ dispose() {} }),
      dispose: async () => {
        disposed += 1;
      },
    }),
    createId: () => "id-exit",
  });
  const result = await adapter.connect({
    target: { kind: "lan", host: "127.0.0.1", port: 1 },
    workspacePath: "/w",
    workspaceIdentity: "remote:lan:127.0.0.1:1:/w",
  });
  assert.equal(result.ok, true);
  const sessionId = result.ok ? result.descriptor.remoteSessionId : "";
  await adapter.attach({ sessionId });
  await adapter.disposeHost();
  assert.equal(disposed, 1, "Host 退出必须释放底层 transport");
  assert.equal(scopedAttachments.count(), 0, "Host 退出必须回收 scoped attachment");
});

test("Bot 远端桥：状态按 target+identity 命中；重连复用/新建并广播；runtime port 真能 RPC", async () => {
  // spec: docs/specs/im-bot-remote-workspace-binding.md + 迁移计划 §5.4（botRemote.reconnected）。
  // Tauri Host 里 registry 与 BotsService 同进程，Bot 桥不再经 parentPort 转发给 main。
  const transport = transportPair();
  const demux = createFrameDemultiplexer(transport.server);
  const scopedAttachments = createScopedAttachmentRegistry({ demux, log: () => undefined });
  let connectCount = 0;
  const adapter = await createRemoteWorkspaceHostService({
    services: new ServiceCollection(),
    log: () => undefined,
    scopedAttachments,
    guardChannel: (_channelName, channel) => channel,
    connectLan: async () => {
      connectCount += 1;
      return {
        services: createRemoteServices("bot"),
        onDidClose: () => ({ dispose() {} }),
        dispose: async () => undefined,
      };
    },
    // Bot runtime 端口只需证明「远端服务集合经端口可调」；用测试 echo 频道代替真实服务。
    createRemoteWorkspaceServices: () => createRemoteServices("bot"),
    createId: (() => {
      let next = 0;
      return () => `bot-${++next}`;
    })(),
  });
  try {
    const lanTarget = { kind: "lan" as const, host: "127.0.0.1", port: 45880 };
    const workspaceIdentity = "remote:lan:127.0.0.1:45880:/w";
    const connected = await adapter.connect({
      target: lanTarget,
      workspacePath: "/w",
      workspaceIdentity,
    });
    assert.equal(connected.ok, true);
    const sessionId = connected.ok ? connected.descriptor.remoteSessionId : "";
    const botTarget = { workspacePath: "/w", workspaceIdentity, target: lanTarget };

    assert.equal(await adapter.queryBotConnectionStatus(botTarget), true);
    assert.equal(
      await adapter.queryBotConnectionStatus({ ...botTarget, workspaceIdentity: "other" }),
      false,
      "identity 不同不得命中（同 target 的另一个 workspace）",
    );
    assert.equal(
      await adapter.queryBotConnectionStatus({
        ...botTarget,
        target: { kind: "lan", host: "127.0.0.1", port: 1 },
      }),
      false,
      "target 不同不得命中",
    );

    const events: string[] = [];
    const subscription = adapter.onDynamicConnectionEvent()((event) => events.push(event.kind));
    // 已有 online session：复用，不新建、不广播 botReconnected。
    const reused = await adapter.reconnectBotWorkspace(botTarget);
    assert.deepEqual(reused, { ok: true, sessionId });
    assert.equal(connectCount, 1, "复用不应重新建立底层连接");
    assert.equal(events.includes("botReconnected"), false, "复用不应广播重连事件");

    // 同一 target 的另一个 workspace：新建 logical session 并广播。
    const otherIdentity = "remote:lan:127.0.0.1:45880:/w2";
    const reconnected = await adapter.reconnectBotWorkspace({
      workspacePath: "/w2",
      workspaceIdentity: otherIdentity,
      target: lanTarget,
    });
    assert.equal(reconnected.ok, true);
    assert.notEqual(reconnected.ok ? reconnected.sessionId : "", sessionId);
    assert.equal(events.includes("botReconnected"), true, "新建连接必须广播 botReconnected");
    assert.equal((await adapter.listSessions()).length, 2);

    // runtime port：远端服务集合经进程内 MessageChannel 真能 RPC。
    const runtime = await adapter.createBotRuntimePort(botTarget);
    assert.equal(runtime.ok, true);
    if (!runtime.ok) {
      throw new Error("runtime port 未创建");
    }
    // Node 的 MessagePort 与浏览器同形（addEventListener/postMessage/start/close）。
    const client = new ChannelClient(new MessagePortProtocol(runtime.port as never));
    const echo = await client
      .getChannel<IChannel>("test-echo")
      .call<{ scopeId: string }>("echo", [{ n: 1 }]);
    assert.deepEqual(echo, { scopeId: "bot", arg: { n: 1 } });
    runtime.dispose();
    // 未连接/未绑定的 workspace 明确失败，不返回半个端口。
    const missing = await adapter.createBotRuntimePort({
      workspacePath: "/missing",
      workspaceIdentity: "missing",
      target: lanTarget,
    });
    assert.deepEqual(missing, { ok: false, error: "远端 workspace 尚未连接或未绑定上下文。" });

    subscription.dispose();
  } finally {
    await adapter.disposeHost();
  }
});
