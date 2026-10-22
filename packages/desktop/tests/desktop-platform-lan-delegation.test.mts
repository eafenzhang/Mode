/**
 * LR-1 / LR-2：`desktopPlatform` 的 LAN 与远程桌面设置面委托。
 *
 * 守五件事：
 * 1. Tauri runtime 且有 Host 服务时，LAN/RD 调用走 Host，**不碰** `window.mode`；
 * 2. 非 Tauri（Electron）时保持既有 `window.mode` 路径；
 * 3. 服务 getter 是**调用时**解析的（端口晚于 `createDesktopPlatform` 到达）；
 * 4. `removeLanPairedPeer` / `renameLanPairedPeer` 在 Tauri 且无 Host 服务时显式拒绝，
 *    不允许回落到注入层的静默 no-op；
 * 5. 主控会话（rdStartSession / 信号 / 剪贴板 / 事件）仍走壳，不被 LR-2 提前接管。
 */
import assert from "node:assert/strict";
import test from "node:test";

import type {
  ILanAccessService,
  IRemoteDesktopService,
  IRemoteService,
  IServiceAccessor,
} from "@mode/services";
import type { LanAccessState, LanPairedPeer } from "@mode/shared";
import type { RemoteWorkspaceConnectionEvent } from "@mode/services";
import type {
  RemoteDesktopPeerCapability,
  RemoteDesktopServerState,
  RemoteDesktopSessionEvent,
} from "@mode/shared/remote-desktop";

const HOST_STATE: LanAccessState = {
  enabled: true,
  port: 45880,
  addresses: ["192.168.1.2:45880"],
  pairCode: null,
  clients: [],
};
const MOCK_STATE: LanAccessState = {
  enabled: false,
  port: null,
  addresses: [],
  pairCode: null,
  clients: [],
};

const HOST_RD_STATE: RemoteDesktopServerState = {
  enabled: true,
  authorizedClients: [{ id: "client-a", label: "Office-PC" }],
  encoderCapability: "available",
};
const MOCK_RD_STATE: RemoteDesktopServerState = {
  enabled: false,
  authorizedClients: [],
  encoderCapability: "unknown",
};
const HOST_PEER: RemoteDesktopPeerCapability = {
  online: true,
  remoteDesktop: true,
  remoteDesktopNative: true,
  name: "Peer",
};

const calls: string[] = [];

const fakeMode = {
  getLanAccessState: async () => {
    calls.push("mode.getLanAccessState");
    return MOCK_STATE;
  },
  setLanAccessEnabled: async () => {
    calls.push("mode.setLanAccessEnabled");
    return MOCK_STATE;
  },
  createLanAccessPairCode: async () => MOCK_STATE,
  removeLanAccessClient: async () => MOCK_STATE,
  resetLanAccessTokens: async () => MOCK_STATE,
  discoverLanPeers: async () => [],
  pairLanPeer: async () => ({ token: "t", serverId: "s" }),
  getLanPairedPeers: async () => [],
  getLanPeerWorkspaces: async () => ({ workspaces: [] }),
  removeLanPairedPeer: async () => {
    calls.push("mode.removeLanPairedPeer");
  },
  renameLanPairedPeer: async () => {
    calls.push("mode.renameLanPairedPeer");
  },
  rdGetPeerCapability: async (serverId: string) => {
    calls.push(`mode.rdGetPeerCapability:${serverId}`);
    return { online: false, remoteDesktop: false, remoteDesktopNative: false };
  },
  rdStartSession: async (serverId: string) => {
    calls.push(`mode.rdStartSession:${serverId}`);
    return { ok: true as const };
  },
  rdGetControlledState: async () => {
    calls.push("mode.rdGetControlledState");
    return { controlled: false };
  },
  rdSendSignal: () => {
    calls.push("mode.rdSendSignal");
    return Promise.resolve();
  },
  rdEndSession: async () => {
    calls.push("mode.rdEndSession");
  },
  rdReadClipboardText: async () => {
    calls.push("mode.rdReadClipboardText");
    return "clip";
  },
  rdGetServerState: async () => {
    calls.push("mode.rdGetServerState");
    return MOCK_RD_STATE;
  },
  rdSetServerEnabled: async () => {
    calls.push("mode.rdSetServerEnabled");
    return MOCK_RD_STATE;
  },
  rdRevokeClient: async () => {
    calls.push("mode.rdRevokeClient");
    return MOCK_RD_STATE;
  },
  rdAuthorizeClient: async () => {
    calls.push("mode.rdAuthorizeClient");
    return MOCK_RD_STATE;
  },
  onRemoteDesktopSessionEvent: () => ({ dispose() {} }),
  connectRemote: async () => {
    calls.push("mode.connectRemote");
    return { success: true, sessionId: "mode-session" };
  },
  cancelPendingRemoteConnection: async () => {
    calls.push("mode.cancelPendingRemoteConnection");
  },
  bindRemoteWorkspaceSessionContext: async () => {
    calls.push("mode.bindRemoteWorkspaceSessionContext");
  },
  disposeRemoteSession: async () => {
    calls.push("mode.disposeRemoteSession");
  },
  onRemoteConnectionLog: () => () => {},
  onRemoteSessionClosed: () => () => {},
  onBotRemoteWorkspaceReconnected: () => () => {},
};

