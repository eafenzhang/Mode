import { randomUUID } from "node:crypto";
import {
  HostResponseTypes,
  hostBotRemoteWorkspaceConnectionStatusResultMessageSchema,
  hostBotRemoteWorkspaceRuntimePortMessageSchema,
  hostBotRemoteWorkspaceReconnectResultMessageSchema,
  type RemoteTarget,
} from "@mode/shared";
import { type IModeTaskService as IModeTaskServiceShape } from "../session/modeTaskService.js";
import type { ICredentialService } from "../credential/credential.js";
import type { ISettingService } from "../setting/setting.js";
import { type ModeAgentAppRuntimePreferences } from "../mode-agent/modeAgent.js";
import {
  createRemoteRuntimeServicesFromPort,
  type RemoteBotWorkspaceRuntimeServices,
} from "#src/bots/botRemoteRuntimeServices.js";

interface ParentPortLike {
  postMessage(message: unknown, transfer?: unknown[]): void;
  on(event: "message", listener: (event: ParentPortMessageEvent) => void): void;
  off?(event: "message", listener: (event: ParentPortMessageEvent) => void): void;
}

interface ParentPortMessageEvent {
  data: unknown;
  ports?: unknown[];
}

/** Bot 桥查询/重连/建 runtime port 时携带的远端目标（target 已按 settings 快照解析出 secret）。 */
export interface BotRemoteWorkspaceTransportTarget {
  workspacePath: string;
  workspaceIdentity: string;
  target: RemoteTarget;
}

/**
 * 进程内直连传输（Tauri Host sidecar）。
 *
 * 旧 Electron 线靠 `parentPort` 把请求转发给 main，由 main 的 registry 处理；迁移后
 * registry 就在同一个 Host 进程里（`docs/plans/2026-10-19-lan-remote-sidecar-migration.md` §5.4），
 * 因此不再需要一个跨进程路由器。传输由 Host 装配层注入，三个方法分别对应
 * 「查连接状态 / 重连 / 建 runtime attachment」。
 */
export interface BotRemoteWorkspaceTransport {
  /** `null` 表示查询失败（调用方回退到本地 Set），不是「未连接」。 */
  queryConnectionStatus(target: BotRemoteWorkspaceTransportTarget): Promise<boolean | null>;
  reconnect(target: BotRemoteWorkspaceTransportTarget): Promise<{
    ok: boolean;
    sessionId?: string;
    error?: string;
  }>;
  createRuntimePort(target: BotRemoteWorkspaceTransportTarget): Promise<{
    ok: boolean;
    port?: unknown;
    error?: string;
    /** Host 侧 RPC 端点与端口的释放函数（直连传输才需要）。 */
    dispose?: () => void;
  }>;
}

