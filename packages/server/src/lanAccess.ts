// 服务端形态的局域网被连端（docs/specs/lan-host-server-mode.md）：
// 「配对码 → 逐设备长期令牌」与开关键的状态所有者；HTTP 端点在 http.ts 注册，
// web 平台只做 fetch。语义与桌面端 lanAccessServer 对齐：同 shared 契约、同凭据键、
// 同失败口径（关闭 = 不对外服务；码错/过期/被重置统一 401，不给探测信号）。
// 跨包约束：桌面端实现（packages/desktop/src/host/lanAccessServer.ts）不可被服务端引用，
// 本文件按同一 shared 契约复刻记录解析与令牌语义。

import { randomBytes, randomUUID, timingSafeEqual } from "node:crypto";
import { networkInterfaces } from "node:os";
import {
  LAN_ACCESS_CLIENTS_CREDENTIAL_KEY,
  LAN_ACCESS_PAIR_CODE_TTL_MS,
  LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY,
  buildLanAccessClientTokenKey,
  formatLogPrefix,
  type LanAccessClientRecord,
  type LanAccessPairCode,
  type LanAccessPairRequest,
  type LanAccessPairResult,
  type LanAccessState,
} from "@mode/shared";
import type { ICredentialService } from "@mode/services";

const log = (...args: unknown[]) => console.log(formatLogPrefix("lanAccess", process.pid), ...args);
const warn = (...args: unknown[]) => console.warn(formatLogPrefix("lanAccess", process.pid), ...args);

/** 开关持久化键：服务端本地状态，客户端不读取（不进 shared 契约）。 */
export const LAN_ACCESS_ENABLED_CREDENTIAL_KEY = "lan:access:enabled";

export interface LanAccessHostOptions {
  credentials: ICredentialService;
  /** 对外可达端口；listen 完成前可用函数形式惰性求值（getState 现取）。 */
  port: number | (() => number);
  /** 设备展示名（配对响应与设置页状态；应与 server-info 的 name 一致）。 */
  name: string;
}

/** http.ts 端点层消费的动作面；令牌校验/配对/录制由同一 handle 提供。 */
export interface LanAccessHostHandle {
  getServerId(): string;
  getState(): LanAccessState;
  setEnabled(enabled: boolean): Promise<LanAccessState>;
  createPairCode(): LanAccessState;
  removeClient(clientId: string): Promise<LanAccessState>;
  resetTokens(): Promise<LanAccessState>;
  verifyToken(token: string): boolean;
  resolveClientId(token: string): string | null;
  redeem(request: LanAccessPairRequest): Promise<LanAccessPairResult | null>;
  recordClientWorkspace(clientId: string, workspacePath: string): void;
}

/** 定长比较；长度不同直接失败（令牌是 base64url 定长串）。 */
function constantTimeEqual(left: string, right: string): boolean {
  const a = Buffer.from(left, "utf8");
  const b = Buffer.from(right, "utf8");
  if (a.length !== b.length || a.length === 0) {
    return false;
  }
  return timingSafeEqual(a, b);
}

function generatePairCode(): LanAccessPairCode {
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

/**
 * 读 lan client 记录（凭据服务 JSON 数组）。字段缺省归一与桌面端同名导出同口径；
 * 契约唯一真相 = shared 的 `LanAccessClientRecord` 字段集。
 */
async function readClientRecords(
  credentials: ICredentialService,
): Promise<LanAccessClientRecord[]> {
  const raw = await credentials.load(LAN_ACCESS_CLIENTS_CREDENTIAL_KEY);
  if (!raw) {
    return [];
  }
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) {
      return [];
    }
    const records: LanAccessClientRecord[] = [];
    for (const item of parsed) {
      if (!item || typeof item !== "object") {
        continue;
      }
      const record = item as Record<string, unknown>;
      if (typeof record["id"] !== "string" || !record["id"]) {
        continue;
      }
      records.push({
        id: record["id"],
        label: typeof record["label"] === "string" ? record["label"] : "",
        createdAt: typeof record["createdAt"] === "number" ? record["createdAt"] : Date.now(),
        lastUsedAt: typeof record["lastUsedAt"] === "number" ? record["lastUsedAt"] : null,
        ...(typeof record["lastWorkspacePath"] === "string" && record["lastWorkspacePath"]
          ? { lastWorkspacePath: record["lastWorkspacePath"] }
          : {}),
      });
    }
    return records;
  } catch {
    return [];
  }
}