const fakeLanAccess = {
  getState: async () => {
    calls.push("host.getState");
    return HOST_STATE;
  },
  setEnabled: async ({ enabled }: { enabled: boolean }) => {
    calls.push(`host.setEnabled:${String(enabled)}`);
    return HOST_STATE;
  },
  createPairCode: async () => HOST_STATE,
  removeClient: async ({ clientId }: { clientId: string }) => {
    calls.push(`host.removeClient:${clientId}`);
    return HOST_STATE;
  },
  resetTokens: async () => HOST_STATE,
  discoverPeers: async () => [],
  pairPeer: async () => ({ token: "t", serverId: "s" }),
  listPairedPeers: async () => [] as LanPairedPeer[],
  getPeerWorkspaces: async ({ serverId }: { serverId: string }) => {
    calls.push(`host.getPeerWorkspaces:${serverId}`);
    return { workspaces: [] };
  },
  removePairedPeer: async ({ serverId }: { serverId: string }) => {
    calls.push(`host.removePairedPeer:${serverId}`);
  },
  renamePairedPeer: async ({ serverId, name }: { serverId: string; name: string }) => {
    calls.push(`host.renamePairedPeer:${serverId}:${name}`);
  },
  onDidChange: () => ({ dispose() {} }),
} satisfies ILanAccessService;

function fakeServices(
  lanAccessService: ILanAccessService | undefined,
  remoteDesktopService?: IRemoteDesktopService,
  remoteService?: IRemoteService,
): IServiceAccessor {
  return {
    lanAccessService,
    remoteDesktopService,
    remoteService,
  } as unknown as IServiceAccessor;
}

const sessionListeners = new Set<(event: RemoteDesktopSessionEvent) => void>();

function emitSessionEvent(event: RemoteDesktopSessionEvent): void {
  for (const listener of sessionListeners) {
    listener(event);
  }
}

const fakeRemoteDesktop = {
  getServerState: async () => {
    calls.push("host.rd.getServerState");
    return HOST_RD_STATE;
  },
  setServerEnabled: async ({ enabled }: { enabled: boolean }) => {
    calls.push(`host.rd.setServerEnabled:${String(enabled)}`);
    return HOST_RD_STATE;
  },
  authorizeClient: async ({ clientId }: { clientId: string }) => {
    calls.push(`host.rd.authorizeClient:${clientId}`);
    return HOST_RD_STATE;
  },
  revokeClient: async ({ clientId }: { clientId: string }) => {
    calls.push(`host.rd.revokeClient:${clientId}`);
    return HOST_RD_STATE;
  },
  getPeerCapability: async ({ serverId }: { serverId: string }) => {
    calls.push(`host.rd.getPeerCapability:${serverId}`);
    return HOST_PEER;
  },
  getControlledState: async () => {
    calls.push("host.rd.getControlledState");
    return { controlled: true };
  },
  startSession: async ({ serverId }: { serverId: string }) => {
    calls.push(`host.rd.startSession:${serverId}`);
    return { ok: true as const };
  },
  sendSignal: async ({ message }: { message: { type: string } }) => {
    calls.push(`host.rd.sendSignal:${message.type}`);
  },
  endSession: async (params?: { reason?: "user-end" | "error" }) => {
    calls.push(`host.rd.endSession:${params?.reason ?? "none"}`);
  },
  respondConsent: async ({ clientId }: { clientId: string }) => {
    calls.push(`host.rd.respondConsent:${clientId}`);
  },
  onDynamicSessionEvent: () => (listener: (event: RemoteDesktopSessionEvent) => void) => {
    sessionListeners.add(listener);
    return {
      dispose() {
        sessionListeners.delete(listener);
      },
    };
  },
  onConsentRequest: () => ({ dispose() {} }),
  onDidChangeServerState: () => ({ dispose() {} }),
} satisfies IRemoteDesktopService;

