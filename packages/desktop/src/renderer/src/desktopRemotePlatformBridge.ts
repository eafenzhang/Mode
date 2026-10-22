import {
  buildRemoteWorkspaceIdentity,
  stripRemoteTargetSecrets,
  type IPlatformService,
  type RemoteTarget,
} from "@mode/shared";
import type { IRemoteService } from "@mode/services";

/**
 * 远程工作区（LR-4）的平台委托：connect/bind/dispose 与事件订阅。
 *
 * Tauri 下 Host 持有 `windowRemoteConnectionRegistry`；renderer 在 connect 成功后向页面桥
 * 申请 scoped port（`mode:scoped-service-port`），bind 换代时先建新 scope 再释放旧 scope。
 * Electron 下保持 `window.mode` 路径。
 */

interface HostBridgeScopedPorts {
  createScopedServicePort(options: {
    scopeId: number;
    attachmentId: string;
    sessionId: string;
    target: RemoteTarget;
    capabilities?: { computerUse?: boolean };
  }): boolean;
  releaseScopedServicePort(scopeId: number): boolean;
}

function hostBridgeScopedPorts(): Partial<HostBridgeScopedPorts> | undefined {
  return (window as Window & { __MODE_HOST_BRIDGE__?: Partial<HostBridgeScopedPorts> })
    .__MODE_HOST_BRIDGE__;
}

type RemotePlatformMethods = Pick<
  IPlatformService,
  | "onRemoteConnectionLog"
  | "onRemoteSessionClosed"
  | "onBotRemoteWorkspaceReconnected"
  | "connectRemote"
  | "cancelPendingRemoteConnection"
  | "bindRemoteWorkspaceSessionContext"
  | "disposeRemoteSession"
>;

