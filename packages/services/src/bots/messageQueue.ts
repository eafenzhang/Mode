import type { BotQueuedAttachment, BotQueuedMessage } from "@mode/shared";

/**
 * 任务运行中收到的入站消息队列。旧实现直接回复“任务运行中”并丢弃消息，
 * 用户必须盯着任务结束再重发；这里改为暂存，任务终态后按序自动投递。
 * 队列随 BotState 持久化，重启不丢。纯函数实现，便于单测。
 */

/** 队列上限；超出后丢弃最旧消息，保证语义始终是“最新指令优先”。 */
export const BOT_MESSAGE_QUEUE_LIMIT = 20;

/** 相同文本在窗口内的重复入站视为平台重试，不重复入队。 */
const DEDUPE_WINDOW_MS = 60_000;

/** 单对话队列内已物化附件的字节预算；超出则调用方回退为"不入队"
 *  （docs/specs/bot-queued-message-attachments.md 规则 2）。 */
export const BOT_QUEUED_ATTACHMENT_BUDGET_BYTES = 20 * 1024 * 1024;

export interface BotQueuedMessageInput {
  text: string;
  providerUserId?: string;
  displayName?: string;
  chatId?: string;
  /** 入队前已物化的附件（本地路径 + 展示元数据）。 */
  attachments?: BotQueuedAttachment[];
}

export interface EnqueueQueuedMessageResult {
  queue: BotQueuedMessage[];
  /** 本次入队后该消息的位次（1 开始）；被去重跳过时返回已有位次 */
  position: number;
  /** 因队列满而被丢弃的最旧消息 */
  dropped?: BotQueuedMessage;
}

/** 一组附件的字节合计（缺 sizeBytes 按 0 计）。 */
export function attachmentBytes(
  attachments: readonly BotQueuedAttachment[] | undefined,
): number {
  return (attachments ?? []).reduce((sum, item) => sum + (item.sizeBytes ?? 0), 0);
}

/** 队列内已物化附件的字节合计；与 `attachmentBytes` 一起做入队预算判定。 */
export function queuedAttachmentBytes(
  queue: readonly BotQueuedMessage[] | undefined,
): number {
  return (queue ?? []).reduce(
    (total, item) => total + attachmentBytes(item.attachments),
    0,
  );
}

export function enqueueQueuedMessage(
  queue: BotQueuedMessage[] | undefined,
  input: BotQueuedMessageInput,
  now = Date.now(),
): EnqueueQueuedMessageResult {
  const existing = queue ?? [];
  const trimmed = input.text.trim();
  const attachments =
    input.attachments && input.attachments.length > 0 ? input.attachments : undefined;
  // Bugfix: 纯附件消息的 text 可能为空，按文本去重既无意义，还会把"两张不同截图、同样空文本"
  // 误判成平台重试（docs/specs/bot-queued-message-attachments.md 规则 4）。
  const duplicateIndex = attachments
    ? -1
    : existing.findIndex(
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
      ...(attachments ? { attachments } : {}),
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
