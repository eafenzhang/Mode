/**
 * 账号服务 REST 的统一出口：单次超时、幂等 GET 自动重试一次，以及响应体解析。
 *
 * 为什么单独成文件：`api.ts` 已经逼近 max-lines 上限，而超时/重试/解析是横切关注点，
 * 和具体端点无关（docs/specs/remote-control-v3.md §5.4）。
 *
 * 只重试 GET 的理由：refresh token 是单次消费的，POST 超时后重发同一 token 会被服务端
 * 判定为重放并撤销整个 token family；设备注册同理会重复签发凭证。GET 是只读的，
 * 一次网络抖动不应把整条 v3 连接判死（Host 的 `ensurePeer` 交叉校验也走这里）。
 */

/** 设备目录 / `auth/me` 这类读接口的单次超时。 */
const GET_TIMEOUT_MS = 8_000;
/** 对端 descriptor 交叉校验发生在建连关键路径上，用更短预算避免拖满 connect 预算。 */
export const PEER_GET_TIMEOUT_MS = 5_000;
/** 注册/刷新等签发凭证的 POST。 */
export const MUTATION_TIMEOUT_MS = 15_000;
/** GitHub OAuth / Device Flow 的启动与轮询。 */
export const POLL_TIMEOUT_MS = 10_000;
const GET_RETRY_DELAY_MS = 250;

interface FetchPolicy {
  timeoutMs?: number;
  /** 只有幂等 GET 才传 1；POST 一律 0（refresh token 单次消费，重发即重放）。 */
  retries?: number;
}

/** 网络层失败才可重试；HTTP 4xx/5xx 由调用方按 RemoteV3ApiError 处理，不在此重试。 */
function isRetryableFetchError(error: unknown): boolean {
  if (!(error instanceof Error)) return false;
  // AbortSignal.timeout → TimeoutError；调用方主动取消是 AbortError，不重试。
  if (error.name === "TimeoutError") return true;
  if (error.name === "AbortError") return false;
  if (error.message === "fetch failed") return true;
  const code = (error.cause as { code?: unknown } | undefined)?.code;
  return (
    code === "UND_ERR_CONNECT_TIMEOUT" ||
    code === "ECONNRESET" ||
    code === "ETIMEDOUT" ||
    code === "ENOTFOUND" ||
    code === "EAI_AGAIN"
  );
}

/**
 * 账号服务请求：每次调用都挂超时 signal；`retries > 0` 时对网络/超时错误退避重试。
 *
 * 账号服务不可达时（真机复现为 ~30% 的 HTTPS 连接超时）没有超时会让工作区向导
 * 无限停在「正在连接」；没有 GET 重试则一次抖动就把连接判死。
 */
export async function requestFetch(
  fetchImpl: typeof fetch,
  url: string,
  init: RequestInit,
  policy: FetchPolicy = {},
): Promise<Response> {
  const timeoutMs = policy.timeoutMs ?? GET_TIMEOUT_MS;
  const attempts = 1 + (policy.retries ?? 0);
  let lastError: unknown;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      return await fetchImpl(url, { ...init, signal: AbortSignal.timeout(timeoutMs) });
    } catch (error) {
      lastError = error;
      if (attempt + 1 >= attempts || !isRetryableFetchError(error)) {
        throw error;
      }
      await new Promise((resolve) => {
        setTimeout(resolve, GET_RETRY_DELAY_MS * (attempt + 1));
      });
    }
  }
  throw lastError;
}

/** 非 2xx 也常带 JSON 错误体；解析失败按空对象处理，由调用方给兜底文案。 */
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
