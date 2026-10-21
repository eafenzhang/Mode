import { randomBytes, randomUUID, timingSafeEqual } from "node:crypto";
import type { WebSocket } from "ws";
import {
  LAN_ACCESS_CLIENTS_CREDENTIAL_KEY,
  LAN_ACCESS_DEFAULT_PORT,
  LAN_ACCESS_PAIR_CODE_TTL_MS,
  LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY,
  buildLanAccessClientTokenKey,
  type LanAccessClientRecord,
  type LanAccessPairCode,
  type LanAccessPairRequest,
  type LanAccessPairResult,
  type LanAccessState,
  type LanPublicRelayInfo,
  type ServerRemoteWorkspaceInfo,
  isModeCuaInternalFeatureEnabled,
} from "@mode/shared";
import type { RemoteDesktopErrorCode, RemoteDesktopQuality } from "@mode/shared/remote-desktop";
import { createHttpServer } from "@mode/server";
import { type ICredentialService, type ServiceCollection } from "@mode/services";
import { formatLogPrefix } from "@mode/shared";
import { readLanAccessClientRecords } from "./lanAccessRecords.js";
import { startLanDiscoveryResponder } from "./lanDiscoveryResponder.js";
import {
  buildPublicRelayUrl,
  resolveLanRelayIdentity,
  startLanRelayTunnel,
  type RelayTunnelStartOptions,
} from "./lanRelayTunnel.js";
import { createRemoteAgentEnroller } from "./lanRemoteAgentEnrollment.js";
import { closeListenableQuietly, waitForListenPort } from "./lanListenPort.js";
import type { ListenableServer } from "./lanListenPort.js";
import { listLanAddresses } from "./lanNetworkAddresses.js";

const log = (...args: unknown[]) => console.log(formatLogPrefix("lanAccess", process.pid), ...args);
const warn = (...args: unknown[]) =>
  console.warn(formatLogPrefix("lanAccess", process.pid), ...args);

/** 远程桌面打包层 kill-switch 的 env 键（docs/specs/lan-remote-desktop.md 接口变更 6）。 */
export const MODE_REMOTE_DESKTOP_KILL_SWITCH_ENV_KEY = "MODE_REMOTE_DESKTOP_KILL_SWITCH";

/**
 * 打包层 kill-switch（镜像 MODE_CUA_PRODUCT_HELPER 的「只能关不能开」语义）：
 * 置为 1/true/on → 压死远程桌面能力位；未设置或置为 0/false/off → 不干预，
 * 是否启用由被控开关（设置页总开关）决定，本开关永远不能把功能打开。
 */
export function isRemoteDesktopKillSwitchEngaged(
  env: Record<string, string | undefined> = process.env,
): boolean {
  const explicit = env[MODE_REMOTE_DESKTOP_KILL_SWITCH_ENV_KEY]?.trim().toLowerCase();
  return explicit === "1" || explicit === "true" || explicit === "on";
}

/** 远程桌面设备能力位口径（纯函数，跨平台可测）：win32 && kill-switch 未触发 && 被控开关开。 */
export function computeRemoteDesktopCapability(params: {
  platform: string;
  killSwitchEngaged: boolean;
  serverEnabled: boolean;
}): boolean {
  return params.platform === "win32" && !params.killSwitchEngaged && params.serverEnabled;
}

/** 局域网访问服务端：监听局域网、应答发现、校验逐设备令牌、受理配对码。 */
export interface LanAccessServerHandle {
  getState(): LanAccessState;
  /**
   * 用云端登录下发的 relay token 启动隧道（幂等）。LAN 监听启动时没有令牌也能先生成
   * tunnelId；登录成功后再调用本方法。
   */
  startRelayTunnel(token: string): void;
  createPairCode(): LanAccessState;
  removeClient(clientId: string): Promise<LanAccessState>;
  resetTokens(): Promise<LanAccessState>;
  /** 运行期这些记录的唯一内存写者就是本 handle：写授权必须经它（含 persist）。 */
  setClientRemoteDesktopAuthorized(clientId: string, authorized: boolean): Promise<void>;
  dispose(): Promise<void>;
}

