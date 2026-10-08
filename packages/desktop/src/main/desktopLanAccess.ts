import { randomUUID } from "node:crypto";
import { ipcMain } from "electron";
import {
  HostMessageTypes,
  HostResponseTypes,
  PlatformChannels,
  type HostLanAccessStateResponse,
  type HostLanPairPeerResultResponse,
  type HostLanPairedPeersListResponse,
  type HostLanPairedPeerRemoveResponse,
  type HostLanPairedPeerWorkspacesResponse,
  type LanAccessPairResult,
  type LanAccessState,
  type LanPairPeerRequest,
} from "@mode/shared";
import { discoverLanPeers } from "./desktopLanDiscovery.js";
import { listLiveHostProcesses } from "./resourceManagerWindow.js";

/**
 * 局域网访问（服务端）main 侧客户端：
 * 状态与动作都转发给「owner host」——第一个存活的窗口 Host 进程，
 * 保证一台机器同一时刻只有一个监听实例；它退出后下一次请求自动改选新 owner。
 */

const LAN_ACCESS_REQUEST_TIMEOUT_MS = 8_000;

interface PendingLanAccessRequest {
  resolve: (result: HostLanAccessStateResponse) => void;
}

interface PendingLanPairRequest {
  resolve: (result: HostLanPairPeerResultResponse) => void;
}

export type LanPairedPeerOpResponse =
  | HostLanPairedPeersListResponse
  | HostLanPairedPeerWorkspacesResponse
  | HostLanPairedPeerRemoveResponse;

const pendingLanPeerOps = new Map<
  string,
  { resolve: (result: LanPairedPeerOpResponse) => void; reject: (error: Error) => void }
>();

/** host → main：「我配对的对端」三个动作的结果统一回收（按 requestId 关联）。 */
export function resolveLanPairedPeerOpResult(result: LanPairedPeerOpResponse): void {
  const pending = pendingLanPeerOps.get(result.requestId);
  if (!pending) {
    return;
  }
  pendingLanPeerOps.delete(result.requestId);
  if (!result.ok) {
    pending.reject(new Error(result.error ?? "局域网对端操作失败"));
    return;
  }
  pending.resolve(result);
}

const pendingRequests = new Map<string, PendingLanAccessRequest>();
const pendingPairRequests = new Map<string, PendingLanPairRequest>();
let ownerLabel: string | null = null;

/** desktopHostProcess 收到 Host 回帖时调用。 */
export function resolveLanAccessStateResult(
  label: string,
  result: HostLanAccessStateResponse,
): void {
  const pending = pendingRequests.get(result.requestId);
  if (!pending) {
    return;
  }
  pendingRequests.delete(result.requestId);
  pending.resolve(result);
}

/** desktopHostProcess 收到 Host 配对回帖时调用。 */
export function resolveLanPairPeerResult(
  label: string,
  result: HostLanPairPeerResultResponse,
): void {
  const pending = pendingPairRequests.get(result.requestId);
  if (!pending) {
    return;
  }
  pendingPairRequests.delete(result.requestId);
  pending.resolve(result);
}

async function callLanPairHost(request: LanPairPeerRequest): Promise<LanAccessPairResult> {
  const owner = pickOwner();
  if (!owner) {
    throw new Error("当前没有可用的窗口宿主进程");
  }
  const requestId = randomUUID();
  return await new Promise<LanAccessPairResult>((resolve, reject) => {
    const timer = setTimeout(() => {
      pendingPairRequests.delete(requestId);
      reject(new Error("配对请求超时"));
    }, LAN_ACCESS_REQUEST_TIMEOUT_MS);
    pendingPairRequests.set(requestId, {
      resolve: (result) => {
        clearTimeout(timer);
        if (!result.ok || !result.token) {
          reject(new Error(result.error ?? "配对失败"));
          return;
        }
        resolve({
          token: result.token,
          serverId: result.serverId ?? "",
          ...(result.name ? { name: result.name } : {}),
        });
      },
    });
    try {
      owner.child.postMessage({
        type: HostMessageTypes.LanPairPeer,
        requestId,
        host: request.host,
        port: request.port,
        code: request.code,
        ...(request.label ? { label: request.label } : {}),
      });
    } catch (error) {
      clearTimeout(timer);
      pendingPairRequests.delete(requestId);
      reject(error instanceof Error ? error : new Error(String(error)));
    }
  });
}

