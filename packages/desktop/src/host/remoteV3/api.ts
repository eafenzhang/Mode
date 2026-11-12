import { parseDeviceDescriptor, type DeviceDescriptor } from "@mode/remote-core";
import type {
  RemoteV3DeviceSummary,
  RemoteV3PairingStart,
  RemoteV3PairingState,
} from "@mode/shared";

import type { RemoteV3Tokens } from "./identity.js";

export interface RemoteV3Registration extends RemoteV3Tokens {
  accountId: string;
  device: DeviceDescriptor;
}

/** 账号服务返回的非 2xx；Host 的重连策略按 status 区分「凭证失效」与「网络抖动」。 */
export class RemoteV3ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "RemoteV3ApiError";
    this.status = status;
  }
}

export function isRemoteV3AuthError(error: unknown): boolean {
  return error instanceof RemoteV3ApiError && (error.status === 401 || error.status === 403);
}

export function normalizeRemoteServerUrl(raw: string): string {
  const url = new URL(raw.trim());
  const local =
    url.hostname === "localhost" || url.hostname === "127.0.0.1" || url.hostname === "::1";
  if (url.protocol !== "https:" && !(url.protocol === "http:" && local)) {
    throw new Error("远程控制 v3 服务必须使用 HTTPS（仅本机开发允许 HTTP）");
  }
  // 保留路径前缀（fnOS 网关/反向代理子路径）；只去掉尾部斜杠。
  url.pathname = url.pathname.replace(/\/+$/u, "");
  url.search = "";
  url.hash = "";
  return url.toString().replace(/\/$/, "");
}

export async function registerRemoteV3Host(input: {
  serverUrl: string;
  webSessionToken: string;
  descriptor: DeviceDescriptor;
  /** LAN `host:port` 列表（最多 8 个），为 P2 直连预留；不含令牌。 */
  addresses?: string[];
  fetchImpl?: typeof fetch;
}): Promise<RemoteV3Registration> {
  const fetchImpl = input.fetchImpl ?? fetch;
  const response = await fetchImpl(`${input.serverUrl}/api/v1/devices`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${input.webSessionToken}`,
    },
    body: JSON.stringify({
      device: input.descriptor,
      ...(input.addresses?.length ? { addresses: input.addresses } : {}),
    }),
  });
  const body = await readJson(response);
  if (!response.ok) {
    throw new RemoteV3ApiError(response.status, readError(body, "设备注册失败"));
  }
  if (
    typeof body.accessToken !== "string" ||
    typeof body.refreshToken !== "string" ||
    typeof body.accountId !== "string"
  ) {
    throw new Error("设备注册响应缺少 token");
  }
  return {
    accountId: body.accountId,
    device: parseDeviceDescriptor(body.device, "device"),
    accessToken: body.accessToken,
    accessExpiresAt: typeof body.accessExpiresAt === "number" ? body.accessExpiresAt : 0,
    refreshToken: body.refreshToken,
    refreshExpiresAt: typeof body.refreshExpiresAt === "number" ? body.refreshExpiresAt : 0,
  };
}

export async function registerRemoteV3OwnedRole(input: {
  serverUrl: string;
  accessToken: string;
  descriptor: DeviceDescriptor;
  fetchImpl?: typeof fetch;
}): Promise<RemoteV3Registration> {
  const fetchImpl = input.fetchImpl ?? fetch;
  const response = await fetchImpl(`${input.serverUrl}/api/v1/devices/register-owned-role`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${input.accessToken}`,
    },
    body: JSON.stringify({ device: input.descriptor }),
  });
  const body = await readJson(response);
  if (!response.ok)
    throw new RemoteV3ApiError(response.status, readError(body, "注册相反角色失败"));
  if (
    typeof body.accessToken !== "string" ||
    typeof body.refreshToken !== "string" ||
    typeof body.accountId !== "string"
  ) {
    throw new Error("相反角色注册响应缺少 token");
  }
  return {
    accountId: body.accountId,
    device: parseDeviceDescriptor(body.device, "device"),
    accessToken: body.accessToken,
    accessExpiresAt: typeof body.accessExpiresAt === "number" ? body.accessExpiresAt : 0,
    refreshToken: body.refreshToken,
    refreshExpiresAt: typeof body.refreshExpiresAt === "number" ? body.refreshExpiresAt : 0,
  };
}

export async function refreshRemoteV3Tokens(input: {
  serverUrl: string;
  deviceId: string;
  refreshToken: string;
  fetchImpl?: typeof fetch;
}): Promise<RemoteV3Tokens> {
  const fetchImpl = input.fetchImpl ?? fetch;
  const response = await fetchImpl(`${input.serverUrl}/api/v1/devices/refresh`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ deviceId: input.deviceId, refreshToken: input.refreshToken }),
  });
  const body = await readJson(response);
  if (!response.ok) {
    throw new RemoteV3ApiError(response.status, readError(body, "设备 token 刷新失败"));
  }
  if (typeof body.accessToken !== "string" || typeof body.refreshToken !== "string") {
    throw new Error("设备 token 刷新响应缺少 token");
  }
  return {
    accessToken: body.accessToken,
    accessExpiresAt: typeof body.accessExpiresAt === "number" ? body.accessExpiresAt : 0,
    refreshToken: body.refreshToken,
    refreshExpiresAt: typeof body.refreshExpiresAt === "number" ? body.refreshExpiresAt : 0,
  };
}