export function createBotRemoteWorkspaceService(params: {
  parentPort?: ParentPortLike | null;
  /** 进程内直连传输；与 `parentPort` 二选一，同时存在时优先直连。 */
  transport?: BotRemoteWorkspaceTransport;
  settingService: ISettingService;
  credentialService: ICredentialService;
  createRuntimeServicesFromPort?: (port: unknown) => RemoteBotWorkspaceRuntimeServices;
}) {
  const parentPort = params.parentPort;
  const transport = params.transport;
  if (!parentPort && !transport) {
    return undefined;
  }
  const activeParentPort = parentPort;
  const connectedWorkspaceKeys = new Set<string>();
  const pending = new Map<
    string,
    (result: { ok: boolean; sessionId?: string; error?: string }) => void
  >();
  const pendingRuntimePorts = new Map<
    string,
    (result: { ok: boolean; port?: unknown; error?: string }) => void
  >();
  const pendingConnectionStatus = new Map<
    string,
    (result: { ok: boolean; connected?: boolean; error?: string }) => void
  >();
  const runtimeServicesByWorkspaceKey = new Map<string, RemoteBotWorkspaceRuntimeServices>();
  // 直连传输建的 runtime port 由 Host 持有 RPC 端点与 MessageChannel；dispose 必须一起关闭，
  // 否则 Host 退出后端口/ChannelServer 仍挂在内存里。
  const runtimePortDisposersByWorkspaceKey = new Map<string, () => void>();
  let latestAppRuntimePreferences: ModeAgentAppRuntimePreferences | undefined;
  let appRuntimePreferencesRevision = 0;
  const onMessage = (event: ParentPortMessageEvent) => {
    const result = hostBotRemoteWorkspaceReconnectResultMessageSchema.safeParse(event.data);
    if (result.success) {
      const { requestId } = result.data;
      const resolve = pending.get(requestId);
      if (!resolve) {
        return;
      }
      pending.delete(requestId);
      resolve({
        ok: result.data.ok,
        sessionId: result.data.sessionId,
        error: result.data.error,
      });
      return;
    }

    const connectionStatusResult =
      hostBotRemoteWorkspaceConnectionStatusResultMessageSchema.safeParse(event.data);
    if (connectionStatusResult.success) {
      const { requestId } = connectionStatusResult.data;
      const resolve = pendingConnectionStatus.get(requestId);
      if (!resolve) {
        return;
      }
      pendingConnectionStatus.delete(requestId);
      resolve({
        ok: connectionStatusResult.data.ok,
        connected: connectionStatusResult.data.connected,
        error: connectionStatusResult.data.error,
      });
      return;
    }

    const runtimePortResult = hostBotRemoteWorkspaceRuntimePortMessageSchema.safeParse(event.data);
    if (!runtimePortResult.success) {
      return;
    }
    const { requestId } = runtimePortResult.data;
    const resolve = pendingRuntimePorts.get(requestId);
    if (!resolve) {
      return;
    }
    pendingRuntimePorts.delete(requestId);
    resolve({
      ok: runtimePortResult.data.ok,
      port: event.ports?.[0],
      error: runtimePortResult.data.error,
    });
  };
  activeParentPort?.on("message", onMessage);

  async function buildRemoteTargetForWorkspace(target: {
    workspacePath: string;
    workspaceIdentity: string;
  }): Promise<RemoteTarget | null> {
    const settings = await params.settingService.get();
    const workspaceIdentity = target.workspaceIdentity.trim();
    const remoteSessions = (settings.lastWorkspaceSession ?? []).filter(
      (item) => item.kind === "remote",
    );
    const entry =
      // Bugfix: UI 建连后可能把 workspacePath 规范化为 realpath，但 bot context 仍保留旧路径。
      // 远端身份隔离语义以 workspaceIdentity 为准，查连接信息时必须先按 identity 命中。
      remoteSessions.find((item) => item.workspaceIdentity === workspaceIdentity) ??
      remoteSessions.find(
        (item) =>
          item.workspacePath === target.workspacePath &&
          item.workspaceIdentity === workspaceIdentity,
      );
    if (!entry || entry.kind !== "remote") {
      return null;
    }
    if (entry.target.kind !== "ssh") {
      return entry.target;
    }
    return {
      kind: "ssh",
      host: entry.target.host,
      port: entry.target.port,
      username: entry.target.username,
      privateKeyPath: entry.target.privateKeyPath,
      password: entry.target.passwordCredentialKey
        ? ((await params.credentialService.load(entry.target.passwordCredentialKey)) ?? undefined)
        : undefined,
      privateKeyPassphrase: entry.target.privateKeyPassphraseCredentialKey
        ? ((await params.credentialService.load(entry.target.privateKeyPassphraseCredentialKey)) ??
          undefined)
        : undefined,
    };
  }

  async function queryMainConnectionStatus(target: {
    workspacePath: string;
    workspaceIdentity: string;
    remoteTarget: RemoteTarget;
  }): Promise<boolean | null> {
    if (transport) {
      try {
        return await transport.queryConnectionStatus({
          workspacePath: target.workspacePath,
          workspaceIdentity: target.workspaceIdentity,
          target: target.remoteTarget,
        });
      } catch {
        // 查询失败与「未连接」是两件事：返回 null 让调用方回退到本地 Set。
        return null;
      }
    }
    if (!activeParentPort) {
      return null;
    }
    const requestId = `bot-status-${randomUUID()}`;
    const result = await new Promise<{
      ok: boolean;
      connected?: boolean;
      error?: string;
    }>((resolve) => {
      pendingConnectionStatus.set(requestId, resolve);
      activeParentPort.postMessage({
        type: HostResponseTypes.BotRemoteWorkspaceConnectionStatusRequest,
        requestId,
        workspacePath: target.workspacePath,
        workspaceIdentity: target.workspaceIdentity,
        target: target.remoteTarget,
      });
      setTimeout(() => {
        if (pendingConnectionStatus.delete(requestId)) {
          resolve({ ok: false, error: "远端 workspace 连接状态查询超时。" });
        }
      }, 5_000);
    });
    return result.ok ? result.connected === true : null;
  }

  return {
    async isConnected(target: {
      workspacePath: string;
      workspaceIdentity: string;
    }): Promise<boolean> {
      const workspaceKey = target.workspaceIdentity.trim() || target.workspacePath;
      const remoteTarget = await buildRemoteTargetForWorkspace(target);
      if (!remoteTarget) {
        connectedWorkspaceKeys.delete(workspaceKey);
        return false;
      }

      // Bugfix: UI 手动重连不会经过 bot 的 /reconnect，单靠本地 Set 会误判为未连接。
      // 每次询问 main 的 live session 表，顺手清理远端断开后的陈旧 bot 标记。
      const connected = await queryMainConnectionStatus({
        ...target,
        remoteTarget,
      });
      if (connected !== null) {
        if (connected) {
          connectedWorkspaceKeys.add(workspaceKey);
        } else {
          connectedWorkspaceKeys.delete(workspaceKey);
        }
        return connected;
      }

      return connectedWorkspaceKeys.has(workspaceKey);
    },
    async ensureConnected(target: {
      workspacePath: string;
      workspaceIdentity: string;
    }): Promise<{ ok: boolean; message?: string }> {
      const remoteTarget = await buildRemoteTargetForWorkspace(target);
      if (!remoteTarget) {
        return {
          ok: false,
          message: "未找到该远端 workspace 的连接信息。",
        };
      }
      if (transport) {
        const direct = await transport.reconnect({
          workspacePath: target.workspacePath,
          workspaceIdentity: target.workspaceIdentity,
          target: remoteTarget,
        });
        if (direct.ok) {
          connectedWorkspaceKeys.add(target.workspaceIdentity.trim() || target.workspacePath);
          return { ok: true };
        }
        return { ok: false, message: direct.error ?? "unknown" };
      }
      if (!activeParentPort) {
        return { ok: false, message: "远端 workspace 重连通道不可用。" };
      }
      const requestId = `bot-reconnect-${randomUUID()}`;
      const result = await new Promise<{
        ok: boolean;
        sessionId?: string;
        error?: string;
      }>((resolve) => {
        pending.set(requestId, resolve);
        activeParentPort.postMessage({
          type: HostResponseTypes.BotRemoteWorkspaceReconnectRequest,
          requestId,
          workspacePath: target.workspacePath,
          workspaceIdentity: target.workspaceIdentity,
          target: remoteTarget,
        });
        setTimeout(() => {
          if (pending.delete(requestId)) {
            resolve({ ok: false, error: "远端 workspace 重连超时。" });
          }
        }, 60_000);
      });
      if (result.ok) {
        connectedWorkspaceKeys.add(target.workspaceIdentity.trim() || target.workspacePath);
        return { ok: true };
      }
      return { ok: false, message: result.error ?? "unknown" };
    },
    async getModeTaskService(target: {
      workspacePath: string;
      workspaceIdentity: string;
    }): Promise<IModeTaskServiceShape | null> {
      return (await getRuntimeServices(target))?.modeTaskService ?? null;
    },
    async getModelSelectionService(target: { workspacePath: string; workspaceIdentity: string }) {
      return (await getRuntimeServices(target))?.modelSelectionService ?? null;
    },
    async syncAppRuntimePreferences(preferences: ModeAgentAppRuntimePreferences): Promise<void> {
      latestAppRuntimePreferences = preferences;
      appRuntimePreferencesRevision += 1;
      // 修复原因：远端 Bot runtime 不属于任何 renderer 窗口，Root 的 Agent 同步无法触达它。
      // 这里只更新已经缓存的 runtime，避免切换设置时为了闲置 Bot 新建远端 Host/Agent。
      await Promise.all(
        Array.from(runtimeServicesByWorkspaceKey.values()).map((services) =>
          services.modeAgentService.syncAppRuntimePreferences(preferences),
        ),
      );
    },
    dispose(): void {
      // Bugfix: host dispose 时移除 parentPort 监听，避免窗口 reload 后旧 bot 重连 promise 继续接收结果。
      activeParentPort?.off?.("message", onMessage);
      for (const disposePort of runtimePortDisposersByWorkspaceKey.values()) {
        try {
          disposePort();
        } catch {
          // 端口已关闭不是错误：dispose 只保证「尽力回收」。
        }
      }
      runtimePortDisposersByWorkspaceKey.clear();
      pending.clear();
      pendingRuntimePorts.clear();
      pendingConnectionStatus.clear();
      runtimeServicesByWorkspaceKey.clear();
      connectedWorkspaceKeys.clear();
    },
  };

  async function getRuntimeServices(target: {
    workspacePath: string;
    workspaceIdentity: string;
  }): Promise<RemoteBotWorkspaceRuntimeServices | null> {
    const workspaceKey = target.workspaceIdentity.trim() || target.workspacePath;
    const cached = runtimeServicesByWorkspaceKey.get(workspaceKey);
    if (cached) {
      return cached;
    }
    const remoteTarget = await buildRemoteTargetForWorkspace(target);
    if (!remoteTarget) {
      return null;
    }
    const result = transport
      ? await transport.createRuntimePort({
          workspacePath: target.workspacePath,
          workspaceIdentity: target.workspaceIdentity,
          target: remoteTarget,
        })
      : await requestRuntimePortFromParent(target, remoteTarget);
    if (!result.ok || !result.port) {
      throw new Error(result.error ?? "远端 workspace runtime 初始化失败。");
    }
    // Bugfix: Bot 任务以前只知道远端 identity，却继续调用本地 task service。
    // 这里把 main 转发来的远端 RPC 端口包装成一组 runtime services；
    // task wrapper 命令走 IModeTaskService，session 主状态走 Mode session facade。
    const services =
      params.createRuntimeServicesFromPort?.(result.port) ??
      createRemoteRuntimeServicesFromPort(result.port);
    // 远端 Bot 与 UI workspace 共用同一个远端 Environment。这里只确认远端
    // Model Selection Facade 已就绪，Desktop 不再向远端注入完整 Provider Registry。
    await services.modelSelectionService.getView();
    while (true) {
      const revision = appRuntimePreferencesRevision;
      const cachedPreferences = latestAppRuntimePreferences;
      const preferences: ModeAgentAppRuntimePreferences = cachedPreferences
        ? cachedPreferences
        : await params.settingService.get().then((settings) => ({
            askUserQuestionAutoResolutionEnabled:
              settings.askUserQuestionAutoResolutionEnabled !== false,
            modelIoFullRetentionEnabled: settings.modelIoFullRetentionEnabled === true,
          }));
      await services.modeAgentService.syncAppRuntimePreferences(preferences);
      if (revision === appRuntimePreferencesRevision) {
        break;
      }
    }
    // 只有远端 Registry 和 App Runtime Preferences 都已就绪后才缓存 runtime services。
    runtimeServicesByWorkspaceKey.set(workspaceKey, services);
    if (result.dispose) {
      runtimePortDisposersByWorkspaceKey.set(workspaceKey, result.dispose);
    }
    return services;
  }

  async function requestRuntimePortFromParent(
    target: { workspacePath: string; workspaceIdentity: string },
    remoteTarget: RemoteTarget,
  ): Promise<{ ok: boolean; port?: unknown; error?: string; dispose?: () => void }> {
    if (!activeParentPort) {
      return { ok: false, error: "远端 workspace runtime 通道不可用。" };
    }
    const requestId = `bot-runtime-${randomUUID()}`;
    return await new Promise((resolve) => {
      pendingRuntimePorts.set(requestId, resolve);
      activeParentPort.postMessage({
        type: HostResponseTypes.BotRemoteWorkspaceRuntimePortRequest,
        requestId,
        workspacePath: target.workspacePath,
        workspaceIdentity: target.workspaceIdentity,
        target: remoteTarget,
      });
      setTimeout(() => {
        if (pendingRuntimePorts.delete(requestId)) {
          resolve({ ok: false, error: "远端 workspace runtime 初始化超时。" });
        }
      }, 60_000);
    });
  }
}