/** 「我配对的对端」三动作：与局域网控制同款的 owner host 转发 + 超时回收。 */
async function callLanPairedPeerHost(
  type:
    | typeof HostMessageTypes.LanPairedPeersList
    | typeof HostMessageTypes.LanPairedPeerWorkspaces
    | typeof HostMessageTypes.LanPairedPeerRemove,
  serverId?: string,
): Promise<LanPairedPeerOpResponse> {
  const owner = pickOwner();
  if (!owner) {
    throw new Error("当前没有可用的窗口宿主进程");
  }
  const requestId = randomUUID();
  return await new Promise<LanPairedPeerOpResponse>((resolve, reject) => {
    const timer = setTimeout(() => {
      pendingLanPeerOps.delete(requestId);
      reject(new Error("局域网对端请求超时"));
    }, LAN_ACCESS_REQUEST_TIMEOUT_MS);
    pendingLanPeerOps.set(requestId, {
      resolve: (result) => {
        clearTimeout(timer);
        resolve(result);
      },
      reject: (error) => {
        clearTimeout(timer);
        reject(error);
      },
    });
    try {
      owner.child.postMessage({
        type,
        requestId,
        ...(serverId ? { serverId } : {}),
      });
    } catch (error) {
      clearTimeout(timer);
      pendingLanPeerOps.delete(requestId);
      reject(error instanceof Error ? error : new Error(String(error)));
    }
  });
}

function pickOwner(): { label: string; child: { postMessage: (message: unknown) => void } } | null {
  const hosts = listLiveHostProcesses();
  if (hosts.length === 0) {
    ownerLabel = null;
    return null;
  }
  const owner = ownerLabel ? hosts.find((host) => host.label === ownerLabel) : undefined;
  if (owner) {
    return owner;
  }
  const first = hosts[0];
  ownerLabel = first.label;
  return first;
}

type LanAccessAction = "get-state" | "set-enabled" | "create-pair-code" | "remove-client" | "reset-tokens";

const ACTION_MESSAGE_TYPE: Record<LanAccessAction, string> = {
  "get-state": HostMessageTypes.LanAccessGetState,
  "set-enabled": HostMessageTypes.LanAccessSetEnabled,
  "create-pair-code": HostMessageTypes.LanAccessCreatePairCode,
  "remove-client": HostMessageTypes.LanAccessRemoveClient,
  "reset-tokens": HostMessageTypes.LanAccessResetTokens,
};

async function callLanAccessHost(
  action: LanAccessAction,
  payload: { enabled?: boolean; clientId?: string } = {},
): Promise<LanAccessState> {
  const owner = pickOwner();
  if (!owner) {
    throw new Error("当前没有可用的窗口宿主进程");
  }
  const requestId = randomUUID();
  const message = {
    type: ACTION_MESSAGE_TYPE[action],
    requestId,
    ...payload,
  };
  return await new Promise<LanAccessState>((resolve, reject) => {
    const timer = setTimeout(() => {
      pendingRequests.delete(requestId);
      reject(new Error("局域网访问请求超时"));
    }, LAN_ACCESS_REQUEST_TIMEOUT_MS);
    pendingRequests.set(requestId, {
      resolve: (result) => {
        clearTimeout(timer);
        if (!result.ok || !result.state) {
          reject(new Error(result.error ?? "局域网访问操作失败"));
          return;
        }
        resolve(result.state);
      },
    });
    try {
      owner.child.postMessage(message);
    } catch (error) {
      clearTimeout(timer);
      pendingRequests.delete(requestId);
      reject(error instanceof Error ? error : new Error(String(error)));
    }
  });
}

