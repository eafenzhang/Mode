/**
 * Web/fpk 作为 LAN 主控（docs/specs/web-remote-workspace.md）。
 *
 * 唯一所有者：
 * - 配对记录/令牌仍归 `ICredentialService`（`lan:peer:*`，与 desktop 同 key）；
 * - 本条连接（attach 后的 `/ws/host`）归本 controller 的 `sessions`；
 * - 浏览器 `/ws/remote/:id` 通过 `takeRemoteConnection` 接管 connection，并在 WS close 时
 *   dispose，不在 server 里保留第二个已消费 session。
 */
import { randomUUID } from "node:crypto";
import {
  formatLogPrefix,
  resolveLanTargetEndpoints,
  type LanAccessPairResult,
  type LanConnectOptions,
  type LanDiscoveredPeer,
  type LanPairedPeer,
  type LanPairedPeerWorkspaces,
  type LanPeerEndpoint,
} from "@mode/shared";
import type { ICredentialService, IServiceAccessor } from "@mode/services";
import {
  attachLanRemoteConnection,
  pairLanPeer,
  readLanPeerEndpoints,
  type LanRemoteConnection,
} from "./lanRemoteAttach.js";
import { discoverLanPeers } from "./lanDiscovery.js";
import {
  getLanPairedPeerWorkspaces,
  listLanPairedPeers,
  removeLanPairedPeerRecord,
  renameLanPairedPeerRecord,
} from "./lanPairedPeers.js";
import { endpointRequest, selectReachableLanPeerEndpoint } from "./lanPeerEndpoints.js";

const LOG_PREFIX = formatLogPrefix("mode-server:lan-peer", process.pid);

export interface LanRemoteConnectionView {
  services: IServiceAccessor;
  dispose(): Promise<void>;
}

const UNCLAIMED_SESSION_TTL_MS = 60_000;

interface SessionEntry {
  connection: LanRemoteConnection;
  /** 对端连接断开时通知已接管的浏览器 WS，避免页面停在半死服务上。 */
  onClosed?: () => void;
}

/**
 * 连接目标解析的安全边界：带 `serverId` 时端点与令牌**只**来自服务端配对记录，
 * 忽略浏览器提交的 endpoints/token——否则被篡改的 renderer 可以把已存令牌引到
 * 任意地址（令牌外泄）。无 serverId 的手动目标才使用客户端提供的端点/令牌。
 */
export function resolveLanConnectTarget(
  target: LanConnectOptions,
  pairedRecord: { endpoints: LanPeerEndpoint[]; token?: string } | null,
): { endpoints: LanPeerEndpoint[]; token?: string } {
  if (target.serverId?.trim()) {
    if (!pairedRecord) {
      throw new Error("该配对缺少地址信息，请重新配对");
    }
    return {
      endpoints: pairedRecord.endpoints,
      ...(pairedRecord.token ? { token: pairedRecord.token } : {}),
    };
  }
  return {
    endpoints: target.endpoints?.length ? target.endpoints : resolveLanTargetEndpoints(target),
    ...(target.token?.trim() ? { token: target.token.trim() } : {}),
  };
}

export class LanPeerController {
  private readonly sessions = new Map<string, SessionEntry>();
  /** 浏览器拿到 id 但没来连 `/ws/remote/:id` 时的兜底释放，避免半开 attachment 泄漏。 */
  private readonly pendingTimers = new Map<string, ReturnType<typeof setTimeout>>();

  constructor(
    private readonly credentials: ICredentialService,
    private readonly log: (...args: unknown[]) => void = (...args) =>
      console.log(LOG_PREFIX, ...args),
  ) {}

  listPeers(): Promise<LanPairedPeer[]> {
    return listLanPairedPeers(this.credentials);
  }

