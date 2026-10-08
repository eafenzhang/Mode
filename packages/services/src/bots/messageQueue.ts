import type { BotQueuedMessage } from "@mode/shared";

/**
 * 任务运行中收到的入站消息队列。旧实现直接回复“任务运行中”并丢弃消息，
 * 用户必须盯着任务结束再重发；这里改为暂存，任务终态后按序自动投递。
 * 队列随 BotState 持久化，重启不丢。纯函数实现，便于单测。
 */

/** 队列上限；超出后丢弃最旧消息，保证语义始终是“最新指令优先”。 */
export const BOT_MESSAGE_QUEUE_LIMIT = 20;

/** 相同文本在窗口内的重复入站视为平台重试，不重复入队。 */
const DEDUPE_WINDOW_MS = 60_000;

export interface BotQueuedMessageInput {
  text: string;
  providerUserId?: string;
  displayName?: string;
  chatId?: string;
}

export interface EnqueueQueuedMessageResult {
  queue: BotQueuedMessage[];
  /** 本次入队后该消息的位次（1 开始）；被去重跳过时返回已有位次 */
  position: number;
  /** 因队列满而被丢弃的最旧消息 */
  dropped?: BotQueuedMessage;
}

export function enqueueQueuedMessage(
  queue: BotQueuedMessage[] | undefined,
  input: BotQueuedMessageInput,
  now = Date.now(),
): EnqueueQueuedMessageResult {
  const existing = queue ?? [];
  const trimmed = input.text.trim();
  const duplicateIndex = existing.findIndex(
    (item) =>
      item.text === trimmed &&
      item.text.length > 0 &&
      now - item.receivedAt < DEDUPE_WINDOW_MS,
  );
  if (duplicateIndex >= 0) {
    return { queue: existing, position: duplicateIndex + 1 };
  }
  const next: BotQueuedMessage[] = [
    ...existing,
    {
      text: trimmed,
      receivedAt: now,
      ...(input.providerUserId ? { providerUserId: input.providerUserId } : {}),
      ...(input.displayName ? { displayName: input.displayName } : {}),
      ...(input.chatId ? { chatId: input.chatId } : {}),
    },
  ];
  let dropped: BotQueuedMessage | undefined;
  while (next.length > BOT_MESSAGE_QUEUE_LIMIT) {
    dropped = next.shift();
  }
  return { queue: next, position: next.length, dropped };
}

export function dequeueQueuedMessage(
  queue: BotQueuedMessage[] | undefined,
): { next: BotQueuedMessage[]; message: BotQueuedMessage | null } {
  const existing = queue ?? [];
  if (existing.length === 0) {
    return { next: existing, message: null };
  }
  const [message, ...rest] = existing;
  return { next: rest, message: message ?? null };
}