export async function createLanAccessHost(
  options: LanAccessHostOptions,
): Promise<LanAccessHostHandle> {
  const { credentials, name } = options;
  const resolvePort = (): number =>
    typeof options.port === "function" ? options.port() : options.port;

  let serverId = (await credentials.load(LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY))?.trim() ?? "";
  if (!serverId) {
    serverId = randomUUID();
    await credentials.save(LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY, serverId);
  }

  let enabled = (await credentials.load(LAN_ACCESS_ENABLED_CREDENTIAL_KEY))?.trim() === "true";
  let clients = await readClientRecords(credentials);
  /** clientId → token 的内存缓存；真相仍在凭据服务里。 */
  const clientTokens = new Map<string, string>();
  for (const client of clients) {
    const token = await credentials.load(buildLanAccessClientTokenKey(client.id));
    if (token) {
      clientTokens.set(client.id, token);
    }
  }

  let pairCode: LanAccessPairCode | null = null;
  const lastTouchAt = new Map<string, number>();

  const persistClients = async (): Promise<void> => {
    await credentials.save(LAN_ACCESS_CLIENTS_CREDENTIAL_KEY, JSON.stringify(clients));
  };

  /** 命中才记 lastUsedAt，每分钟至多落盘一次，避免凭据写入变热点（与桌面端同口径）。 */
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
      warn("persist lastUsedAt failed:", error instanceof Error ? error.message : error);
    });
  };

  const resolveClientId = (presented: string): string | null => {
    if (!enabled || !presented) {
      return null;
    }
    for (const [clientId, token] of clientTokens) {
      if (constantTimeEqual(token, presented)) {
        touchClient(clientId);
        return clientId;
      }
    }
    return null;
  };

  const verifyToken = (presented: string): boolean => resolveClientId(presented) !== null;

  const recordClientWorkspace = (clientId: string, workspacePath: string): void => {
    const target = clients.find((item) => item.id === clientId);
    if (!target || target.lastWorkspacePath === workspacePath) {
      return;
    }
    target.lastWorkspacePath = workspacePath;
    void persistClients().catch((error: unknown) => {
      warn("persist lastWorkspacePath failed:", error instanceof Error ? error.message : error);
    });
  };

  const redeem = async (request: LanAccessPairRequest): Promise<LanAccessPairResult | null> => {
    if (!enabled) {
      return null;
    }
    const current = pairCode;
    if (!current || current.expiresAt <= Date.now()) {
      pairCode = null;
      return null;
    }
    if (!constantTimeEqual(current.code, request.code.trim().toUpperCase())) {
      return null;
    }
    // 一次性：无论后续写入是否成功都先作废，避免同码被重放（镜像桌面端实现）。
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
    return { token, serverId, ...(name ? { name } : {}) };
  };

  const getState = (): LanAccessState => {
    if (pairCode && pairCode.expiresAt <= Date.now()) {
      pairCode = null;
    }
    const port = resolvePort();
    const active = enabled && port > 0;
    return {
      enabled,
      port: active ? port : null,
      addresses: active ? listLanAddresses(port) : [],
      pairCode: active && pairCode ? { ...pairCode } : null,
      clients: clients.map((client) => ({ ...client })),
    };
  };

  return {
    getServerId: () => serverId,
    getState,
    async setEnabled(next: boolean): Promise<LanAccessState> {
      enabled = next;
      if (!enabled) {
        pairCode = null;
      }
      await credentials.save(LAN_ACCESS_ENABLED_CREDENTIAL_KEY, enabled ? "true" : "false");
      return getState();
    },
    createPairCode(): LanAccessState {
      if (enabled) {
        pairCode = generatePairCode();
      }
      return getState();
    },
    async removeClient(clientId: string): Promise<LanAccessState> {
      if (!clients.some((item) => item.id === clientId)) {
        return getState();
      }
      clients = clients.filter((item) => item.id !== clientId);
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
    verifyToken,
    resolveClientId,
    redeem,
    recordClientWorkspace,
  };
}
