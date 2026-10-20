/**
 * LAN/公网对端端点传输基元：URL 构造、请求超时与可重试错误分类。
 *
 * 配对/挂接（lanRemoteAttach）与探测/会话（remoteDesktopPeerClient）共用同一套语义；
 * 单独成文件，避免 lanRemoteAttach 因工具函数膨胀超过行数上限。
 */

/** WebSocket 协议由 HTTP 端点协议推导：https → wss（公网必须走 TLS）。 */
export function toWebSocketScheme(scheme: "http" | "https"): "ws" | "wss" {
  return scheme === "https" ? "wss" : "ws";
}

export function buildLanHttpBase(
  host: string,
  port: number,
  scheme: "http" | "https" = "http",
): string {
  return `${scheme}://${host}:${port}`;
}

export function buildLanWsUrl(
  host: string,
  port: number,
  scheme: "http" | "https" = "http",
): string {
  return `${toWebSocketScheme(scheme)}://${host}:${port}/ws/host`;
}

/**
 * 端点请求错误：`retryable` 决定是否切换下一个端点。只有传输失败/超时/5xx/404/
 * 协议不符可重试；401/403 等对端级结果不重试（同一令牌在另一个端点不会更好）。
 */
export class LanPeerRequestError extends Error {
  constructor(
    message: string,
    readonly retryable: boolean,
    readonly code?: "unauthorized" | "internal",
  ) {
    super(message);
  }
}

export function isRetryableLanPeerError(error: unknown): boolean {
  return error instanceof LanPeerRequestError && error.retryable;
}

export async function fetchWithTimeout(
  url: string,
  init: RequestInit,
  timeoutMs: number,
): Promise<Response> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    return await fetch(url, { ...init, signal: controller.signal });
  } finally {
    clearTimeout(timer);
  }
}

export async function fetchPeerResponse(
  url: string,
  init: RequestInit,
  timeoutMs: number,
): Promise<Response> {
  try {
    return await fetchWithTimeout(url, init, timeoutMs);
  } catch (error) {
    const timedOut = error instanceof Error && error.name === "AbortError";
    throw new LanPeerRequestError(
      timedOut
        ? `连接对端超时（${timeoutMs}ms）`
        : `连接对端失败：${error instanceof Error ? error.message : String(error)}`,
      true,
    );
  }
}
