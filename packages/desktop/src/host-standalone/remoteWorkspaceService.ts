import { randomUUID } from "node:crypto";

import { Emitter, type IServerChannel } from "@mode/rpc";
import {
  buildRemoteWorkspaceIdentity,
  type ModePromptAttachment,
  type RemoteConnectionRuntimeLog,
  type RemoteTarget,
  type TraceId,
} from "@mode/shared";
import {
  IClientConfigService,
  ICredentialService,
  IRemoteService,
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
import { attachLanRemoteConnection, loadLanPeerToken } from "../host/lanRemoteAttach.js";
import {
  createLanPromptAttachmentTransferService,
  materializeLanPromptAttachments,
} from "../host/lanPromptAttachments.js";
import {
  createRemotePromptAttachmentSessionService,
  createRemotePromptAttachmentTaskService,
} from "../host/remotePromptAttachments.js";
import { createRemoteWorkspaceServiceCollection } from "../host/remoteWorkspaceServiceCollection.js";
import { hideInternalServiceMembers } from "./internalServiceMembers.js";
import type { ScopedAttachmentRegistry } from "./scopedAttachments.js";

/**
 * `remote` 频道的 Host 实现（LR-4，无 Electron）。
 *
 * 状态所有者是 Host 内的 `windowRemoteConnectionRegistry`（每窗口一个）：
 * logical session、连接复用、workspace bind 与 attachment 生命周期都在这里。renderer 只
 * 发起/取消连接、绑定上下文、申请 scoped attachment；services 走单独的 scoped MessagePort。
 *
 * 当前只接 LAN。SSH/WSL/Docker 在 Tauri Host 里明确返回不支持（不伪装成功）。
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

export interface RemoteWorkspaceHostService extends IRemoteService {
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
  createId?: () => string;
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
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
    const token = target.token?.trim() || (await loadLanPeerToken(credentials, target.serverId));
    if (!token) {
      throw new Error("这台设备还没有配对：请先完成局域网配对");
    }
    const connection = await attachLanRemoteConnection({
      host: target.host,
      port: target.port,
      token,
      signal,
    });
    const promptAttachmentTransferService = createLanPromptAttachmentTransferService({
      channel: connection.services.modeAgentService,
    });
    const materializePromptAttachments = async (request: {
      taskId: string;
      traceId: TraceId;
      content: string;
      attachments?: ModePromptAttachment[];
    }) => {
      const result = await materializeLanPromptAttachments(request, {
        fileService: connection.services.fileService,
      });
      return { content: result.content, attachments: result.attachments };
    };
    const remoteServices = createRemoteWorkspaceServiceCollection({
      clientConfigService: services.get(IClientConfigService),
      connectionServices: connection.services,
      sourceServices: services,
      parentPort: null,
      // Tauri Host 没有跨窗口 main 通道：任务路由通告与实时镜像暂不接（登记在 LR-4 剩余）。
      createReportingRemoteModeTaskService: (service) => service,
      createRemotePromptAttachmentTaskService: (service) =>
        createRemotePromptAttachmentTaskService(service, {
          materializePromptAttachments,
        }),
      createRemotePromptAttachmentSessionService: (service) =>
        createRemotePromptAttachmentSessionService(service, {
          materializePromptAttachments,
        }),
      promptAttachmentTransferService,
      runtimePreferencesBridge: {
        onError: (error: unknown) =>
          log(`remote_runtime_preferences_failed ${describeError(error)}`),
      },
    });
    const closeListeners = new Set<(event: WindowRemoteConnectionCloseEvent) => void>();
    let disposed = false;
    return {
      services: remoteServices,
      capabilities:
        connection.info.capabilities.computerUse === true ? { computerUse: true } : undefined,
      onDidClose(listener) {
        closeListeners.add(listener);
        return { dispose: () => closeListeners.delete(listener) };
      },
      async dispose() {
        if (disposed) {
          return;
        }
        disposed = true;
        closeListeners.clear();
        await disposeServiceResourcesAndWait(remoteServices);
        await connection.dispose();
      },
    };
  };

  const registry = createWindowRemoteConnectionRegistry<ServiceCollection, RemoteCapabilities>({
    connect: async (request) => {
      if (request.target.kind !== "lan") {
        throw new Error(`Tauri Host 暂不支持 ${request.target.kind} 远程工作区（LR-4 剩余）`);
      }
      return await connectLanTarget(request.target, request.signal);
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
          remoteAssets: {},
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

    async disposeHost(): Promise<void> {
      scopeSubscription.dispose();
      for (const record of Array.from(attachments.values())) {
        record.dispose();
      }
      attachments.clear();
      attachmentsByScope.clear();
      events.dispose();
      await registry.dispose();
    },
  };
  // Host 内部生命周期方法不能被 ProxyChannel 暴露成 RPC command。
  return hideInternalServiceMembers(service, ["disposeHost"]);
}
