import type { Event } from "@mode/rpc";
import type {
  RemoteConnectionRuntimeLog,
  RemoteTarget,
  WindowHostRemoteWorkspaceDescriptor,
} from "@mode/shared";
import { ServiceChannels } from "@mode/shared";

import { createServiceDescriptor } from "../descriptors.js";

/**
 * 远程工作区管理面（LR-4，浏览器安全契约）。
 *
 * 状态所有者是 Host 内的 `windowRemoteConnectionRegistry`（每窗口一个）：
 * logical session、连接复用、workspace bind 与 attachment 都由它管。renderer 只通过本频道
 * 发起/取消连接、绑定上下文、申请 scoped attachment；scoped service 走单独的 MessagePort。
 *
 * 当前 Tauri Host 只接 LAN 目标；SSH/WSL/Docker 明确返回不支持（不伪装成功）。
 */

export interface RemoteConnectParams {
  target: RemoteTarget;
  /** 幂等/取消关联 id；缺省由 Host 生成。 */
  requestId?: string;
  workspacePath?: string;
  workspaceIdentity?: string;
}

export type RemoteConnectResult =
  | { ok: true; descriptor: WindowHostRemoteWorkspaceDescriptor }
  | { ok: false; error: string };

export interface RemoteBindContextParams {
  sessionId: string;
  workspacePath: string;
  workspaceIdentity?: string;
}

export interface RemoteSessionParams {
  sessionId: string;
}

export interface RemoteDetachParams {
  attachmentId: string;
}

export interface RemoteAttachmentDescriptor {
  /** Host 分配的 scoped 帧路由 id（不复用）。 */
  scopeId: number;
  attachmentId: string;
  sessionId: string;
  capabilities?: { computerUse?: boolean };
}

export interface RemoteWorkspaceSessionSnapshot {
  remoteSessionId: string;
  requestId: string;
  target: RemoteTarget;
  workspacePath?: string;
  workspaceIdentity?: string;
  generation: number;
  state: "connecting" | "online" | "closing" | "failed" | "disconnected";
  sourceAvailability: "online" | "offline";
}

export type RemoteWorkspaceConnectionEvent =
  | { kind: "log"; entry: RemoteConnectionRuntimeLog }
  | { kind: "connected"; descriptor: WindowHostRemoteWorkspaceDescriptor }
  | { kind: "connectFailed"; requestId: string; error: string }
  | {
      kind: "closed";
      sessionId: string;
      exitCode: number | null;
      signal: string | null;
      error?: string;
    };

export interface IRemoteService {
  /** 建立 logical session；失败返回可读错误（不抛跨进程细节）。 */
  connect(params: RemoteConnectParams): Promise<RemoteConnectResult>;
  /** 取消尚未完成的连接（按 requestId）。 */
  cancelConnect(params: { requestId: string }): Promise<void>;
  /** 绑定 canonical workspace 身份；之后 attach 才能拿到 scoped services。 */
  bindContext(params: RemoteBindContextParams): Promise<void>;
  /** 释放 logical session 及其 Attachment。 */
  dispose(params: RemoteSessionParams): Promise<void>;
  /**
   * 申请一个 scoped attachment：Host 建好 scoped `ChannelServer` 后返回 id；
   * renderer 再用页面桥建端口并回 ACK（`scope-ready` 控制帧）后才开始 RPC。
   */
  attach(params: RemoteSessionParams): Promise<RemoteAttachmentDescriptor>;
  detach(params: RemoteDetachParams): Promise<void>;
  getSession(params: RemoteSessionParams): Promise<RemoteWorkspaceSessionSnapshot | null>;
  listSessions(): Promise<RemoteWorkspaceSessionSnapshot[]>;
  /**
   * 非幂等连接事件（动态事件）：连接过程日志、connected/failed、closed。
   * 只发给当前订阅者，不缓冲、不重放。
   */
  onDynamicConnectionEvent(sessionId?: string): Event<RemoteWorkspaceConnectionEvent>;
}

export const IRemoteService = createServiceDescriptor<IRemoteService>(ServiceChannels.Remote);
