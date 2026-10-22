/* eslint-disable max-lines -- LAN/SSH/WSL/Docker 共用同一 registry、workspace bind 与 attachment 生命周期，拆分会让单 owner 状态机跨文件。 */
import { randomUUID } from "node:crypto";
import { MessageChannel } from "node:worker_threads";

import {
  ChannelServer,
  Emitter,
  MessagePortProtocol,
  type IServerChannel,
  type MessagePortLike,
} from "@mode/rpc";
import {
  buildRemoteWorkspaceIdentity,
  isSameRemoteTarget,
  MODE_VERSION,
  stripRemoteTargetSecrets,
  type BotRemoteWorkspaceReconnectedEvent,
  type ModePromptAttachment,
  type RemoteConnectionRuntimeLog,
  type RemoteTarget,
  type TraceId,
} from "@mode/shared";
import {
  IClientConfigService,
  ICredentialService,
  IRemoteService,
  type IPromptAttachmentTransferService,
  type IServiceAccessor,
  type RemoteAttachmentDescriptor,
  type RemoteBindContextParams,
  type RemoteConnectParams,
  type RemoteConnectResult,
  type RemoteDetachParams,
  type RemoteSessionParams,
  type RemoteWorkspaceConnectionEvent,
  type RemoteWorkspaceSessionSnapshot,
  type ServiceCollection,
} from "@mode/services";
import { disposeServiceResourcesAndWait } from "@mode/services/node";

import {
  createWindowRemoteConnectionRegistry,
  type WindowRemoteConnectionCloseEvent,
  type WindowRemoteConnectionHandle,
} from "../host/windowRemoteConnectionRegistry.js";
import {
  attachLanRemoteConnection,
  loadLanPeerToken,
  readLanPeerEndpoints,
} from "../host/lanRemoteAttach.js";
import { endpointsForLanTarget, selectReachableLanPeerEndpoint } from "../host/lanPeerEndpoints.js";
import {
  createLanPromptAttachmentTransferService,
  materializeLanPromptAttachments,
} from "../host/lanPromptAttachments.js";
import {
  createRemotePromptAttachmentSessionService,
  createRemotePromptAttachmentTaskService,
  materializeRemotePromptAttachments,
} from "../host/remotePromptAttachments.js";
import { createRemoteBackendPromptAttachmentTransferService } from "../host/remoteBackendPromptAttachments.js";
import { createRemoteWorkspaceServiceCollection } from "../host/remoteWorkspaceServiceCollection.js";
import { hideInternalServiceMembers } from "./internalServiceMembers.js";
import { resolveRemoteAssetDirs, type RemoteAssetDirs } from "./remoteAssets.js";
import type { ScopedAttachmentRegistry } from "./scopedAttachments.js";

/**
 * `remote` 频道的 Host 实现（LR-4，无 Electron）。
 *
 * 状态所有者是 Host 内的 `windowRemoteConnectionRegistry`（每窗口一个）：
 * logical session、连接复用、workspace bind 与 attachment 生命周期都在这里。renderer 只
 * 发起/取消连接、绑定上下文、申请 scoped attachment；services 走单独的 scoped MessagePort。
 *
 * LAN 走 HTTP/WS 挂接；SSH/WSL/Docker 复用 `@mode/server/remote` 的 backend +
 * `connectRemote`，连接成功后使用同一 registry 与 scoped attachment 生命周期。
 */

interface RemoteCapabilities {
  computerUse?: boolean;
}

interface AttachmentRecord {
  attachmentId: string;
  sessionId: string;
  scopeId: number;
  dispose: () => void;
}

/** Bot 侧的远端 workspace 目标（与 `@mode/services` 的直连传输同形）。 */
export interface BotRemoteWorkspaceHostTarget {
  workspacePath: string;
  workspaceIdentity: string;
  target: RemoteTarget;
}