export interface StartLanAccessServerParams {
  services: ServiceCollection;
  credentials: ICredentialService;
  /** 每次请求现取工作区列表：用户刚打开的工作区要能被对端选到。 */
  getWorkspaces: () => Promise<ServerRemoteWorkspaceInfo[]>;
  appVersion: string;
  machineName: string;
  /**
   * 内置公网中继覆写：缺省 baseUrl 用 DEFAULT_PUBLIC_RELAY_BASE_URL、token 读
   * MODE_RELAY_TOKEN。拿到 token 才启动隧道并下发 publicRelay；否则保持仅局域网。
   */
  relay?: { baseUrl?: string; token?: string };
  /** 测试注入点：默认 startRelayTunnelClient。 */
  startTunnel?: (options: RelayTunnelStartOptions) => { close(): void };
  /** 测试/隔离用监听端口覆写；生产缺省 `LAN_ACCESS_DEFAULT_PORT`。 */
  port?: number;
  /** Web/手机远控静态资源目录；缺省 API-only。 */
  webRoot?: string;
  /**
   * 远程桌面被控总开关读取点（设置页「远程桌面」分区，默认关）。
   * 握手即时求值（能力位不持久化）；缺省 = 关闭。T6 接入设置存储。
   */
  isRemoteDesktopServerEnabled?: () => boolean;
  /**
   * 远程桌面被控 hook 注入点（T3 接 main 桥：同意流 / 信令转发 / 媒体 WS 交接）。
   * 缺省实现：requestSession 一律拒绝（disabled）、onSessionSocket 与 onMediaSocket 直接关闭连接。
   */
  remoteDesktop?: {
    requestSession?: (
      clientId: string,
      quality?: RemoteDesktopQuality,
    ) => Promise<{ ok: true } | { error: RemoteDesktopErrorCode }>;
    /** `session.reissueMediaTicket`：媒体 WS 瞬断重连（P2-1）的票据重签（由 server 层注入）。 */
    onSessionSocket?: (
      clientId: string,
      socket: WebSocket,
      session?: { reissueMediaTicket?: () => { mediaTicket: string; expiresAt: number } },
    ) => void;
    onMediaSocket?: (clientId: string, socket: WebSocket) => void;
  };
}

/** 定长比较；长度不同直接失败，不做 pad（令牌是 base64url 定长串）。 */
function constantTimeEqual(left: string, right: string): boolean {
  const a = Buffer.from(left, "utf8");
  const b = Buffer.from(right, "utf8");
  if (a.length !== b.length || a.length === 0) {
    return false;
  }
  return timingSafeEqual(a, b);
}

function createPairCode(): LanAccessPairCode {
  return {
    code: randomBytes(3).toString("hex").toUpperCase(),
    expiresAt: Date.now() + LAN_ACCESS_PAIR_CODE_TTL_MS,
  };
}