const remoteListeners = new Set<(event: RemoteWorkspaceConnectionEvent) => void>();
let nextScopeId = 20;
const bridgeCalls: string[] = [];
const remoteEvents = {
  emit(event: RemoteWorkspaceConnectionEvent): void {
    for (const listener of remoteListeners) listener(event);
  },
};

const fakeRemoteService = {
  connect: async (params: {
    target: { kind: string };
    requestId?: string;
    workspacePath?: string;
    workspaceIdentity?: string;
  }) => {
    calls.push(
      `host.remote.connect:${params.target.kind}:${params.requestId ?? ""}:${params.workspacePath ?? ""}:${params.workspaceIdentity ?? ""}`,
    );
    return {
      ok: true as const,
      descriptor: {
        remoteSessionId: "session-1",
        target: params.target as never,
        ...(params.workspacePath ? { workspacePath: params.workspacePath } : {}),
        ...(params.workspaceIdentity ? { workspaceIdentity: params.workspaceIdentity } : {}),
        generation: 1,
      },
    };
  },
  cancelConnect: async ({ requestId }: { requestId: string }) => {
    calls.push(`host.remote.cancel:${requestId}`);
  },
  bindContext: async (params: { sessionId: string; workspacePath: string }) => {
    calls.push(`host.remote.bind:${params.sessionId}:${params.workspacePath}`);
  },
  dispose: async ({ sessionId }: { sessionId: string }) => {
    calls.push(`host.remote.dispose:${sessionId}`);
  },
  attach: async ({ sessionId }: { sessionId: string }) => {
    const scopeId = nextScopeId++;
    const attachmentId = `attachment-${scopeId}`;
    calls.push(`host.remote.attach:${sessionId}:${scopeId}`);
    return { scopeId, attachmentId, sessionId, capabilities: { computerUse: true } };
  },
  detach: async ({ attachmentId }: { attachmentId: string }) => {
    calls.push(`host.remote.detach:${attachmentId}`);
  },
  getSession: async ({ sessionId }: { sessionId: string }) => ({
    remoteSessionId: sessionId,
    requestId: "request-1",
    target: { kind: "lan", host: "192.168.1.9", port: 45880 },
    workspacePath: "/w",
    workspaceIdentity: "remote:lan:192.168.1.9:45880:/w",
    generation: 1,
    state: "online" as const,
    sourceAvailability: "online" as const,
  }),
  listSessions: async () => [],
  onDynamicConnectionEvent: () => (listener: (event: RemoteWorkspaceConnectionEvent) => void) => {
    remoteListeners.add(listener);
    return {
      dispose() {
        remoteListeners.delete(listener);
      },
    };
  },
} satisfies IRemoteService;

const fakeBridge = {
  createScopedServicePort(options: {
    scopeId: number;
    attachmentId: string;
    sessionId: string;
    target: { kind: string };
  }) {
    bridgeCalls.push(
      `create:${options.scopeId}:${options.attachmentId}:${options.sessionId}:${options.target.kind}`,
    );
    return true;
  },
  releaseScopedServicePort(scopeId: number) {
    bridgeCalls.push(`release:${scopeId}`);
    return true;
  },
};

// `desktopBrowserPlatformBridge` 在模块求值期读取 window.mode，必须在 import 之前装好。
const globalScope = globalThis as unknown as {
  window?: { mode: typeof fakeMode; __MODE_HOST_BRIDGE__?: typeof fakeBridge };
  __TAURI__?: unknown;
};
globalScope.window = { mode: fakeMode, __MODE_HOST_BRIDGE__: fakeBridge };