export function createDesktopRemotePlatformBridge(options: {
  getRemoteService: () => IRemoteService | undefined;
}): RemotePlatformMethods {
  /** sessionId → 当前 attachment；bind 换代时先建新 scope 再释放旧 scope。 */
  const remoteAttachments = new Map<string, { attachmentId: string; scopeId: number }>();

  const attachRemoteSession = async (sessionId: string, target: RemoteTarget): Promise<boolean> => {
    const remote = options.getRemoteService();
    const bridge = hostBridgeScopedPorts();
    if (!remote || typeof bridge?.createScopedServicePort !== "function") {
      return false;
    }
    const attachment = await remote.attach({ sessionId });
    const created = bridge.createScopedServicePort({
      scopeId: attachment.scopeId,
      attachmentId: attachment.attachmentId,
      sessionId,
      // 令牌只在连接流程内使用；renderer store 只留脱敏 target。
      target: stripRemoteTargetSecrets(target),
      ...(attachment.capabilities ? { capabilities: attachment.capabilities } : {}),
    });
    if (!created) {
      await remote.detach({ attachmentId: attachment.attachmentId }).catch(() => undefined);
      return false;
    }
    const previous = remoteAttachments.get(sessionId);
    remoteAttachments.set(sessionId, {
      attachmentId: attachment.attachmentId,
      scopeId: attachment.scopeId,
    });
    if (previous) {
      // 同一 session 的 attachment 换代：旧端口先由页面桥回收，Host 侧 detach 幂等。
      hostBridgeScopedPorts()?.releaseScopedServicePort?.(previous.scopeId);
      void remote.detach({ attachmentId: previous.attachmentId }).catch(() => undefined);
    }
    return true;
  };

  return {
    onRemoteConnectionLog: (handler) => {
      const remote = options.getRemoteService();
      if (!remote) {
        return window.mode.onRemoteConnectionLog(handler);
      }
      const disposable = remote.onDynamicConnectionEvent()((event) => {
        if (event.kind === "log") {
          handler(event.entry);
        }
      });
      return () => disposable.dispose();
    },
    onRemoteSessionClosed: (handler) => {
      const remote = options.getRemoteService();
      if (!remote) {
        return window.mode.onRemoteSessionClosed(handler);
      }
      const disposable = remote.onDynamicConnectionEvent()((event) => {
        if (event.kind === "closed") {
          remoteAttachments.delete(event.sessionId);
          handler({
            sessionId: event.sessionId,
            reason: "host-exit",
            exitCode: event.exitCode,
            signal: event.signal,
            ...(event.error ? { error: event.error } : {}),
          });
        }
      });
      return () => disposable.dispose();
    },
    onBotRemoteWorkspaceReconnected: (handler) => {
      const remote = options.getRemoteService();
      if (!remote) {
        // Tauri 注入层目前仍是留痕空操作；Host 服务缺席时保持原语义（不伪造事件）。
        return window.mode.onBotRemoteWorkspaceReconnected(handler);
      }
      const disposable = remote.onDynamicConnectionEvent()((event) => {
        if (event.kind === "botReconnected") {
          handler(event.event);
        }
      });
      return () => disposable.dispose();
    },
    connectRemote: async (remoteOptions, requestId, context) => {
      const remote = options.getRemoteService();
      if (!remote) {
        return window.mode.connectRemote(remoteOptions, requestId, context);
      }
      const workspacePath = context?.workspacePath?.trim() || "/";
      const workspaceIdentity =
        context?.workspaceIdentity?.trim() ||
        buildRemoteWorkspaceIdentity(workspacePath, remoteOptions);
      const result = await remote.connect({
        target: remoteOptions,
        ...(requestId ? { requestId } : {}),
        workspacePath,
        workspaceIdentity,
      });
      if (!result.ok) {
        return { success: false, error: result.error };
      }
      const sessionId = result.descriptor.remoteSessionId;
      try {
        const attached = await attachRemoteSession(sessionId, remoteOptions);
        if (!attached) {
          await remote.dispose({ sessionId }).catch(() => undefined);
          return { success: false, error: "页面桥不可用，无法建立 scoped attachment" };
        }
      } catch (error) {
        await remote.dispose({ sessionId }).catch(() => undefined);
        return {
          success: false,
          error: error instanceof Error ? error.message : String(error),
        };
      }
      return { success: true, sessionId };
    },
    cancelPendingRemoteConnection: async (requestId) => {
      const remote = options.getRemoteService();
      if (!remote) {
        await window.mode.cancelPendingRemoteConnection?.(requestId);
        return;
      }
      if (requestId) {
        await remote.cancelConnect({ requestId });
      }
    },
    bindRemoteWorkspaceSessionContext: async (context) => {
      const remote = options.getRemoteService();
      if (!remote) {
        await window.mode.bindRemoteWorkspaceSessionContext?.(context);
        return;
      }
      await remote.bindContext({
        sessionId: context.remoteSessionId,
        workspacePath: context.workspacePath,
        ...(context.workspaceIdentity ? { workspaceIdentity: context.workspaceIdentity } : {}),
      });
      // bind 换代：旧 attachment 的 scope 已与 session 不匹配，必须换一个新端口。
      const session = await remote.getSession({ sessionId: context.remoteSessionId });
      if (!session) {
        throw new Error(`远程 session 不存在：${context.remoteSessionId}`);
      }
      const attached = await attachRemoteSession(context.remoteSessionId, session.target);
      if (!attached) {
        throw new Error("页面桥不可用，无法换代 scoped attachment");
      }
    },
    disposeRemoteSession: async (sessionId) => {
      const remote = options.getRemoteService();
      if (!remote) {
        await window.mode.disposeRemoteSession(sessionId);
        return;
      }
      const attachment = remoteAttachments.get(sessionId);
      if (attachment) {
        remoteAttachments.delete(sessionId);
        hostBridgeScopedPorts()?.releaseScopedServicePort?.(attachment.scopeId);
        await remote.detach({ attachmentId: attachment.attachmentId }).catch(() => undefined);
      }
      await remote.dispose({ sessionId });
    },
  };
}
