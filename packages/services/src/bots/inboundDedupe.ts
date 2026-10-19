import type { BotInboundMessage } from "@mode/shared";

/**
 * 入站消息幂等去重（纯内存，无 IO）。
 * 飞书 WebSocket 可能重投同一条 im.message.receive_v1，微信/Telegram 也可能在
 * 重试后重放同一 message id。普通消息有创建/发送任务的副作用，必须在进入业务
 * 处理前按 provider message id 幂等，避免同一句 hello 被执行两轮。
 */

/** 去重窗口：覆盖平台重连/重投的最长间隔。 */
export const BOT_INBOUND_DELIVERY_DEDUPE_TTL_MS = 2 * 60_000;

export interface InboundDeliveryDedupe {
  /** 生成去重键；平台未提供 message id 时返回 null（该消息不做去重）。 */
  key(message: BotInboundMessage): string | null;
  /** 标记已处理；返回 false 表示窗口内重复，调用方应跳过。 */
  mark(message: BotInboundMessage, now?: number): boolean;
  /** 处理失败时释放记录，允许平台重投后重试。 */
  release(message: BotInboundMessage): void;
  /** 清空全部记录（dispose）。 */
  clear(): void;
}

/** 对话身份 + provider message id 构成去重键；同一 bot 的不同对话互不影响。 */
export function buildInboundDeliveryKey(message: BotInboundMessage): string | null {
  const providerMessageId = message.actor.providerMessageId?.trim();
  if (!providerMessageId) {
    return null;
  }
  return [
    message.actor.botId,
    message.actor.provider,
    message.actor.chatId ?? message.actor.providerUserId,
    providerMessageId,
  ].join("::");
}

export function createInboundDeliveryDedupe(
  ttlMs: number = BOT_INBOUND_DELIVERY_DEDUPE_TTL_MS,
): InboundDeliveryDedupe {
  const recentAtByKey = new Map<string, number>();

  const prune = (now: number): void => {
    for (const [key, at] of recentAtByKey) {
      if (now - at >= ttlMs) {
        recentAtByKey.delete(key);
      }
    }
  };

  return {
    key: buildInboundDeliveryKey,
    mark(message, now = Date.now()): boolean {
      prune(now);
      const deliveryKey = buildInboundDeliveryKey(message);
      if (!deliveryKey) {
        return true;
      }
      if (recentAtByKey.has(deliveryKey)) {
        return false;
      }
      recentAtByKey.set(deliveryKey, now);
      return true;
    },
    release(message) {
      const deliveryKey = buildInboundDeliveryKey(message);
      if (deliveryKey) {
        recentAtByKey.delete(deliveryKey);
      }
    },
    clear() {
      recentAtByKey.clear();
    },
  };
}