const { createDesktopPlatform } = await import("../src/renderer/src/desktopPlatform.js");

function setTauriRuntime(enabled: boolean): void {
  if (enabled) {
    globalScope.__TAURI__ = {};
  } else {
    delete globalScope.__TAURI__;
  }
}

test("Tauri + Host 服务：LAN 调用委托 Host，不碰 window.mode", async () => {
  calls.length = 0;
  setTauriRuntime(true);
  const platform = createDesktopPlatform({
    isLocalDevelopmentRuntime: false,
    getHostServices: () => fakeServices(fakeLanAccess),
  });

  assert.deepEqual(await platform.getLanAccessState(), HOST_STATE);
  assert.equal(await platform.setLanAccessEnabled(true), HOST_STATE);
  assert.equal(await platform.removeLanAccessClient("client-1"), HOST_STATE);
  await platform.removeLanPairedPeer("server-1");
  await platform.renameLanPairedPeer("server-2", "Desk");
  await platform.getLanPeerWorkspaces("server-3");
  assert.deepEqual(calls, [
    "host.getState",
    "host.setEnabled:true",
    "host.removeClient:client-1",
    "host.removePairedPeer:server-1",
    "host.renamePairedPeer:server-2:Desk",
    "host.getPeerWorkspaces:server-3",
  ]);
  assert.equal(
    calls.some((call) => call.startsWith("mode.")),
    false,
  );
});

test("非 Tauri（Electron）：保持 window.mode 路径", async () => {
  calls.length = 0;
  setTauriRuntime(false);
  const platform = createDesktopPlatform({
    isLocalDevelopmentRuntime: false,
    getHostServices: () => fakeServices(fakeLanAccess),
  });
  assert.deepEqual(await platform.getLanAccessState(), MOCK_STATE);
  await platform.removeLanPairedPeer("server-1");
  assert.deepEqual(calls, ["mode.getLanAccessState", "mode.removeLanPairedPeer"]);
});

test("服务 getter 是调用时解析：端口晚到后委托立即生效", async () => {
  calls.length = 0;
  setTauriRuntime(true);
  let services: IServiceAccessor | null = null;
  const platform = createDesktopPlatform({
    isLocalDevelopmentRuntime: false,
    getHostServices: () => services,
  });
  services = fakeServices(fakeLanAccess);
  assert.deepEqual(await platform.getLanAccessState(), HOST_STATE);
  assert.deepEqual(calls, ["host.getState"]);
});

test("Tauri 且无 Host 服务：删除/重命名显式拒绝，不回落成静默 no-op", async () => {
  calls.length = 0;
  setTauriRuntime(true);
  const platform = createDesktopPlatform({
    isLocalDevelopmentRuntime: false,
    getHostServices: () => null,
  });
  await assert.rejects(platform.removeLanPairedPeer("server-1"), /Host 服务不可用/u);
  await assert.rejects(platform.renameLanPairedPeer("server-1", "x"), /Host 服务不可用/u);
  assert.equal(calls.includes("mode.removeLanPairedPeer"), false);
  assert.equal(calls.includes("mode.renameLanPairedPeer"), false);
});

test("Tauri + Host：远程桌面设置面与主控会话委托 Host，剪贴板仍走壳", async () => {
  calls.length = 0;
  setTauriRuntime(true);
  const platform = createDesktopPlatform({
    isLocalDevelopmentRuntime: false,
    getHostServices: () => fakeServices(fakeLanAccess, fakeRemoteDesktop),
  });

  assert.deepEqual(await platform.rdGetServerState(), HOST_RD_STATE);
  assert.equal(await platform.rdSetServerEnabled(false), HOST_RD_STATE);
  assert.equal(await platform.rdAuthorizeClient("client-a"), HOST_RD_STATE);
  assert.equal(await platform.rdRevokeClient("client-a"), HOST_RD_STATE);
  assert.deepEqual(await platform.rdGetPeerCapability("server-1"), HOST_PEER);
  assert.deepEqual(await platform.rdGetControlledState(), { controlled: true });
  assert.deepEqual(calls, [
    "host.rd.getServerState",
    "host.rd.setServerEnabled:false",
    "host.rd.authorizeClient:client-a",
    "host.rd.revokeClient:client-a",
    "host.rd.getPeerCapability:server-1",
    "host.rd.getControlledState",
  ]);

  // LR-3：主控会话面迁到 Host；剪贴板读取仍是壳能力。
  calls.length = 0;
  assert.deepEqual(await platform.rdStartSession("server-1"), { ok: true });
  platform.rdSendSignal({ type: "bye" });
  await platform.rdEndSession("user-end");
  await platform.rdReadClipboardText();
  assert.deepEqual(calls, [
    "host.rd.startSession:server-1",
    "host.rd.sendSignal:bye",
    "host.rd.endSession:user-end",
    "mode.rdReadClipboardText",
  ]);
});

