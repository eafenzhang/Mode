import { createSocket } from "node:dgram";
import { networkInterfaces } from "node:os";
import { randomBytes, randomUUID, timingSafeEqual } from "node:crypto";
import {
  LAN_ACCESS_CLIENTS_CREDENTIAL_KEY,
  LAN_ACCESS_DEFAULT_PORT,
  LAN_ACCESS_DISCOVERY_PORT,
  LAN_ACCESS_PAIR_CODE_TTL_MS,
  LAN_ACCESS_PROBE,
  LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY,
  buildLanAccessClientTokenKey,
  buildLanAnnouncementPayload,
  type LanAccessClientRecord,
  type LanAccessPairCode,
  type LanAccessPairRequest,
  type LanAccessPairResult,
  type LanAccessState,
  type ServerRemoteWorkspaceInfo,
} from "@mode/shared";
import { createHttpServer } from "@mode/server";
import type { ICredentialService, ServiceCollection } from "@mode/services";
import { formatLogPrefix } from "@mode/shared";

const log = (...args: unknown[]) => console.log(formatLogPrefix("lanAccess", process.pid), ...args);
const warn = (...args: unknown[]) => console.warn(formatLogPrefix("lanAccess", process.pid), ...args);

/** 局域网访问服务端：监听局域网、应答发现、校验逐设备令牌、受理配对码。 */
export interface LanAccessServerHandle {
  getState(): LanAccessState;
  createPairCode(): LanAccessState;
  removeClient(clientId: string): Promise<LanAccessState>;
  resetTokens(): Promise<LanAccessState>;
  dispose(): Promise<void>;
}

export interface StartLanAccessServerParams {
  services: ServiceCollection;
  credentials: ICredentialService;
  /** 每次请求现取工作区列表：用户刚打开的工作区要能被对端选到。 */
  getWorkspaces: () => Promise<ServerRemoteWorkspaceInfo[]>;
  appVersion: string;
  machineName: string;
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

function listLanAddresses(port: number): string[] {
  const addresses: string[] = [];
  for (const infos of Object.values(networkInterfaces())) {
    for (const info of infos ?? []) {
      if (info.family !== "IPv4" || info.internal) {
        continue;
      }
      addresses.push(`${info.address}:${port}`);
    }
  }
  return addresses;
}

/** srvx/node server 的端口要等 listen 完成；轮询 address() 直到可用。 */
async function waitForListenPort(server: unknown, fallback: number): Promise<number> {
  const address = (server as { address?: () => unknown }).address;
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (typeof address === "function") {
      const value = (server as { address: () => unknown }).address();
      if (value && typeof value === "object" && "port" in value) {
        const port = Number((value as { port: unknown }).port);
        if (Number.isFinite(port) && port > 0) {
          return port;
        }
      }
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  return fallback;
}

async function readClients(credentials: ICredentialService): Promise<LanAccessClientRecord[]> {
  const raw = await credentials.load(LAN_ACCESS_CLIENTS_CREDENTIAL_KEY);
  if (!raw) {
    return [];
  }
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) {
      return [];
    }
    return parsed
      .filter((item): item is LanAccessClientRecord => {
        if (!item || typeof item !== "object") {
          return false;
        }
        const record = item as Partial<LanAccessClientRecord>;
        return typeof record.id === "string" && record.id.length > 0;
      })
      .map((record) => ({
        id: record.id,
        label: typeof record.label === "string" ? record.label : "",
        createdAt: typeof record.createdAt === "number" ? record.createdAt : Date.now(),
        lastUsedAt: typeof record.lastUsedAt === "number" ? record.lastUsedAt : null,
        ...(typeof record.lastWorkspacePath === "string" && record.lastWorkspacePath
          ? { lastWorkspacePath: record.lastWorkspacePath }
          : {}),
      }));
  } catch {
    return [];
  }
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

  let clients = await readClients(credentials);
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
      warn("persist client lastWorkspacePath failed:", error instanceof Error ? error.message : error);
    });
  };

  const redeem = async (request: LanAccessPairRequest): Promise<LanAccessPairResult | null> => {
    const current = pairCode;
    if (!current || current.expiresAt <= Date.now()) {
      pairCode = null;
      return null;
    }
    if (!constantTimeEqual(current.code, request.code.trim().toUpperCase())) {
      return null;
    }
    // 一次性：无论后续写入是否成功都先作废，避免同码被重放。
    pairCode = null;
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
    return { token, serverId, name: params.machineName };
  };

  const server = createHttpServer(params.services, LAN_ACCESS_DEFAULT_PORT, {
    host: "0.0.0.0",
    authRequired: true,
    verifyAuthToken,
    lanPairing: { redeem },
    resolveLanClientByToken,
    onLanClientWorkspace: recordClientWorkspace,
    serverId,
    name: params.machineName,
    workspaces: () => params.getWorkspaces(),
  });
  const port = await waitForListenPort(server, LAN_ACCESS_DEFAULT_PORT);

  // 发现应答：只回应收到的探测，不做周期广播（少噪声，也少信息外泄）。
  const discoverySocket = createSocket({ type: "udp4", reuseAddr: true });
  const announcement = buildLanAnnouncementPayload({
    serverId,
    name: params.machineName,
    version: params.appVersion,
    port,
    requiresPairing: true,
    platform: process.platform,
  });
  discoverySocket.on("message", (message, rinfo) => {
    if (message.toString("utf8").trim() !== LAN_ACCESS_PROBE || disposed) {
      return;
    }
    discoverySocket.send(announcement, rinfo.port, rinfo.address, (error) => {
      if (error) {
        warn("lan discovery reply failed:", error.message);
      }
    });
  });
  discoverySocket.on("error", (error) => {
    warn("lan discovery socket error:", error.message);
  });
  await new Promise<void>((resolve) => {
    discoverySocket.bind(LAN_ACCESS_DISCOVERY_PORT, "0.0.0.0", () => resolve());
    // 同机多实例时发现端口可能已被占用：降级为「仅手动地址可达」，不影响已配对的监听。
    discoverySocket.once("error", () => resolve());
  });

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
    };
  };

  return {
    getState,
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
    async dispose(): Promise<void> {
      if (disposed) {
        return;
      }
      disposed = true;
      pairCode = null;
      try {
        discoverySocket.close();
      } catch {
        // 关闭未绑定的 socket 会抛错：忽略即可。
      }
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

/** 供测试与 UI 复用：局域网访问默认监听地址（不含端口）。 */
export const LAN_ACCESS_BIND_HOST = "0.0.0.0";
