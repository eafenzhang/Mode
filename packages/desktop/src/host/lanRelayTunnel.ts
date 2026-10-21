/**
 * 内置公网中继（docs/specs/lan-remote-public-endpoints.md）：
 * 解析 relay 配置、持久化 tunnelId、启动出站隧道。
 *
 * 从 lanAccessServer 抽出的原因：LAN 监听已接近行数上限，且中继生命周期与
 * 监听生命周期需要显式配对（启停同源），单独成文件便于测试与复用。
 */
import { randomBytes } from "node:crypto";

import {
  DEFAULT_PUBLIC_RELAY_BASE_URL,
  LAN_ACCESS_RELAY_TUNNEL_ID_CREDENTIAL_KEY,
  type LanPublicRelayInfo,
} from "@mode/shared";
import {
  DEFAULT_RELAY_TUNNEL_PATH,
  startRelayTunnelClient,
} from "@mode/cloud-relay/tunnel";
import type { ICredentialService } from "@mode/services";

export interface RelayTunnelStartOptions {
  relayUrl: string;
  token: string;
  tunnelId: string;
  localBaseUrl: string;
  log: (line: string) => void;
}

export interface ResolvedLanRelay {
  publicRelay: LanPublicRelayInfo;
  token: string;
}

/**
 * 解析中继身份（baseUrl + 持久化 tunnelId），**不要求令牌**：
 * 云端 Bot 登录时先有身份，登录成功拿到 relay token 后再启动隧道。
 * 非法地址/非 HTTPS 返回 null（保持 LAN-only）。
 */
export async function resolveLanRelayIdentity(params: {
  credentials: ICredentialService;
  relay?: { baseUrl?: string; token?: string };
  warn: (...args: unknown[]) => void;
}): Promise<LanPublicRelayInfo | null> {
  const baseUrl = (
    params.relay?.baseUrl ??
    process.env.MODE_PUBLIC_RELAY_BASE_URL ??
    DEFAULT_PUBLIC_RELAY_BASE_URL
  ).trim();
  try {
    if (new URL(baseUrl).protocol !== "https:") {
      params.warn("relay base url must be https:", baseUrl);
      return null;
    }
  } catch {
    params.warn("invalid relay base url:", baseUrl);
    return null;
  }
  let tunnelId =
    (await params.credentials.load(LAN_ACCESS_RELAY_TUNNEL_ID_CREDENTIAL_KEY))?.trim() ?? "";
  if (!tunnelId) {
    // 与 relay 服务端 16-128 字符的 tunnelId 约束对齐；持久化保证主控地址稳定。
    tunnelId = randomBytes(24).toString("base64url");
    await params.credentials.save(LAN_ACCESS_RELAY_TUNNEL_ID_CREDENTIAL_KEY, tunnelId);
  }
  return { baseUrl, tunnelId };
}

/** 兼容旧调用：有身份且有令牌才返回可启动配置。 */
export async function resolveLanRelayConfig(params: {
  credentials: ICredentialService;
  relay?: { baseUrl?: string; token?: string };
  warn: (...args: unknown[]) => void;
}): Promise<ResolvedLanRelay | null> {
  const publicRelay = await resolveLanRelayIdentity(params);
  const token = (params.relay?.token ?? process.env.MODE_RELAY_TOKEN ?? "").trim();
  return publicRelay && token ? { publicRelay, token } : null;
}

/** 主控/设置页展示的公开地址：https://host/d/<tunnelId>。 */
export function buildPublicRelayUrl(relay: LanPublicRelayInfo): string {
  return `${relay.baseUrl.replace(/\/+$/u, "")}/d/${relay.tunnelId}`;
}

/** 内置公网中继的隧道入口：https://host[/base] → wss://host[/base]/_tunnel。 */
function buildRelayTunnelUrl(baseUrl: string): string {
  const url = new URL(baseUrl);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  url.pathname = `${url.pathname.replace(/\/+$/u, "")}${DEFAULT_RELAY_TUNNEL_PATH}`;
  url.search = "";
  url.hash = "";
  return url.toString();
}

/** 启动出站隧道；失败只告警并返回 null，LAN 监听保持可用。 */
export function startLanRelayTunnel(params: {
  config: ResolvedLanRelay;
  localPort: number;
  startTunnel?: (options: RelayTunnelStartOptions) => { close(): void };
  log: (...args: unknown[]) => void;
  warn: (...args: unknown[]) => void;
}): { close(): void } | null {
  try {
    const startTunnel = params.startTunnel ?? startRelayTunnelClient;
    const tunnel = startTunnel({
      relayUrl: buildRelayTunnelUrl(params.config.publicRelay.baseUrl),
      token: params.config.token,
      tunnelId: params.config.publicRelay.tunnelId,
      localBaseUrl: `http://127.0.0.1:${params.localPort}`,
      log: (line) => params.log(line),
    });
    params.log(
      `lan_relay_tunnel_started tunnel=${params.config.publicRelay.tunnelId.slice(0, 8)}…`,
    );
    return tunnel;
  } catch (error) {
    params.warn(
      "lan relay tunnel start failed:",
      error instanceof Error ? error.message : String(error),
    );
    return null;
  }
}
