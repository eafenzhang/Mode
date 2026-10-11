/**
 * main 侧 RD host 桥：main ↔ host 的 RD 消息收发（低层传输）。
 *
 * 与 host 的镜像契约见 hostMessages.ts；本模块只做：
 * - 出站：把消息发给「RD 宿主」——收到过 host→main RD 消息的那个 host（更准确），
 *   没有则退回第一个存活 host（同 desktopLanAccess 的 owner host 选择先例）；
 * - 入站：desktopHostProcess 收到 parentPort 消息时按类型分流到这里（共享 schema 之前）；
 * - 请求/响应关联：server-state / peer-probe / peer-session-open 按 requestId 回收（10s 超时）；
 * - host 退出清理：宿主消失即把在途请求失败、并通知会话协调器收口（信令通道已断）。
 */
import { randomUUID } from "node:crypto";
import {
  RemoteDesktopHostMessageTypes,
  isRemoteDesktopMessageTypeTag,
  remoteDesktopMessageFromHostSchema,
  type RemoteDesktopMessageFromHost,
  type RemoteDesktopMessageToHost,
} from "./hostMessages.js";
import { listLiveHostProcesses } from "./resourceManagerWindow.js";

const RD_REQUEST_TIMEOUT_MS = 10_000;

export interface RemoteDesktopHostLogger {
  info(...args: unknown[]): void;
  warn(...args: unknown[]): void;
}

/** 会话侧回调：hostBridge 不 import coordinator，避免模块环。 */
export interface RemoteDesktopHostSessionHandlers {
  handleConsentRequest(message: Extract<RemoteDesktopMessageFromHost, { type: typeof RemoteDesktopHostMessageTypes.RemoteDesktopConsentRequest }>): void;
  handleSignalFromHost(message: Extract<RemoteDesktopMessageFromHost, { type: typeof RemoteDesktopHostMessageTypes.RemoteDesktopSignalFromHost }>): void;
  handleSessionClosed(message: Extract<RemoteDesktopMessageFromHost, { type: typeof RemoteDesktopHostMessageTypes.RemoteDesktopSessionClosed }>): void;
  handleHostGone(): void;
}

type PendingRequest =
  | {
      kind: "server-state";
      resolve: (value: { enabled: boolean; authorizedClients: Array<{ id: string; label: string }> }) => void;
      reject: (error: Error) => void;
      timer: ReturnType<typeof setTimeout>;
    }
  | {
      kind: "peer-probe";
      resolve: (value: { online: boolean; remoteDesktop: boolean; name?: string }) => void;
      reject: (error: Error) => void;
      timer: ReturnType<typeof setTimeout>;
    }
  | {
      kind: "peer-session-open";
      resolve: (value: { ok: true; wsUrl: string } | { ok: false; errorCode?: string; error: string }) => void;
      reject: (error: Error) => void;
      timer: ReturnType<typeof setTimeout>;
    };

let logger: RemoteDesktopHostLogger | null = null;
let sessionHandlers: RemoteDesktopHostSessionHandlers | null = null;
/** 收到过 RD 消息的 host（consent/signal 会话宿主）；host 退出时清空。 */
let activeHostLabel: string | null = null;
const pendingRequests = new Map<string, PendingRequest>();

export function configureRemoteDesktopHostBridge(options: {
  logger: RemoteDesktopHostLogger;
  sessionHandlers: RemoteDesktopHostSessionHandlers;
}): void {
  logger = options.logger;
  sessionHandlers = options.sessionHandlers;
}

function pickHost(): { label: string; child: { postMessage: (message: unknown) => void } } | null {
  const hosts = listLiveHostProcesses();
  if (hosts.length === 0) {
    activeHostLabel = null;
    return null;
  }
  const active = activeHostLabel ? hosts.find((host) => host.label === activeHostLabel) : undefined;
  if (active) {
    return active;
  }
  activeHostLabel = hosts[0]?.label ?? null;
  return hosts[0] ?? null;
}

/** 出站：发给 RD 宿主。返回 false = 当前没有可用 host（会话应据此收口）。 */
export function sendRemoteDesktopMessageToHost(message: RemoteDesktopMessageToHost): boolean {
  const host = pickHost();
  if (!host) {
    logger?.warn("[remote-desktop] no live host process to send message:", message.type);
    return false;
  }
  try {
    host.child.postMessage(message);
    return true;
  } catch (error) {
    logger?.warn(
      "[remote-desktop] postMessage to host failed:",
      error instanceof Error ? error.message : error,
    );
    return false;
  }
}

function settlePending<T extends PendingRequest["kind"]>(
  requestId: string,
  kind: T,
  settle: (pending: Extract<PendingRequest, { kind: T }>) => void,
): void {
  const pending = pendingRequests.get(requestId);
  if (!pending || pending.kind !== kind) {
    return;
  }
  pendingRequests.delete(requestId);
  clearTimeout(pending.timer);
  settle(pending as Extract<PendingRequest, { kind: T }>);
}

/** 联合体的 Omit 会塌缩成公共键集合，这里保持分配式（每个成员各自去掉 requestId）。 */
type DistributiveOmit<T, K extends PropertyKey> = T extends unknown ? Omit<T, K> : never;

