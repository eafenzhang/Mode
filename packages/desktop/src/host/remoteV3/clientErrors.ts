import { RemoteV3ApiError } from "./api.js";

/**
 * 401/403/409 都意味着「这套 client 凭证已经不能再用」，需要重新注册而非继续刷新。
 * Control WS 的 4002（invalid device credential）同理，但它不是 HTTP 错误。
 */
export function isClientCredentialFailure(error: unknown): boolean {
  if (error instanceof RemoteV3ApiError) {
    return error.status === 401 || error.status === 403 || error.status === 409;
  }
  return error instanceof Error && /4002|invalid device credential/u.test(error.message);
}

/** 日志用的短原因：不把完整响应体写进日志。 */
export function describeFailure(error: unknown): string {
  if (error instanceof RemoteV3ApiError) return `http_${error.status}`;
  return error instanceof Error ? error.message.slice(0, 60) : "unknown";
}