test("Tauri + Host：会话事件订阅走 Host 动态事件，disposer 生效", async () => {
  calls.length = 0;
  setTauriRuntime(true);
  const platform = createDesktopPlatform({
    isLocalDevelopmentRuntime: false,
    getHostServices: () => fakeServices(fakeLanAccess, fakeRemoteDesktop),
  });
  const received: RemoteDesktopSessionEvent[] = [];
  const dispose = platform.onRemoteDesktopSessionEvent((event) => received.push(event));
  emitSessionEvent({ kind: "state", state: "connected" });
  assert.deepEqual(received, [{ kind: "state", state: "connected" }]);
  dispose();
  emitSessionEvent({ kind: "state", state: "closed" });
  assert.equal(received.length, 1, "disposer 后不再收事件");
});

test("非 Tauri：远程桌面设置面保持 window.mode 路径", async () => {
  calls.length = 0;
  setTauriRuntime(false);
  const platform = createDesktopPlatform({
    isLocalDevelopmentRuntime: false,
    getHostServices: () => fakeServices(fakeLanAccess, fakeRemoteDesktop),
  });
  assert.deepEqual(await platform.rdGetServerState(), MOCK_RD_STATE);
  await platform.rdSetServerEnabled(false);
  await platform.rdAuthorizeClient("client-a");
  await platform.rdRevokeClient("client-a");
  await platform.rdGetPeerCapability("server-1");
  await platform.rdGetControlledState();
  await platform.rdStartSession("server-1");
  platform.rdSendSignal({ type: "bye" });
  await platform.rdEndSession("user-end");
  assert.deepEqual(calls, [
    "mode.rdGetServerState",
    "mode.rdSetServerEnabled",
    "mode.rdAuthorizeClient",
    "mode.rdRevokeClient",
    "mode.rdGetPeerCapability:server-1",
    "mode.rdGetControlledState",
    "mode.rdStartSession:server-1",
    "mode.rdSendSignal",
    "mode.rdEndSession",
  ]);
});

test("Tauri + Host：远程工作区 connect/bind/dispose 委托 Host 并投递 scoped port", async () => {
  calls.length = 0;
  bridgeCalls.length = 0;
  setTauriRuntime(true);
  const platform = createDesktopPlatform({
    isLocalDevelopmentRuntime: false,
    getHostServices: () => fakeServices(fakeLanAccess, fakeRemoteDesktop, fakeRemoteService),
  });
  const target = { kind: "lan" as const, host: "192.168.1.9", port: 45880 };
  const identity = "remote:lan:192.168.1.9:45880:/w";

  assert.deepEqual(
    await platform.connectRemote(target, "request-1", {
      workspacePath: "/w",
      workspaceIdentity: identity,
    }),
    { success: true, sessionId: "session-1" },
  );
  assert.deepEqual(bridgeCalls, ["create:20:attachment-20:session-1:lan"]);

  // bind 换代：先建新 scope，再释放旧 scope。
  await platform.bindRemoteWorkspaceSessionContext({
    remoteSessionId: "session-1",
    workspacePath: "/w2",
  });
  assert.deepEqual(bridgeCalls, [
    "create:20:attachment-20:session-1:lan",
    "create:21:attachment-21:session-1:lan",
    "release:20",
  ]);

  await platform.cancelPendingRemoteConnection("request-1");
  await platform.disposeRemoteSession("session-1");
  assert.deepEqual(bridgeCalls, [
    "create:20:attachment-20:session-1:lan",
    "create:21:attachment-21:session-1:lan",
    "release:20",
    "release:21",
  ]);
  assert.deepEqual(calls, [
    `host.remote.connect:lan:request-1:/w:${identity}`,
    "host.remote.attach:session-1:20",
    "host.remote.bind:session-1:/w2",
    "host.remote.attach:session-1:21",
    "host.remote.detach:attachment-20",
    "host.remote.cancel:request-1",
    "host.remote.detach:attachment-21",
    "host.remote.dispose:session-1",
  ]);
});