function requestToHost(
  message: DistributiveOmit<RemoteDesktopMessageToHost, "requestId">,
  kind: PendingRequest["kind"],
): Promise<unknown> {
  const requestId = randomUUID();
  return new Promise<unknown>((resolve, reject) => {
    const timer = setTimeout(() => {
      pendingRequests.delete(requestId);
      reject(new Error("远程桌面宿主请求超时"));
    }, RD_REQUEST_TIMEOUT_MS);
    pendingRequests.set(requestId, { kind, resolve, reject, timer } as PendingRequest);
    if (
      !sendRemoteDesktopMessageToHost({
        ...message,
        requestId,
      } as RemoteDesktopMessageToHost)
    ) {
      pendingRequests.delete(requestId);
      clearTimeout(timer);
      reject(new Error("当前没有可用的窗口宿主进程"));
    }
  });
}

export async function requestRemoteDesktopServerState(): Promise<{
  enabled: boolean;
  authorizedClients: Array<{ id: string; label: string }>;
}> {
  const promise = requestToHost(
    { type: RemoteDesktopHostMessageTypes.RemoteDesktopServerStateRequest },
    "server-state",
  );
  return (await promise) as { enabled: boolean; authorizedClients: Array<{ id: string; label: string }> };
}

export async function requestRemoteDesktopSetServerEnabled(enabled: boolean): Promise<{
  enabled: boolean;
  authorizedClients: Array<{ id: string; label: string }>;
}> {
  const promise = requestToHost(
    { type: RemoteDesktopHostMessageTypes.RemoteDesktopSetServerEnabled, enabled },
    "server-state",
  );
  return (await promise) as { enabled: boolean; authorizedClients: Array<{ id: string; label: string }> };
}

export async function requestRemoteDesktopRevokeClient(clientId: string): Promise<{
  enabled: boolean;
  authorizedClients: Array<{ id: string; label: string }>;
}> {
  const promise = requestToHost(
    { type: RemoteDesktopHostMessageTypes.RemoteDesktopRevokeClient, clientId },
    "server-state",
  );
  return (await promise) as { enabled: boolean; authorizedClients: Array<{ id: string; label: string }> };
}

export async function requestRemoteDesktopPeerProbe(serverId: string): Promise<{
  online: boolean;
  remoteDesktop: boolean;
  name?: string;
}> {
  const promise = requestToHost(
    { type: RemoteDesktopHostMessageTypes.RemoteDesktopPeerProbe, serverId },
    "peer-probe",
  );
  return (await promise) as { online: boolean; remoteDesktop: boolean; name?: string };
}

export async function requestRemoteDesktopPeerSessionOpen(serverId: string): Promise<
  { ok: true; wsUrl: string } | { ok: false; errorCode?: string; error: string }
> {
  const promise = requestToHost(
    { type: RemoteDesktopHostMessageTypes.RemoteDesktopPeerSessionOpen, serverId },
    "peer-session-open",
  );
  return (await promise) as { ok: true; wsUrl: string } | { ok: false; errorCode?: string; error: string };
}

/**
 * main 侧入站分流：类型标签命中 RD 集合才消费（返回 true），否则调用方继续共享 schema 校验。
 * 标签命中但 schema 不合法：记日志并按未消费处理（不进入共享联合体，避免双报错噪音）。
 */
export function handleRemoteDesktopHostMessage(label: string, data: unknown): boolean {
  const type = (data as { type?: unknown } | null)?.type;
  if (typeof type !== "string" || !isRemoteDesktopMessageTypeTag(type)) {
    return false;
  }
  const parsed = remoteDesktopMessageFromHostSchema.safeParse(data);
  if (!parsed.success) {
    logger?.warn(`[remote-desktop] invalid host message dropped: ${type}`);
    return true;
  }
  activeHostLabel = label;
  const message = parsed.data;
  if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopConsentRequest) {
    sessionHandlers?.handleConsentRequest(message);
    return true;
  }
  if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopSignalFromHost) {
    sessionHandlers?.handleSignalFromHost(message);
    return true;
  }
  if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopSessionClosed) {
    sessionHandlers?.handleSessionClosed(message);
    return true;
  }
  if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopServerStateResult) {
    settlePending(message.requestId, "server-state", (pending) => {
      if (!message.ok) {
        pending.reject(new Error(message.error ?? "读取远程桌面设置失败"));
        return;
      }
      pending.resolve({ enabled: message.enabled, authorizedClients: message.authorizedClients });
    });
    return true;
  }
  if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopPeerProbeResult) {
    settlePending(message.requestId, "peer-probe", (pending) => {
      pending.resolve({
        online: message.online,
        remoteDesktop: message.remoteDesktop,
        ...(message.name ? { name: message.name } : {}),
      });
    });
    return true;
  }
  settlePending(message.requestId, "peer-session-open", (pending) => {
    pending.resolve(
      message.ok && message.wsUrl
        ? { ok: true, wsUrl: message.wsUrl }
        : {
            ok: false,
            ...(message.errorCode ? { errorCode: message.errorCode } : {}),
            error: message.error ?? "对端会话请求失败",
          },
    );
  });
  return true;
}

/** host 退出（desktopHostProcess）：在途请求失败 + 会话协调器收口（信令通道已断）。 */
export function forgetRemoteDesktopHost(label: string): void {
  for (const [requestId, pending] of pendingRequests) {
    pending.reject(new Error("宿主进程已退出"));
    pendingRequests.delete(requestId);
    clearTimeout(pending.timer);
  }
  if (activeHostLabel === label) {
    activeHostLabel = null;
    sessionHandlers?.handleHostGone();
  }
}