export function registerLanAccessIpcHandlers(options: {
  logger: {
    warn: (...args: unknown[]) => void;
    error: (...args: unknown[]) => void;
  };
}): void {
  const handle = (action: LanAccessAction, payload: { enabled?: boolean; clientId?: string } = {}) =>
    async (): Promise<LanAccessState> => {
      try {
        return await callLanAccessHost(action, payload);
      } catch (error) {
        options.logger.warn("[lan-access] action failed:", {
          action,
          message: error instanceof Error ? error.message : String(error),
        });
        throw error;
      }
    };

  ipcMain.handle(PlatformChannels.DiscoverLanPeers, async () => {
    try {
      return await discoverLanPeers();
    } catch (error) {
      options.logger.warn("[lan-access] discovery failed:", error);
      return [];
    }
  });
  ipcMain.handle(PlatformChannels.PairLanPeer, async (_event, rawPayload: unknown) => {
    const payload = rawPayload as Partial<LanPairPeerRequest> | undefined;
    const host = typeof payload?.host === "string" ? payload.host.trim() : "";
    const port = typeof payload?.port === "number" ? payload.port : Number.NaN;
    const code = typeof payload?.code === "string" ? payload.code.trim() : "";
    if (!host || !Number.isInteger(port) || port <= 0 || port > 65535 || !code) {
      throw new Error("配对请求缺少地址或配对码");
    }
    return await callLanPairHost({
      host,
      port,
      code,
      ...(typeof payload?.label === "string" && payload.label.trim()
        ? { label: payload.label.trim() }
        : {}),
    });
  });
  ipcMain.handle(PlatformChannels.GetLanAccessState, handle("get-state"));
  ipcMain.handle(PlatformChannels.CreateLanAccessPairCode, handle("create-pair-code"));
  ipcMain.handle(PlatformChannels.ResetLanAccessTokens, handle("reset-tokens"));
  ipcMain.handle(PlatformChannels.SetLanAccessEnabled, async (_event, enabled: unknown) => {
    return await handle("set-enabled", { enabled: enabled === true })();
  });
  ipcMain.handle(PlatformChannels.GetLanPairedPeers, async () => {
    try {
      const result = await callLanPairedPeerHost(HostMessageTypes.LanPairedPeersList);
      if (result.type !== "lan-paired-peers-list-result") {
        throw new Error("局域网对端响应类型不匹配");
      }
      return result.peers ?? [];
    } catch (error) {
      options.logger.warn("[lan-access] list paired peers failed:", {
        message: error instanceof Error ? error.message : String(error),
      });
      throw error;
    }
  });
  ipcMain.handle(PlatformChannels.GetLanPeerWorkspaces, async (_event, rawServerId: unknown) => {
    const serverId = typeof rawServerId === "string" ? rawServerId.trim() : "";
    if (!serverId) {
      throw new Error("serverId 不能为空");
    }
    try {
      const result = await callLanPairedPeerHost(HostMessageTypes.LanPairedPeerWorkspaces, serverId);
      if (result.type !== "lan-paired-peer-workspaces-result") {
        throw new Error("局域网对端响应类型不匹配");
      }
      return {
        ...(result.name ? { name: result.name } : {}),
        workspaces: result.workspaces ?? [],
      };
    } catch (error) {
      options.logger.warn("[lan-access] get peer workspaces failed:", {
        serverId,
        message: error instanceof Error ? error.message : String(error),
      });
      throw error;
    }
  });
  ipcMain.handle(PlatformChannels.RemoveLanPairedPeer, async (_event, rawServerId: unknown) => {
    const serverId = typeof rawServerId === "string" ? rawServerId.trim() : "";
    if (!serverId) {
      throw new Error("serverId 不能为空");
    }
    try {
      await callLanPairedPeerHost(HostMessageTypes.LanPairedPeerRemove, serverId);
    } catch (error) {
      options.logger.warn("[lan-access] remove paired peer failed:", {
        serverId,
        message: error instanceof Error ? error.message : String(error),
      });
      throw error;
    }
  });
  ipcMain.handle(PlatformChannels.RemoveLanAccessClient, async (_event, clientId: unknown) => {
    if (typeof clientId !== "string" || !clientId.trim()) {
      throw new Error("clientId 不能为空");
    }
    return await handle("remove-client", { clientId: clientId.trim() })();
  });
}

/** host 退出时清掉它的 owner 标记与在途请求，避免把旧结果写给新宿主。 */
export function forgetLanAccessHost(label: string): void {
  for (const [requestId, pending] of pendingPairRequests) {
    // 宿主退出时把在途配对请求一次性失败掉，避免 UI 一直转圈。
    pending.resolve({
      type: HostResponseTypes.LanPairPeerResult,
      requestId,
      ok: false,
      error: "宿主进程已退出",
    });
    pendingPairRequests.delete(requestId);
  }
  if (ownerLabel === label) {
    ownerLabel = null;
  }
}