export async function fetchRemoteV3Peer(input: {
  serverUrl: string;
  accessToken: string;
  deviceId: string;
  fetchImpl?: typeof fetch;
}): Promise<DeviceDescriptor> {
  const fetchImpl = input.fetchImpl ?? fetch;
  const response = await fetchImpl(
    `${input.serverUrl}/api/v1/devices/${encodeURIComponent(input.deviceId)}`,
    { headers: { authorization: `Bearer ${input.accessToken}` } },
  );
  const body = await readJson(response);
  if (!response.ok)
    throw new RemoteV3ApiError(response.status, readError(body, "读取对端设备失败"));
  return parseDeviceDescriptor(body.device, "peerDevice");
}

export async function fetchRemoteV3Account(input: {
  serverUrl: string;
  webSessionToken: string;
  fetchImpl?: typeof fetch;
}): Promise<{ id: string; displayName: string | null }> {
  const fetchImpl = input.fetchImpl ?? fetch;
  const response = await fetchImpl(`${input.serverUrl}/api/v1/auth/me`, {
    headers: { authorization: `Bearer ${input.webSessionToken}` },
  });
  const body = await readJson(response);
  if (!response.ok) {
    throw new RemoteV3ApiError(response.status, readError(body, "读取账号信息失败"));
  }
  const account = body.account as { id?: unknown; displayName?: unknown } | undefined;
  if (body.kind !== "web" || typeof account?.id !== "string" || !account.id) {
    throw new Error("账号信息响应缺少 account.id");
  }
  return {
    id: account.id,
    displayName:
      typeof account.displayName === "string" && account.displayName.trim()
        ? account.displayName.trim()
        : null,
  };
}

export async function listRemoteV3Devices(input: {
  serverUrl: string;
  accessToken: string;
  fetchImpl?: typeof fetch;
}): Promise<RemoteV3DeviceSummary[]> {
  const fetchImpl = input.fetchImpl ?? fetch;
  const response = await fetchImpl(`${input.serverUrl}/api/v1/devices`, {
    headers: { authorization: `Bearer ${input.accessToken}` },
  });
  const body = await readJson(response);
  if (!response.ok)
    throw new RemoteV3ApiError(response.status, readError(body, "读取设备目录失败"));
  const items = Array.isArray(body.items) ? body.items : [];
  return items.flatMap((entry) => {
    if (typeof entry !== "object" || entry === null) return [];
    const item = entry as { device?: unknown; online?: unknown; addresses?: unknown };
    try {
      const device = parseDeviceDescriptor(item.device, "device");
      return [
        {
          deviceId: device.deviceId,
          name: device.name,
          role: device.role,
          platform: device.platform,
          identityKey: device.identityKey,
          online: item.online === true,
          addresses: Array.isArray(item.addresses)
            ? item.addresses.filter((value): value is string => typeof value === "string")
            : [],
        },
      ];
    } catch {
      return [];
    }
  });
}

export interface GithubDeviceFlowStart {
  deviceCode: string;
  userCode: string;
  verificationUri: string;
  expiresIn: number;
  interval: number;
}

export interface GithubDeviceFlowPoll {
  status: "pending" | "slow_down" | "expired" | "denied" | "ok";
  sessionToken?: string;
  interval?: number;
}

/**
 * 桌面端也走服务端 GitHub Device Flow 代理：client id 只在服务端配置，
 * 客户端不需要（也不再依赖 MODE_REMOTE_GITHUB_CLIENT_ID 环境变量）。
 */
export async function startGithubDeviceFlow(input: {
  serverUrl: string;
  fetchImpl?: typeof fetch;
}): Promise<GithubDeviceFlowStart> {
  const fetchImpl = input.fetchImpl ?? fetch;
  const response = await fetchImpl(`${input.serverUrl}/api/v1/auth/github/device/start`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: "{}",
  });
  const body = await readJson(response);
  if (!response.ok || typeof body.deviceCode !== "string") {
    throw new Error(readError(body, "GitHub Device Flow 启动失败"));
  }
  return {
    deviceCode: body.deviceCode,
    userCode: typeof body.userCode === "string" ? body.userCode : "",
    verificationUri:
      typeof body.verificationUri === "string"
        ? body.verificationUri
        : "https://github.com/login/device",
    expiresIn: typeof body.expiresIn === "number" ? body.expiresIn : 900,
    interval: typeof body.interval === "number" ? body.interval : 5,
  };
}

export async function pollGithubDeviceFlow(input: {
  serverUrl: string;
  deviceCode: string;
  fetchImpl?: typeof fetch;
}): Promise<GithubDeviceFlowPoll> {
  const fetchImpl = input.fetchImpl ?? fetch;
  const response = await fetchImpl(`${input.serverUrl}/api/v1/auth/github/device/poll`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ deviceCode: input.deviceCode }),
  });
  const body = await readJson(response);
  const status = body.status;
  if (status === "ok" && typeof body.sessionToken === "string") {
    return { status: "ok", sessionToken: body.sessionToken };
  }
  if (status === "pending") return { status: "pending" };
  if (status === "slow_down") {
    return {
      status: "slow_down",
      ...(typeof body.interval === "number" ? { interval: body.interval } : {}),
    };
  }
  if (status === "expired") return { status: "expired" };
  if (status === "denied") return { status: "denied" };
  throw new Error(readError(body, "GitHub Device Flow 轮询失败"));
}

export async function readJson(response: Response): Promise<Record<string, unknown>> {
  try {
    const parsed = (await response.json()) as unknown;
    return typeof parsed === "object" && parsed !== null ? (parsed as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

export function readError(body: Record<string, unknown>, fallback: string): string {
  const error = body.error;
  if (typeof error === "object" && error !== null) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string" && message) return message;
  }
  return fallback;
}
