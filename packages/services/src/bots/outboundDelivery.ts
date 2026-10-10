import type { BotPendingDelivery } from "@mode/shared";

/**
 * 出站投递失败的恢复决策与停放队列（纯函数，无 IO）。
 * 事故依据：2026-10-10 微信 /sendmessage 连续返回 prepare failed，回复被
 * stream 队列 warn 后丢弃（docs/specs/bot-outbound-delivery.md）。
 */

/** 微信服务端把会话准备失败报为 errmsg="prepare failed"；换用结构化 code 驱动恢复。 */
export type BotSendFailureCode = "context_expired" | "retryable" | "session_expired";

export class BotSendError extends Error {
  readonly code: BotSendFailureCode;
  readonly detail?: string;
  constructor(code: BotSendFailureCode, message: string, detail?: string) {
    super(message);
    this.name = "BotSendError";
    this.code = code;
    this.detail = detail;
  }
}

/** 只有显式结构化错误可分类；其余错误保持既有冒泡语义，不进入新恢复管线。 */
export function classifySendError(error: unknown): BotSendFailureCode | null {
  return error instanceof BotSendError ? error.code : null;
}

/** 瞬时抖动的退避节奏；两次用尽即停放，避免长时间占住 per-bot 发送队列。 */
export const BOT_SEND_RETRY_DELAYS_MS: readonly number[] = [500, 2_000];

/** 同一 bot 相邻发送的最小间隔，仅平滑突发，不声称满足平台配额。 */
export const BOT_SEND_MIN_INTERVAL_MS = 300;

export const BOT_PENDING_DELIVERY_LIMIT = 100;
export const BOT_PENDING_DELIVERY_MAX_ATTEMPTS = 8;

export type DeliveryRecoveryPlan =
  | { kind: "retry_with_token"; token: string }
  | { kind: "retry_backoff"; delayMs: number }
  | { kind: "retry_without_token" }
  | { kind: "park" };

/**
 * 恢复计划：context_expired 先换新 token、再去 token（各一次）后停放；
 * retryable 有界退避后停放；session_expired 无本地恢复手段，直接停放。
 * attempt 为该条消息已执行的恢复步数。
 */
export function planDeliveryRecovery(
  code: BotSendFailureCode,
  input: { attempt: number; usedToken?: string; freshToken?: string },
): DeliveryRecoveryPlan {
  if (code === "session_expired") {
    return { kind: "park" };
  }
  if (code === "retryable") {
    const delayMs = BOT_SEND_RETRY_DELAYS_MS[input.attempt];
    return delayMs === undefined ? { kind: "park" } : { kind: "retry_backoff", delayMs };
  }
  if (input.attempt === 0 && input.freshToken && input.freshToken !== input.usedToken) {
    return { kind: "retry_with_token", token: input.freshToken };
  }
  if (input.attempt <= 1 && input.usedToken) {
    return { kind: "retry_without_token" };
  }
  return { kind: "park" };
}

export function enqueuePendingDelivery(
  queue: BotPendingDelivery[] | undefined,
  input: { text: string; providerUserId: string },
  now: number,
): { queue: BotPendingDelivery[]; dropped?: BotPendingDelivery } {
  const next = [...(queue ?? []), { ...input, queuedAt: now, attempts: 0 }];
  let dropped: BotPendingDelivery | undefined;
  while (next.length > BOT_PENDING_DELIVERY_LIMIT) {
    dropped = next.shift();
  }
  return { queue: next, ...(dropped ? { dropped } : {}) };
}

export function removeFirstPendingDelivery(
  queue: BotPendingDelivery[],
): { queue: BotPendingDelivery[]; removed?: BotPendingDelivery } {
  const [removed, ...rest] = queue;
  return { queue: rest, ...(removed ? { removed } : {}) };
}

/** 补投失败：队首 attempts+1，超过上限丢弃该条，防止永久积压。 */
export function bumpFirstPendingDeliveryAttempt(
  queue: BotPendingDelivery[],
): { queue: BotPendingDelivery[]; dropped?: BotPendingDelivery } {
  const [first, ...rest] = queue;
  if (!first) {
    return { queue: [] };
  }
  const attempts = first.attempts + 1;
  if (attempts > BOT_PENDING_DELIVERY_MAX_ATTEMPTS) {
    return { queue: rest, dropped: first };
  }
  return { queue: [{ ...first, attempts }, ...rest] };
}

export interface BotSendQueue {
  run<T>(botId: string, task: () => Promise<T>): Promise<T>;
}

/**
 * per-bot FIFO 发送队列 + 最小间隔：同一 bot 的出站严格串行（微信/企微等
 * 平台侧限流与消息乱序都由这里兜住），不同 bot 互不阻塞。
 */
export function createBotSendQueue(minIntervalMs = BOT_SEND_MIN_INTERVAL_MS): BotSendQueue {
  const tails = new Map<string, Promise<unknown>>();
  const lastStartedAtByBot = new Map<string, number>();
  return {
    async run<T>(botId: string, task: () => Promise<T>): Promise<T> {
      const previous = tails.get(botId) ?? Promise.resolve();
      const current = previous
        .catch(() => undefined)
        .then(async (): Promise<T> => {
          const lastStartedAt = lastStartedAtByBot.get(botId) ?? 0;
          const waitMs = lastStartedAt + minIntervalMs - Date.now();
          if (waitMs > 0) {
            await new Promise((resolve) => setTimeout(resolve, waitMs));
          }
          lastStartedAtByBot.set(botId, Date.now());
          return task();
        });
      tails.set(botId, current);
      try {
        return await current;
      } finally {
        if (tails.get(botId) === current) {
          tails.delete(botId);
        }
      }
    },
  };
}