  async pairPeer(params: {
    host: string;
    port: number;
    code: string;
    label?: string;
    scheme?: "http" | "https";
    path?: string;
  }): Promise<LanPairedPeer> {
    // pairLanPeer 的返回值含长期令牌；HTTP 面只返回列表投影，令牌不出服务端。
    const result: LanAccessPairResult = await pairLanPeer({
      credentials: this.credentials,
      ...params,
    });
    const peers = await this.listPeers();
    return (
      peers.find((peer) => peer.serverId === result.serverId) ?? {
        serverId: result.serverId,
        host: params.host,
        port: params.port,
      }
    );
  }

  removePeer(serverId: string): Promise<void> {
    return removeLanPairedPeerRecord(this.credentials, serverId);
  }

  renamePeer(serverId: string, name: string): Promise<void> {
    return renameLanPairedPeerRecord(this.credentials, serverId, name);
  }

  getPeerWorkspaces(serverId: string): Promise<LanPairedPeerWorkspaces> {
    return getLanPairedPeerWorkspaces(this.credentials, serverId);
  }

  discoverPeers(): Promise<LanDiscoveredPeer[]> {
    return discoverLanPeers();
  }

  async connectRemote(target: LanConnectOptions): Promise<{ remoteSessionId: string }> {
    const pairedRecord = target.serverId?.trim()
      ? await readLanPeerEndpoints(this.credentials, target.serverId)
      : null;
    const { endpoints, token } = resolveLanConnectTarget(target, pairedRecord);
    if (!token) {
      throw new Error("尚未配对或令牌缺失，请重新配对");
    }

    const selection = await selectReachableLanPeerEndpoint({ endpoints, token });
    if (!selection.ok) {
      throw selection.error;
    }

    const remoteSessionId = randomUUID();
    const entry: SessionEntry = {
      connection: await attachLanRemoteConnection({
        ...endpointRequest(selection.endpoint),
        token,
        info: selection.info,
        onClose: () => {
          if (this.sessions.get(remoteSessionId) === entry) {
            this.sessions.delete(remoteSessionId);
          }
          entry.onClosed?.();
        },
      }),
    };
    this.sessions.set(remoteSessionId, entry);
    const timer = setTimeout(() => {
      this.pendingTimers.delete(remoteSessionId);
      void this.releaseRemote(remoteSessionId);
    }, UNCLAIMED_SESSION_TTL_MS);
    timer.unref();
    this.pendingTimers.set(remoteSessionId, timer);
    this.log(
      `attached ${remoteSessionId} endpoint=${selection.endpoint.host}:${String(selection.endpoint.port)}`,
    );
    return { remoteSessionId };
  }

  /**
   * WS 接管：取走即从 map 删除；close 由 WS handler 负责 dispose。
   * `onPeerClosed` 在对端连接先断时关闭浏览器 WS（页面随后走正常断线收口）。
   */
  takeRemoteConnection(
    remoteSessionId: string,
    onPeerClosed?: () => void,
  ): LanRemoteConnectionView | null {
    const entry = this.sessions.get(remoteSessionId);
    if (!entry) {
      return null;
    }
    entry.onClosed = onPeerClosed;
    this.sessions.delete(remoteSessionId);
    this.clearPendingTimer(remoteSessionId);
    return entry.connection;
  }

  async releaseRemote(remoteSessionId: string): Promise<void> {
    this.clearPendingTimer(remoteSessionId);
    const entry = this.sessions.get(remoteSessionId);
    if (!entry) {
      return;
    }
    this.sessions.delete(remoteSessionId);
    await entry.connection.dispose().catch(() => undefined);
  }

  async disposeAll(): Promise<void> {
    for (const timer of this.pendingTimers.values()) {
      clearTimeout(timer);
    }
    this.pendingTimers.clear();
    const connections = [...this.sessions.values()];
    this.sessions.clear();
    await Promise.all(
      connections.map((entry) => entry.connection.dispose().catch(() => undefined)),
    );
  }

  private clearPendingTimer(remoteSessionId: string): void {
    const timer = this.pendingTimers.get(remoteSessionId);
    if (timer) {
      clearTimeout(timer);
      this.pendingTimers.delete(remoteSessionId);
    }
  }
}
