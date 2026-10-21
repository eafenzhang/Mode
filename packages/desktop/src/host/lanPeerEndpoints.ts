import {
  resolveLanTargetEndpoints,
  type LanConnectOptions,
  type LanPeerEndpoint,
  type ServerRemoteInfo,
} from "@mode/shared";

import { fetchLanPeerInfo } from "./lanRemoteAttach.js";
import { isRetryableLanPeerError } from "./lanPeerTransport.js";

/**
 * 已配对对端的端点选择（远程工作区 + 远程桌面共用）。
 *
 * 顺序固定：公网 → LAN。只有传输失败/超时/5xx/404/协议不符才换下一个端点；
 * 401/403 等对端级错误立即收口（同一令牌在另一个端点不会更好）。
 */

/** 多端点时公网探测预算：覆盖 TLS 握手与反向隧道冷启动。 */
const PUBLIC_ENDPOINT_PROBE_TIMEOUT_MS = 8_000;
/** 多端点时 LAN 回退预算：公网不可达时不长时间卡在局域网地址上。 */
const LAN_ENDPOINT_PROBE_TIMEOUT_MS = 2_500;

export type LanPeerEndpointSelection =
  | { ok: true; endpoint: LanPeerEndpoint; info: ServerRemoteInfo }
  | { ok: false; error: unknown };

export function endpointProbeTimeoutMs(
  endpoint: LanPeerEndpoint,
  endpointCount: number,
): number | undefined {
  if (endpointCount <= 1) {
    // 单端点沿用既有 10s 预算（fetchLanPeerInfo 缺省值）。
    return undefined;
  }
  return endpoint.kind === "lan"
    ? LAN_ENDPOINT_PROBE_TIMEOUT_MS
    : PUBLIC_ENDPOINT_PROBE_TIMEOUT_MS;
}

export function endpointRequest(endpoint: LanPeerEndpoint): {
  scheme: "http" | "https";
  host: string;
  port: number;
  path?: string;
} {
  return {
    scheme: endpoint.scheme,
    host: endpoint.host,
    port: endpoint.port,
    ...(endpoint.path ? { path: endpoint.path } : {}),
  };
}

async function probeLanPeerEndpoint(params: {
  endpoint: LanPeerEndpoint;
  endpointCount: number;
  token: string;
}): Promise<ServerRemoteInfo> {
  const timeoutMs = endpointProbeTimeoutMs(params.endpoint, params.endpointCount);
  return fetchLanPeerInfo({
    ...endpointRequest(params.endpoint),
    token: params.token,
    ...(timeoutMs !== undefined ? { timeoutMs } : {}),
  });
}

export async function selectReachableLanPeerEndpoint(params: {
  endpoints: LanPeerEndpoint[];
  token: string;
}): Promise<LanPeerEndpointSelection> {
  let lastError: unknown = new Error("对端不可达");
  for (const endpoint of params.endpoints) {
    try {
      const info = await probeLanPeerEndpoint({
        endpoint,
        endpointCount: params.endpoints.length,
        token: params.token,
      });
      return { ok: true, endpoint, info };
    } catch (error) {
      lastError = error;
      if (!isRetryableLanPeerError(error)) {
        return { ok: false, error };
      }
    }
  }
  return { ok: false, error: lastError };
}

/** LAN 目标的端点列表：显式 endpoints 优先，否则用主端点构造单个。 */
export function endpointsForLanTarget(target: LanConnectOptions): LanPeerEndpoint[] {
  return resolveLanTargetEndpoints(target);
}