test("Tauri + Host：远程连接日志与关闭事件走 Host 动态事件", async () => {
  calls.length = 0;
  setTauriRuntime(true);
  const platform = createDesktopPlatform({
    isLocalDevelopmentRuntime: false,
    getHostServices: () => fakeServices(fakeLanAccess, fakeRemoteDesktop, fakeRemoteService),
  });
  const logs: Array<{ message: string }> = [];
  const closed: Array<{ sessionId: string }> = [];
  const reconnected: Array<{ sessionId: string; workspaceIdentity: string }> = [];
  const disposeLog = platform.onRemoteConnectionLog((entry) => logs.push(entry));
  const disposeClosed = platform.onRemoteSessionClosed((event) => closed.push(event));
  const disposeReconnected = platform.onBotRemoteWorkspaceReconnected((event) =>
    reconnected.push(event),
  );

  remoteEvents.emit({
    kind: "log",
    entry: {
      label: "remote",
      level: "info",
      source: "test",
      message: "hello",
      timestamp: "2026-10-19T00:00:00.000Z",
    },
  });
  remoteEvents.emit({
    kind: "closed",
    sessionId: "session-1",
    exitCode: 1,
    signal: null,
  });
  remoteEvents.emit({
    kind: "botReconnected",
    event: {
      sessionId: "bot-session",
      workspacePath: "/w",
      workspaceIdentity: "remote:lan:192.168.1.9:45880:/w",
      target: { kind: "lan", host: "192.168.1.9", port: 45880 },
    },
  });
  assert.deepEqual(logs, [
    {
      label: "remote",
      level: "info",
      source: "test",
      message: "hello",
      timestamp: "2026-10-19T00:00:00.000Z",
    },
  ]);
  assert.deepEqual(closed, [
    { sessionId: "session-1", reason: "host-exit", exitCode: 1, signal: null },
  ]);
  assert.deepEqual(reconnected, [
    {
      sessionId: "bot-session",
      workspacePath: "/w",
      workspaceIdentity: "remote:lan:192.168.1.9:45880:/w",
      target: { kind: "lan", host: "192.168.1.9", port: 45880 },
    },
  ]);

  disposeLog();
  disposeClosed();
  disposeReconnected();
  remoteEvents.emit({ kind: "closed", sessionId: "session-2", exitCode: null, signal: null });
  remoteEvents.emit({
    kind: "botReconnected",
    event: {
      sessionId: "bot-session-2",
      workspacePath: "/w",
      workspaceIdentity: "remote:lan:192.168.1.9:45880:/w",
      target: { kind: "lan", host: "192.168.1.9", port: 45880 },
    },
  });
  assert.equal(closed.length, 1, "disposer 后不再收事件");
  assert.equal(reconnected.length, 1, "bot reconnect 订阅同样在 disposer 后停止");
});

test("非 Tauri：远程工作区保持 window.mode 路径", async () => {
  calls.length = 0;
  setTauriRuntime(false);
  const platform = createDesktopPlatform({
    isLocalDevelopmentRuntime: false,
    getHostServices: () => fakeServices(fakeLanAccess, fakeRemoteDesktop, fakeRemoteService),
  });
  const target = { kind: "lan" as const, host: "192.168.1.9", port: 45880 };
  assert.deepEqual(await platform.connectRemote(target, "request-1"), {
    success: true,
    sessionId: "mode-session",
  });
  await platform.cancelPendingRemoteConnection("request-1");
  await platform.bindRemoteWorkspaceSessionContext({
    remoteSessionId: "mode-session",
    workspacePath: "/w",
  });
  await platform.disposeRemoteSession("mode-session");
  assert.deepEqual(calls, [
    "mode.connectRemote",
    "mode.cancelPendingRemoteConnection",
    "mode.bindRemoteWorkspaceSessionContext",
    "mode.disposeRemoteSession",
  ]);
});
