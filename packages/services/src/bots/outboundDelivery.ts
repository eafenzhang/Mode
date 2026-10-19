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
  | { kind: "refresh_session" }
  | { kind: "retry_with_token"; token: string }
  | { kind: "retry_backoff"; delayMs: number }
  | { kind: "retry_without_token" }
  | { kind: "park" };

/**
 * 恢复计划：context_expired 先做主动续期探测（refresh_session，对齐 MyAgents
 * 「强刷票重试」模式——微信对应物是 /getconfig，探测可能带回新 context_token，
 * 探测调用本身也可能在服务端重建会话），再换新/同 token 重试、去 token 重试后停放；
 * retryable 有界退避后停放；session_expired 无本地恢复手段，直接停放。
 * attempt 为该条消息已执行的恢复步数；refreshed 表示续期探测已完成（或该 provider 无此能力）。
 */
export function planDeliveryRecovery(
  code: BotSendFailureCode,
  input: { attempt: number; usedToken?: string; freshToken?: string; refreshed?: boolean },
): DeliveryRecoveryPlan {
  if (code === "session_expired") {
    return { kind: "park" };
  }
  if (code === "retryable") {
    const delayMs = BOT_SEND_RETRY_DELAYS_MS[input.attempt];
    return delayMs === undefined ? { kind: "park" } : { kind: "retry_backoff", delayMs };
  }
  if (!input.refreshed) {
    return { kind: "refresh_session" };
  }
  if (input.attempt === 0) {
    if (input.freshToken && input.freshToken !== input.usedToken) {
      return { kind: "retry_with_token", token: input.freshToken };
    }
    if (input.usedToken) {
      // 探测无新 token 也同 token 重试一次：探测调用可能已重建服务端会话状态。
      return { kind: "retry_with_token", token: input.usedToken };
    }
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

export function removeFirstPendingDelivery(queue: BotPendingDelivery[]): {
  queue: BotPendingDelivery[];
  removed?: BotPendingDelivery;
} {
  const [removed, ...rest] = queue;
  return { queue: rest, ...(removed ? { removed } : {}) };
}

/** 补投失败：队首 attempts+1，超过上限丢弃该条，防止永久积压。 */
export function bumpFirstPendingDeliveryAttempt(queue: BotPendingDelivery[]): {
  queue: BotPendingDelivery[];
  dropped?: BotPendingDelivery;
} {
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

/**
 * 把「停放提示」排入队首（提示自身发送失败时用）：下一条入站补投时先送达提示、
 * 再按序补投正文（对齐 MyAgents「失败必达用户可见」模式）。
 * 超限时丢最旧的回复、保留队首提示；队列本就在入队时受同一上限约束，最多裁一条。
 */
export function prependPendingDelivery(
  queue: BotPendingDelivery[] | undefined,
  input: { text: string; providerUserId: string },
  now: number,
): { queue: BotPendingDelivery[]; dropped?: BotPendingDelivery } {
  const next = [{ ...input, queuedAt: now, attempts: 0 }, ...(queue ?? [])];
  let dropped: BotPendingDelivery | undefined;
  if (next.length > BOT_PENDING_DELIVERY_LIMIT) {
    // 从下标 1 裁（最旧的回复）；下标 0 的提示必须保留。
    const removed = next.length > 1 ? next.splice(1, 1)[0] : next.pop();
    if (removed) {
      dropped = removed;
    }
  }
  return { queue: next, ...(dropped ? { dropped } : {}) };
}

export interface BotSendQueue {
  run<T>(conversationKey: string, task: () => Promise<T>): Promise<T>;
}

/**
 * per-conversation FIFO 发送队列 + 全局最小启动间隔：同一对话内严格串行保序
 * （消息乱序由这里兜住）；不同对话可并行，不再因为同一 bot 的其他私聊/群
 * 发送慢而被整条队列拖住。全局间隔只平滑突发，不声称满足平台配额。
 */
export function createBotSendQueue(minIntervalMs = BOT_SEND_MIN_INTERVAL_MS): BotSendQueue {
  const tails = new Map<string, Promise<unknown>>();
  // 全局启动串行链：每个任务等上一任务「实际启动」后再计算等待，避免只预留
  // 时间戳时 setTimeout 在负载下乱序触发，导致实际启动间隔小于下限。
  let startTail: Promise<void> = Promise.resolve();
  let lastStartedAt = 0;
  const acquireStartSlot = (): Promise<void> => {
    const scheduled = startTail
      .catch(() => undefined)
      .then(async (): Promise<void> => {
        const waitMs = lastStartedAt + minIntervalMs - Date.now();
        if (waitMs > 0) {
          await new Promise((resolve) => setTimeout(resolve, waitMs));
        }
        lastStartedAt = Date.now();
      });
    startTail = scheduled;
    return scheduled;
  };
  return {
    async run<T>(conversationKey: string, task: () => Promise<T>): Promise<T> {
      const previous = tails.get(conversationKey) ?? Promise.resolve();
      const current = previous
        .catch(() => undefined)
        .then(async (): Promise<T> => {
          await acquireStartSlot();
          return task();
        });
      tails.set(conversationKey, current);
      try {
        return await current;
      } finally {
        if (tails.get(conversationKey) === current) {
          tails.delete(conversationKey);
        }
      }
    },
  };
}