export interface RemoteWorkspaceHostService extends IRemoteService {
  /** Bot 查询该 workspace 是否已有 online logical session（按 target + identity 判等）。 */
  queryBotConnectionStatus(target: BotRemoteWorkspaceHostTarget): Promise<boolean>;
  /** Bot 重连：复用已有 online session，否则新建连接；成功后向 renderer 发事件。 */
  reconnectBotWorkspace(target: BotRemoteWorkspaceHostTarget): Promise<
    | { ok: true; sessionId: string }
    | {
        ok: false;
        error: string;
      }
  >;
  /**
   * 给 Bot 建一条**进程内** scoped attachment：远端服务集合经 MessageChannel 暴露成
   * RPC 端点，端口直接交给 Bot 桥（不经页面桥/帧解复用）。调用方负责调用 `dispose`。
   */
  createBotRuntimePort(target: BotRemoteWorkspaceHostTarget): Promise<
    | { ok: true; port: unknown; dispose: () => void }
    | {
        ok: false;
        error: string;
      }
  >;
  /** Host 退出：释放 registry、所有 attachment 与底层连接。 */
  disposeHost(): Promise<void>;
}

export interface CreateRemoteWorkspaceHostServiceOptions {
  services: ServiceCollection;
  log: (line: string) => void;
  scopedAttachments: ScopedAttachmentRegistry;
  /** scoped 频道同一份 listen 守卫。 */
  guardChannel: (channelName: string, channel: IServerChannel<string>) => IServerChannel<string>;
  /** 测试注入点：替换 LAN transport。 */
  connectLan?: (
    target: Extract<RemoteTarget, { kind: "lan" }>,
    signal: AbortSignal,
  ) => Promise<WindowRemoteConnectionHandle<ServiceCollection, RemoteCapabilities>>;
  /** 测试注入点：替换 SSH/WSL/Docker transport。 */
  connectStdio?: (
    target: Exclude<RemoteTarget, { kind: "lan" }>,
    remoteAssets: RemoteAssetDirs,
    signal: AbortSignal,
  ) => Promise<WindowRemoteConnectionHandle<ServiceCollection, RemoteCapabilities>>;
  /** SSH/WSL/Docker 部署资源目录；缺省按 env/资源目录/mock-cdn 解析。 */
  remoteAssets?: RemoteAssetDirs;
  /** 测试注入点：替换真实 WS 挂接（用于覆盖关闭回调接线，不发起网络）。 */
  attachLan?: typeof attachLanRemoteConnection;
  /** 测试注入点：替换远端服务集合构造（避免单测创建本机 Node 服务）。 */
  createRemoteWorkspaceServices?: typeof createRemoteWorkspaceServiceCollection;
  createId?: () => string;
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

interface RemoteCloseBridge {
  notify(event: WindowRemoteConnectionCloseEvent): void;
  subscribe(listener: (event: WindowRemoteConnectionCloseEvent) => void): { dispose(): void };
}

function createRemoteCloseBridge(): RemoteCloseBridge {
  const listeners = new Set<(event: WindowRemoteConnectionCloseEvent) => void>();
  let closed = false;
  return {
    notify(event) {
      if (closed) {
        return;
      }
      closed = true;
      for (const listener of Array.from(listeners)) {
        listener(event);
      }
      listeners.clear();
    },
    subscribe(listener) {
      if (closed) {
        queueMicrotask(() => listener({ exitCode: null, signal: null }));
        return { dispose: () => undefined };
      }
      listeners.add(listener);
      return { dispose: () => listeners.delete(listener) };
    },
  };
}

type MaterializePromptAttachments = (request: {
  taskId: string;
  traceId: TraceId;
  content: string;
  attachments?: ModePromptAttachment[];
}) => Promise<{ content: string; attachments?: ModePromptAttachment[] }>;

function createRemoteWorkspaceHandle(params: {
  connectionServices: IServiceAccessor;
  sourceServices: ServiceCollection;
  clientConfigService: IClientConfigService;
  createRemoteWorkspaceServices: typeof createRemoteWorkspaceServiceCollection;
  promptAttachmentTransferService: IPromptAttachmentTransferService;
  materializePromptAttachments: MaterializePromptAttachments;
  capabilities?: RemoteCapabilities;
  closeBridge: RemoteCloseBridge;
  disposeConnection: () => Promise<void> | void;
  log: (line: string) => void;
}): WindowRemoteConnectionHandle<ServiceCollection, RemoteCapabilities> {
  const promptAttachmentTransferService = params.promptAttachmentTransferService;
  const remoteServices = params.createRemoteWorkspaceServices({
    clientConfigService: params.clientConfigService,
    connectionServices: params.connectionServices,
    sourceServices: params.sourceServices,
    parentPort: null,
    // Tauri Host 没有跨窗口 main 通道：任务路由通告与实时镜像暂不接（登记在 LR-4 剩余）。
    createReportingRemoteModeTaskService: (service) => service,
    createRemotePromptAttachmentTaskService: (service) =>
      createRemotePromptAttachmentTaskService(service, {
        materializePromptAttachments: params.materializePromptAttachments,
      }),
    createRemotePromptAttachmentSessionService: (service) =>
      createRemotePromptAttachmentSessionService(service, {
        materializePromptAttachments: params.materializePromptAttachments,
      }),
    promptAttachmentTransferService,
    runtimePreferencesBridge: {
      onError: (error: unknown) =>
        params.log(`remote_runtime_preferences_failed ${describeError(error)}`),
    },
  });
  let disposed = false;
  return {
    services: remoteServices,
    ...(params.capabilities ? { capabilities: params.capabilities } : {}),
    onDidClose: (listener) => params.closeBridge.subscribe(listener),
    async dispose() {
      if (disposed) {
        return;
      }
      disposed = true;
      await disposeServiceResourcesAndWait(remoteServices);
      await params.disposeConnection();
    },
  };
}

export async function createRemoteWorkspaceHostService(
  options: CreateRemoteWorkspaceHostServiceOptions,
): Promise<RemoteWorkspaceHostService> {
  const { services, log } = options;
  const createId = options.createId ?? randomUUID;
  const events = new Emitter<RemoteWorkspaceConnectionEvent>();
  const attachments = new Map<string, AttachmentRecord>();
  const attachmentsByScope = new Map<number, AttachmentRecord>();
  const scopeSubscription = options.scopedAttachments.onDidDetach((scopeId) => {
    const record = attachmentsByScope.get(scopeId);
    if (!record) {
      return;
    }
    attachmentsByScope.delete(scopeId);
    attachments.delete(record.attachmentId);
  });

  const emitLog = (
    level: RemoteConnectionRuntimeLog["level"],
    message: string,
    meta: { requestId?: string; sessionId?: string; source?: string } = {},
  ): void => {
    const entry: RemoteConnectionRuntimeLog = {
      label: "remote",
      level,
      source: meta.source ?? "host/remote",
      message,
      timestamp: new Date().toISOString(),
      ...(meta.requestId ? { requestId: meta.requestId } : {}),
      ...(meta.sessionId ? { sessionId: meta.sessionId } : {}),
    };
    log(`remote_${level} ${message}`);
    events.fire({ kind: "log", entry });
  };

  const connectLanTarget = async (
    target: Extract<RemoteTarget, { kind: "lan" }>,
    signal: AbortSignal,
  ): Promise<WindowRemoteConnectionHandle<ServiceCollection, RemoteCapabilities>> => {
    const connect = options.connectLan ?? defaultConnectLan;
    return await connect(target, signal);
  };

  const defaultConnectLan = async (
    target: Extract<RemoteTarget, { kind: "lan" }>,
    signal: AbortSignal,
  ): Promise<WindowRemoteConnectionHandle<ServiceCollection, RemoteCapabilities>> => {
    const credentials = services.getOptional(ICredentialService);
    if (!credentials) {
      throw new Error("凭据服务不可用，无法读取局域网配对令牌");
    }
    const stored = target.serverId
      ? await readLanPeerEndpoints(credentials, target.serverId)
      : null;
    const token = target.token?.trim() || (await loadLanPeerToken(credentials, target.serverId));
    if (!token) {
      throw new Error("这台设备还没有配对：请先完成局域网配对");
    }
    // 端点事实源：显式 target.endpoints > 凭据 meta > target 的主端点。
    const endpoints = target.endpoints?.length
      ? endpointsForLanTarget(target)
      : stored?.endpoints?.length
        ? stored.endpoints
        : endpointsForLanTarget(target);
    // 测试注入 attachLan 时没有真实 HTTP 端点可探测；直接使用主端点，
    // 与生产路径共用同一套端点构造与关闭语义。
    const injectedAttach = options.attachLan !== undefined;
    const selection = injectedAttach
      ? { ok: true as const, endpoint: endpoints[0]!, info: undefined }
      : await selectReachableLanPeerEndpoint({ endpoints, token });
    if (!selection.ok) {
      throw selection.error instanceof Error ? selection.error : new Error("对端不可达");
    }
    const { endpoint } = selection;
    const info = selection.info;
    // LAN 断链是 logical session 的唯一降级信号：必须把 WS close 接到 handle 的
    // onDidClose；否则 registry 永远认为连接在线，scoped RPC 会永久 pending。
    const closeBridge = createRemoteCloseBridge();
    const attachLan = options.attachLan ?? attachLanRemoteConnection;
    const connection = await attachLan({
      scheme: endpoint.scheme,
      host: endpoint.host,
      port: endpoint.port,
      ...(endpoint.path ? { path: endpoint.path } : {}),
      token,
      ...(info ? { info } : {}),
      signal,
      onClose: () => closeBridge.notify({ exitCode: null, signal: null }),
    });
    if (signal.aborted) {
      await connection.dispose();
      throw new Error("远程连接已取消");
    }
    const promptAttachmentTransferService = createLanPromptAttachmentTransferService({
      channel: connection.services.modeAgentService,
    });
    try {
      return createRemoteWorkspaceHandle({
        connectionServices: connection.services,
        sourceServices: services,
        clientConfigService: services.get(IClientConfigService),
        createRemoteWorkspaceServices:
          options.createRemoteWorkspaceServices ?? createRemoteWorkspaceServiceCollection,
        promptAttachmentTransferService,
        materializePromptAttachments: async (request) => {
          const result = await materializeLanPromptAttachments(request, {
            fileService: connection.services.fileService,
          });
          return { content: result.content, attachments: result.attachments };
        },
        capabilities:
          connection.info.capabilities.computerUse === true ? { computerUse: true } : undefined,
        closeBridge,
        disposeConnection: () => connection.dispose(),
        log,
      });
    } catch (error) {
      await connection.dispose();
      throw error;
    }
  };

  const defaultConnectStdio = async (
    target: Exclude<RemoteTarget, { kind: "lan" }>,
    remoteAssets: RemoteAssetDirs,
    signal: AbortSignal,
  ): Promise<WindowRemoteConnectionHandle<ServiceCollection, RemoteCapabilities>> => {
    if (
      !remoteAssets.mockCdnDir &&
      !remoteAssets.remoteCdnBaseUrl &&
      !remoteAssets.remoteCdnBaseUrls?.length
    ) {
      throw new Error(
        "SSH/WSL/Docker 远程工作区缺少 remote assets 源：请设置 MODE_REMOTE_ASSET_CDN_BASE_URL，或提供本地 mock-cdn。",
      );
    }
    const { createRemoteBackend, connectRemote, pickRemoteRuntimeEnv } =
      await import("@mode/server/remote");
    const closeBridge = createRemoteCloseBridge();
    const backend = await createRemoteBackend(target);
    const connection = await connectRemote(backend, {
      signal,
      appVersion: MODE_VERSION,
      remoteRuntimeEnv: pickRemoteRuntimeEnv(process.env),
      ...(remoteAssets.mockCdnDir ? { mockCdnDir: remoteAssets.mockCdnDir } : {}),
      ...(remoteAssets.remoteCdnBaseUrl ? { remoteCdnBaseUrl: remoteAssets.remoteCdnBaseUrl } : {}),
      ...(remoteAssets.remoteCdnBaseUrls?.length
        ? { remoteCdnBaseUrls: remoteAssets.remoteCdnBaseUrls }
        : {}),
      ...(remoteAssets.remoteCacheDir ? { remoteCacheDir: remoteAssets.remoteCacheDir } : {}),
      ...(target.kind === "ssh" && target.assetInstallMode
        ? { assetInstallMode: target.assetInstallMode }
        : {}),
      ...(target.kind === "ssh" && target.resourcePackages
        ? { resourcePackages: target.resourcePackages }
        : {}),
      onDidRemoteClose: ({ code }) => closeBridge.notify({ exitCode: code, signal: null }),
    });
    if (signal.aborted) {
      connection.dispose();
      throw new Error("远程连接已取消");
    }
    try {
      return createRemoteWorkspaceHandle({
        connectionServices: connection.services,
        sourceServices: services,
        clientConfigService: services.get(IClientConfigService),
        createRemoteWorkspaceServices:
          options.createRemoteWorkspaceServices ?? createRemoteWorkspaceServiceCollection,
        promptAttachmentTransferService: createRemoteBackendPromptAttachmentTransferService({
          backend,
        }),
        materializePromptAttachments: async (request) => {
          const result = await materializeRemotePromptAttachments(request, { backend });
          return { content: result.content, attachments: result.attachments };
        },
        closeBridge,
        disposeConnection: async () => {
          await connection.disposeAndWait({ timeoutMs: 5_000 });
        },
        log,
      });
    } catch (error) {
      await connection.disposeAndWait({ timeoutMs: 5_000 }).catch(() => undefined);
      throw error;
    }
  };

  const connectStdioTarget = async (
    target: Exclude<RemoteTarget, { kind: "lan" }>,
    remoteAssets: RemoteAssetDirs,
    signal: AbortSignal,
  ): Promise<WindowRemoteConnectionHandle<ServiceCollection, RemoteCapabilities>> => {
    const connect = options.connectStdio ?? defaultConnectStdio;
    return await connect(target, remoteAssets, signal);
  };

  const registry = createWindowRemoteConnectionRegistry<ServiceCollection, RemoteCapabilities>({
    connect: async (request) => {
      if (request.target.kind === "lan") {
        return await connectLanTarget(request.target, request.signal);
      }
      return await connectStdioTarget(
        request.target,
        request.remoteAssets ?? options.remoteAssets ?? resolveRemoteAssetDirs(),
        request.signal,
      );
    },
    createId,
    onSessionClosed: (event) => {
      detachSessionAttachments(event.remoteSessionId);
      events.fire({
        kind: "closed",
        sessionId: event.remoteSessionId,
        exitCode: event.exitCode,
        signal: event.signal,
        ...(event.error ? { error: event.error } : {}),
      });
      emitLog("warn", `远程连接已关闭：${event.error ?? "连接断开"}`, {
        sessionId: event.remoteSessionId,
      });
    },
  });

  /**
   * Bot 视角的 session 查找：target 按 `isSameRemoteTarget` 判等（四种 kind 全覆盖），
   * workspace 按 identity 判等（缺省回退 path），只认 online。
   *
   * 为什么不能只比 remoteSessionId：Bot 侧只有 settings 里的 target 与 identity，
   * 没有 UI 的连接句柄；重连与 runtime attachment 都必须能从这两项找回同一个 session。
   */
  const findBotSession = (target: BotRemoteWorkspaceHostTarget) => {
    const identity = target.workspaceIdentity.trim();
    return (
      registry
        .listSessions()
        .find(
          (session) =>
            session.state === "online" &&
            isSameRemoteTarget(session.target, target.target) &&
            (identity
              ? (session.workspaceIdentity ?? "").trim() === identity
              : (session.workspacePath ?? "") === target.workspacePath),
        ) ?? null
    );
  };

  /** 进程内 bot runtime 端口的回收函数；Host dispose 时统一关闭。 */
  const botRuntimePortDisposers = new Set<() => void>();

  function detachSessionAttachments(sessionId: string): void {
    for (const record of Array.from(attachments.values())) {
      if (record.sessionId === sessionId) {
        record.dispose();
        attachments.delete(record.attachmentId);
        attachmentsByScope.delete(record.scopeId);
      }
    }
  }

  const requireSessionWorkspace = (sessionId: string) => {
    const session = registry.getSession(sessionId);
    if (!session) {
      throw new Error(`未找到远程 logical session，sessionId=${sessionId}`);
    }
    if (!session.workspacePath || !session.workspaceIdentity) {
      throw new Error("远程 workspace 尚未绑定上下文，先 bindContext 再 attach");
    }
    return session as typeof session & {
      workspacePath: string;
      workspaceIdentity: string;
    };
  };

  const service: RemoteWorkspaceHostService = {
    async connect(params: RemoteConnectParams): Promise<RemoteConnectResult> {
      const requestId = params.requestId?.trim() || createId();
      const workspaceIdentity =
        params.workspaceIdentity?.trim() ||
        (params.workspacePath
          ? buildRemoteWorkspaceIdentity(params.workspacePath, params.target)
          : undefined);
      emitLog("info", `开始连接 ${params.target.kind} 远程工作区`, { requestId });
      try {
        const descriptor = await registry.connect({
          requestId,
          target: params.target,
          remoteAssets: options.remoteAssets ?? resolveRemoteAssetDirs(),
          ...(params.workspacePath ? { workspacePath: params.workspacePath } : {}),
          ...(workspaceIdentity ? { workspaceIdentity } : {}),
        });
        events.fire({ kind: "connected", descriptor });
        emitLog("info", "远程连接已建立", { requestId });
        return { ok: true, descriptor };
      } catch (error) {
        const message = describeError(error);
        events.fire({ kind: "connectFailed", requestId, error: message });
        emitLog("error", `远程连接失败：${message}`, { requestId });
        return { ok: false, error: message };
      }
    },

    async cancelConnect(params: { requestId: string }): Promise<void> {
      registry.cancelConnect(params.requestId);
      emitLog("info", "已取消远程连接", { requestId: params.requestId });
    },

    async bindContext(params: RemoteBindContextParams): Promise<void> {
      const session = registry.getSession(params.sessionId);
      if (!session) {
        throw new Error(`未找到远程 logical session，sessionId=${params.sessionId}`);
      }
      const workspacePath = params.workspacePath?.trim();
      if (!workspacePath) {
        throw new Error("workspacePath 不能为空");
      }
      const workspaceIdentity =
        params.workspaceIdentity?.trim() ||
        buildRemoteWorkspaceIdentity(workspacePath, session.target);
      await registry.bindWorkspaceContext({
        remoteSessionId: params.sessionId,
        workspacePath,
        workspaceIdentity,
      });
      emitLog("info", "远程 workspace 上下文已绑定", { sessionId: params.sessionId });
    },

    async dispose(params: RemoteSessionParams): Promise<void> {
      detachSessionAttachments(params.sessionId);
      await registry.disposeSession(params.sessionId);
      emitLog("info", "远程 workspace session 已释放", { sessionId: params.sessionId });
    },

    async attach(params: RemoteSessionParams): Promise<RemoteAttachmentDescriptor> {
      const session = requireSessionWorkspace(params.sessionId);
      const scope = {
        kind: "remote" as const,
        remoteSessionId: params.sessionId,
        workspacePath: session.workspacePath,
        workspaceIdentity: session.workspaceIdentity,
      };
      const remoteServices = registry.resolveScopedServices(scope);
      const attachmentId = createId();
      const attachment = options.scopedAttachments.attach((server) => {
        const guardedServer = {
          registerChannel(channelName: string, channel: IServerChannel<string>) {
            server.registerChannel(channelName, options.guardChannel(channelName, channel));
          },
        };
        remoteServices.exposeOnChannelServer(guardedServer);
      });
      const record: AttachmentRecord = {
        attachmentId,
        sessionId: params.sessionId,
        scopeId: attachment.scopeId,
        dispose: () => attachment.dispose(),
      };
      attachments.set(attachmentId, record);
      attachmentsByScope.set(attachment.scopeId, record);
      emitLog("info", `scoped attachment 已创建：scope=${attachment.scopeId}`, {
        sessionId: params.sessionId,
      });
      const capabilities = registry.resolveScopedCapabilities(scope);
      return {
        scopeId: attachment.scopeId,
        attachmentId,
        sessionId: params.sessionId,
        ...(capabilities?.computerUse === true ? { capabilities: { computerUse: true } } : {}),
      };
    },

    async detach(params: RemoteDetachParams): Promise<void> {
      const record = attachments.get(params.attachmentId);
      if (!record) {
        return;
      }
      attachments.delete(record.attachmentId);
      attachmentsByScope.delete(record.scopeId);
      record.dispose();
      emitLog("info", `scoped attachment 已释放：scope=${record.scopeId}`, {
        sessionId: record.sessionId,
      });
    },

    async getSession(params: RemoteSessionParams): Promise<RemoteWorkspaceSessionSnapshot | null> {
      return registry.getSession(params.sessionId);
    },

    async listSessions(): Promise<RemoteWorkspaceSessionSnapshot[]> {
      return registry.listSessions();
    },

    onDynamicConnectionEvent: (_sessionId?: string) => events.event,

    // ---- Bot 远端桥（进程内直连；不暴露成 renderer RPC command） ---------------------
    async queryBotConnectionStatus(target: BotRemoteWorkspaceHostTarget): Promise<boolean> {
      return findBotSession(target) !== null;
    },

    async reconnectBotWorkspace(target: BotRemoteWorkspaceHostTarget) {
      const existing = findBotSession(target);
      if (existing) {
        // 已有同 target + identity 的 online session：复用，不重复建 logical session。
        return { ok: true as const, sessionId: existing.remoteSessionId };
      }
      const result = await service.connect({
        target: target.target,
        workspacePath: target.workspacePath,
        ...(target.workspaceIdentity.trim()
          ? { workspaceIdentity: target.workspaceIdentity.trim() }
          : {}),
      });
      if (!result.ok) {
        return { ok: false as const, error: result.error };
      }
      const sessionId = result.descriptor.remoteSessionId;
      const event: BotRemoteWorkspaceReconnectedEvent = {
        sessionId,
        workspacePath: target.workspacePath,
        workspaceIdentity: target.workspaceIdentity,
        // 令牌只在连接流程内使用；事件跨到 renderer，一律脱敏。
        target: stripRemoteTargetSecrets(target.target),
      };
      events.fire({ kind: "botReconnected", event });
      emitLog("info", "Bot 已重连远端工作区", { sessionId, source: "host/bot" });
      return { ok: true as const, sessionId };
    },

    async createBotRuntimePort(target: BotRemoteWorkspaceHostTarget) {
      const session = findBotSession(target);
      if (!session?.workspacePath || !session.workspaceIdentity) {
        return { ok: false as const, error: "远端 workspace 尚未连接或未绑定上下文。" };
      }
      let remoteServices: ServiceCollection;
      try {
        remoteServices = registry.resolveScopedServices({
          kind: "remote",
          remoteSessionId: session.remoteSessionId,
          workspacePath: session.workspacePath,
          workspaceIdentity: session.workspaceIdentity,
        });
      } catch (error) {
        return { ok: false as const, error: describeError(error) };
      }
      // 进程内 MessageChannel：一端挂 ChannelServer 暴露远端服务集合，另一端直接交给 Bot 桥。
      // 这条路径不经过页面桥与帧解复用（没有 renderer 参与），所以也无需 `scope-ready`。
      const channel = new MessageChannel();
      const server = new ChannelServer(
        new MessagePortProtocol(channel.port1 as unknown as MessagePortLike),
        `mode-host/bot-runtime-${session.remoteSessionId}`,
        1000,
        true,
      );
      remoteServices.exposeOnChannelServer({
        registerChannel(channelName: string, channelServer: IServerChannel<string>) {
          server.registerChannel(channelName, options.guardChannel(channelName, channelServer));
        },
      });
      server.ready();
      let closed = false;
      const dispose = (): void => {
        if (closed) {
          return;
        }
        closed = true;
        botRuntimePortDisposers.delete(dispose);
        server.dispose();
        channel.port1.close();
        channel.port2.close();
        emitLog("info", "Bot runtime attachment 已释放", {
          sessionId: session.remoteSessionId,
          source: "host/bot",
        });
      };
      botRuntimePortDisposers.add(dispose);
      emitLog("info", "Bot runtime attachment 已创建", {
        sessionId: session.remoteSessionId,
        source: "host/bot",
      });
      return { ok: true as const, port: channel.port2, dispose };
    },

    async disposeHost(): Promise<void> {
      scopeSubscription.dispose();
      for (const record of Array.from(attachments.values())) {
        record.dispose();
      }
      for (const disposePort of Array.from(botRuntimePortDisposers)) {
        disposePort();
      }
      attachments.clear();
      attachmentsByScope.clear();
      events.dispose();
      await registry.dispose();
    },
  };
  // Host 内部生命周期方法不能被 ProxyChannel 暴露成 RPC command。
  // Bot 三个方法同理：`createBotRuntimePort` 甚至返回 MessagePort（不可 JSON 序列化）。
  return hideInternalServiceMembers(service, [
    "disposeHost",
    "queryBotConnectionStatus",
    "reconnectBotWorkspace",
    "createBotRuntimePort",
  ]);
}