export async function startLanAccessServer(
  params: StartLanAccessServerParams,
): Promise<LanAccessServerHandle> {
  const { credentials } = params;

  let serverId = (await credentials.load(LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY))?.trim() ?? "";
  if (!serverId) {
    serverId = randomUUID();
    await credentials.save(LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY, serverId);
  }

  // 内置公网中继：地址有默认值，令牌是部署 secret（不写进仓库，读 env/注入）。
  // 没有令牌时保持纯 LAN，不向主控下发不可达的公网端点。
  const relayIdentity = await resolveLanRelayIdentity({
    credentials,
    ...(params.relay ? { relay: params.relay } : {}),
    warn,
  });
  let publicRelay: LanPublicRelayInfo | undefined;

  let clients = await readLanAccessClientRecords(credentials);
  /** clientId → token 的内存缓存；真相仍在凭据服务里。 */
  const clientTokens = new Map<string, string>();
  for (const client of clients) {
    const token = await credentials.load(buildLanAccessClientTokenKey(client.id));
    if (token) {
      clientTokens.set(client.id, token);
    }
  }

  let pairCode: LanAccessPairCode | null = null;
  let disposed = false;
  /** 内置公网中继的出站隧道；未配置 token 时为 null（仅 LAN）。 */
  let tunnel: { close(): void } | null = null;
  const lastTouchAt = new Map<string, number>();

  const persistClients = async (): Promise<void> => {
    await credentials.save(LAN_ACCESS_CLIENTS_CREDENTIAL_KEY, JSON.stringify(clients));
  };

  /** 命中才记 lastUsedAt，且每分钟最多写一次，避免把凭据写入变成热点。 */
  const touchClient = (clientId: string): void => {
    const now = Date.now();
    if ((lastTouchAt.get(clientId) ?? 0) > now - 60_000) {
      return;
    }
    lastTouchAt.set(clientId, now);
    const target = clients.find((item) => item.id === clientId);
    if (!target) {
      return;
    }
    target.lastUsedAt = now;
    void persistClients().catch((error: unknown) => {
      warn("persist client lastUsedAt failed:", error instanceof Error ? error.message : error);
    });
  };

  const verifyAuthToken = (presented: string): boolean => {
    for (const [clientId, token] of clientTokens) {
      if (constantTimeEqual(token, presented)) {
        touchClient(clientId);
        return true;
      }
    }
    return false;
  };

  /** 与鉴权同一令牌校验路径，供 /ws/host 升级时把连接归属到客户端记录（录制最近打开目录用）。 */
  const resolveLanClientByToken = (presented: string): string | null => {
    for (const [clientId, token] of clientTokens) {
      if (constantTimeEqual(token, presented)) {
        return clientId;
      }
    }
    return null;
  };

  /**
   * 该配对设备最近在本机打开的工作区目录（spec: docs/specs/lan-paired-devices.md）。
   * 低频事件：命中即写 clients 记录并持久化；同路径重复上报直接忽略。
   */
  const recordClientWorkspace = (clientId: string, workspacePath: string): void => {
    const target = clients.find((item) => item.id === clientId);
    if (!target || target.lastWorkspacePath === workspacePath) {
      return;
    }
    target.lastWorkspacePath = workspacePath;
    void persistClients().catch((error: unknown) => {
      warn(
        "persist client lastWorkspacePath failed:",
        error instanceof Error ? error.message : error,
      );
    });
  };

  const consumePairCode = (code: string): boolean => {
    const current = pairCode;
    if (!current || current.expiresAt <= Date.now()) {
      pairCode = null;
      return false;
    }
    if (!constantTimeEqual(current.code, code.trim().toUpperCase())) {
      return false;
    }
    // 一次性：无论后续写入是否成功都先作废，避免同码被重放。
    pairCode = null;
    return true;
  };

  const redeem = async (request: LanAccessPairRequest): Promise<LanAccessPairResult | null> => {
    if (!consumePairCode(request.code)) {
      return null;
    }
    const record: LanAccessClientRecord = {
      id: randomUUID(),
      label: request.label?.trim() || "LAN client",
      createdAt: Date.now(),
      lastUsedAt: null,
    };
    const token = randomBytes(32).toString("base64url");
    clients = [...clients, record];
    clientTokens.set(record.id, token);
    await credentials.save(buildLanAccessClientTokenKey(record.id), token);
    await persistClients();
    log("paired new lan client:", record.label);
    return {
      token,
      serverId,
      name: params.machineName,
      ...(publicRelay ? { publicRelay } : {}),
    };
  };

  const enrollRemoteAgent = createRemoteAgentEnroller({
    services: params.services,
    consumePairCode,
  });

  const requestedPort = params.port ?? LAN_ACCESS_DEFAULT_PORT;
  const server = createHttpServer(params.services, requestedPort, {
    host: "0.0.0.0",
    authRequired: true,
    ...(params.webRoot ? { staticRoot: params.webRoot, spaFallback: true } : {}),
    verifyAuthToken,
    lanPairing: { redeem },
    publicRelay: () => publicRelay,
    remoteAgentEnrollment: { enroll: enrollRemoteAgent },
    resolveLanClientByToken,
    onLanClientWorkspace: recordClientWorkspace,
    serverId,
    name: params.machineName,
    workspaces: () => params.getWorkspaces(),
    // LAN 对端是完整 desktop（desktop-local authority）。能力位 = 平台 win/mac && 打包层
    // kill-switch 关（isModeCuaInternalFeatureEnabled 为「只能关不能开」语义）。
    // 插件启用态是 workspace 级动态值，不进设备位（docs/specs/lan-remote-computer-use.md）。
    computerUseCapability:
      (process.platform === "win32" || process.platform === "darwin") &&
      isModeCuaInternalFeatureEnabled(process.env),
    // 远程桌面能力位 = win32 && kill-switch 未触发 && 被控开关开（缺省关）。
    // 开关是可变设置，按每次握手现取（能力位本身不持久化，docs/specs/lan-remote-desktop.md 产品规则 5）。
    remoteDesktopCapability: () =>
      computeRemoteDesktopCapability({
        platform: process.platform,
        killSwitchEngaged: isRemoteDesktopKillSwitchEngaged(process.env),
        serverEnabled: params.isRemoteDesktopServerEnabled?.() ?? false,
      }),
    // 原生媒体面版本门（docs/specs/lan-remote-desktop-native-media.md 版本门 1）：
    // 本期纯直替——native ≡ remoteDesktop，故用同一表达式再算一遍（两键必然同值）；
    // 未来若要兼容旧版 WebRTC 对端，再在这里解耦成独立判定。
    remoteDesktopNativeCapability: () =>
      computeRemoteDesktopCapability({
        platform: process.platform,
        killSwitchEngaged: isRemoteDesktopKillSwitchEngaged(process.env),
        serverEnabled: params.isRemoteDesktopServerEnabled?.() ?? false,
      }),
    // T3 接 main 桥：缺省实现下会话请求一律拒绝、信令 socket 断链，不落地半开状态。
    remoteDesktop: {
      requestSession:
        params.remoteDesktop?.requestSession ??
        (async (): Promise<{ error: RemoteDesktopErrorCode }> => ({ error: "disabled" })),
      onSessionSocket:
        params.remoteDesktop?.onSessionSocket ??
        ((_clientId: string, socket: WebSocket) => {
          socket.close(4001, "remote desktop not wired");
        }),
      // 缺省关闭（与 onSessionSocket 同模式）：媒体票已被消费，但没有 relay 可挂，
      // 不留下无归属的媒体连接。
      onMediaSocket:
        params.remoteDesktop?.onMediaSocket ??
        ((_clientId: string, socket: WebSocket) => {
          socket.close(4001, "remote desktop media not wired");
        }),
    },
  });
  const listenable = server as unknown as ListenableServer;
  let port: number;
  try {
    port = await waitForListenPort(listenable, requestedPort, requestedPort);
  } catch (error) {
    // 监听失败：释放 server 上已挂的升级处理器，并把失败交给调用方按「LAN 不可用」处理。
    closeListenableQuietly(listenable);
    throw error;
  }
  // 监听成功后的运行期错误不能再变成未捕获事件（否则仍会杀 Host）；只记录并让请求失败。
  listenable.on("error", (error: Error) => {
    warn("lan access server error:", error.message);
  });

  const discovery = await startLanDiscoveryResponder({
    serverId,
    name: params.machineName,
    version: params.appVersion,
    port,
    isDisposed: () => disposed,
    warn,
  });

  // 监听已建立后再启动出站隧道：本地 127.0.0.1:<port> 必须可达。
  // 隧道连接失败不阻断 LAN：startRelayTunnelClient 自己退避重连，LAN 照常可用。
  const startRelayTunnel = (token: string): void => {
    if (!relayIdentity || tunnel) {
      return;
    }
    const started = startLanRelayTunnel({
      config: { publicRelay: relayIdentity, token: token.trim() },
      localPort: port,
      ...(params.startTunnel ? { startTunnel: params.startTunnel } : {}),
      log,
      warn,
    });
    if (started) {
      tunnel = started;
      publicRelay = relayIdentity;
    }
  };
  const initialRelayToken = (
    params.relay?.token ?? process.env.MODE_RELAY_TOKEN ?? ""
  ).trim();
  if (initialRelayToken) {
    startRelayTunnel(initialRelayToken);
  }

  const getState = (): LanAccessState => {
    if (pairCode && pairCode.expiresAt <= Date.now()) {
      pairCode = null;
    }
    return {
      enabled: true,
      port,
      addresses: listLanAddresses(port),
      pairCode: pairCode ? { ...pairCode } : null,
      clients: clients.map((client) => ({ ...client })),
      publicRelayUrl: publicRelay ? buildPublicRelayUrl(publicRelay) : null,
    };
  };

  return {
    getState,
    startRelayTunnel,
    createPairCode(): LanAccessState {
      pairCode = createPairCode();
      return getState();
    },
    async removeClient(clientId: string): Promise<LanAccessState> {
      const target = clients.find((client) => client.id === clientId);
      if (!target) {
        return getState();
      }
      clients = clients.filter((client) => client.id !== clientId);
      clientTokens.delete(clientId);
      lastTouchAt.delete(clientId);
      await credentials.delete(buildLanAccessClientTokenKey(clientId));
      await persistClients();
      return getState();
    },
    async resetTokens(): Promise<LanAccessState> {
      for (const client of clients) {
        await credentials.delete(buildLanAccessClientTokenKey(client.id));
      }
      clients = [];
      clientTokens.clear();
      lastTouchAt.clear();
      await persistClients();
      return getState();
    },
    async setClientRemoteDesktopAuthorized(clientId: string, authorized: boolean): Promise<void> {
      const target = clients.find((item) => item.id === clientId);
      if (!target) {
        return;
      }
      if (authorized) {
        target.remoteDesktopAuthorized = true;
      } else {
        delete target.remoteDesktopAuthorized;
      }
      await persistClients();
    },
    async dispose(): Promise<void> {
      if (disposed) {
        return;
      }
      disposed = true;
      pairCode = null;
      tunnel?.close();
      tunnel = null;
      discovery.close();
      await new Promise<void>((resolve) => {
        const closable = server as unknown as {
          close: (callback?: () => void) => void;
        };
        try {
          closable.close(() => resolve());
        } catch {
          resolve();
        }
      });
    },
  };
}
