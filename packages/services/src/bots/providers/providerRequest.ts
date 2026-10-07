const BOT_PROVIDER_REQUEST_TIMEOUT_MS = 15_000;

export interface BotProviderJsonResponse<T> {
  ok: boolean;
  status: number;
  payload: T | undefined;
  responseLogId?: string;
}

export interface BotProviderResponse {
  ok: boolean;
  status: number;
}

async function runBotProviderRequest<T>(
  input: string | URL | Request,
  init: RequestInit,
  timeoutMs: number,
  consume: (response: Response) => Promise<T>,
): Promise<T> {
  const controller = new AbortController();
  const externalSignal = init.signal;
  const onAbort = (): void => controller.abort(externalSignal?.reason);
  if (externalSignal?.aborted) {
    onAbort();
  } else {
    externalSignal?.addEventListener("abort", onAbort, { once: true });
  }
  const timeout = setTimeout(() => {
    controller.abort(new Error(`Bot provider request timed out after ${timeoutMs}ms.`));
  }, timeoutMs);
  try {
    const response = await fetch(input, { ...init, signal: controller.signal });
    // 修复原因：收到响应头不代表请求完成。必须在同一个 AbortSignal 和 deadline 下
    // 消费响应体，否则服务端 headers 后停滞仍会永久堵住 Bot actor 队列。
    return await consume(response);
  } catch (error) {
    // 纵深防御：出站 URL 里可能带 token（如 Telegram 的 /bot<id>:<token>/），
    // 任何被抛出的错误在离开本层前先脱敏，避免凭证随错误日志扩散。
    throw sanitizeBotProviderError(error);
  } finally {
    clearTimeout(timeout);
    externalSignal?.removeEventListener("abort", onAbort);
  }
}

/**
 * 从错误消息中移除凭证：
 * - Telegram bot token（`bot<数字>:<token>`）
 * - query 参数中的 token / secret 类字段（access_token、corpsecret、aeskey 等）
 */
export function sanitizeBotProviderMessage(message: string): string {
  return message
    .replace(/(bot\d+:)[A-Za-z0-9_-]+/gu, "$1***")
    .replace(
      /([?&](?:token|access_token|corpsecret|corp_secret|encodingaeskey|aeskey|secret)=)[^&\s"'#]+/giu,
      "$1***",
    );
}

function sanitizeBotProviderError(error: unknown): unknown {
  if (!(error instanceof Error)) {
    return error;
  }
  const sanitized = sanitizeBotProviderMessage(error.message);
  if (sanitized === error.message) {
    return error;
  }
  const clone = new Error(sanitized);
  clone.name = error.name;
  if (error.stack) {
    clone.stack = error.stack.split(error.message).join(sanitized);
  }
  const cause = (error as { cause?: unknown }).cause;
  if (cause !== undefined) {
    (clone as { cause?: unknown }).cause = cause;
  }
  return clone;
}

/**
 * 第三方 Bot API 不一定会自行结束悬挂请求。所有回调 ACK 和出站消息都必须有界，
 * 否则单个请求会占住 actor 串行队列，后续权限、问答和计划审批都无法继续。
 */
export async function fetchBotProvider(
  input: string | URL | Request,
  init: RequestInit = {},
  timeoutMs = BOT_PROVIDER_REQUEST_TIMEOUT_MS,
): Promise<BotProviderResponse> {
  return runBotProviderRequest(input, init, timeoutMs, async (response) => {
    // Bugfix：fetch() 在响应头到达时就会完成，直接返回 Response 会提前撤销 deadline。
    // Telegram 调用方只需要状态，因此在受控 signal 下收完响应体后返回轻量结果。
    await response.arrayBuffer();
    return { ok: response.ok, status: response.status };
  });
}

export async function fetchBotProviderJson<T>(
  input: string | URL | Request,
  init: RequestInit = {},
  timeoutMs = BOT_PROVIDER_REQUEST_TIMEOUT_MS,
): Promise<BotProviderJsonResponse<T>> {
  return runBotProviderRequest(input, init, timeoutMs, async (response) => {
    const text = await response.text();
    let payload: T | undefined;
    if (text) {
      try {
        payload = JSON.parse(text) as T;
      } catch (error) {
        if (response.ok) {
          throw error;
        }
      }
    }
    const responseLogId = response.headers.get("x-tt-logid") ?? undefined;
    return { ok: response.ok, status: response.status, payload, responseLogId };
  });
}
