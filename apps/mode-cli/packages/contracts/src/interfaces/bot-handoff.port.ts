/**
 * 跨聊天交接端口（docs/specs/bot-cross-chat-handoff.md R1）。
 *
 * 只在 IM bot 回合内由 Host 注入：模型只表达「把消息交给本 bot 的哪个对话」，
 * 源 bot 身份（provider/botId）由 Host 从本轮 prompt 的 botDeliveryTarget 决定，
 * 目标归属校验、黑名单、附件白名单与准入全部由 Host 侧 botsService.handoffMessage 执行。
 */
export interface BotHandoffPort {
  handoff(input: BotHandoffInput): Promise<BotHandoffOutput>;
}

export interface BotHandoffInput {
  /** 目标对话键：`private:<userId>` / `group:<chatId>`，取自回合上下文注入的清单。 */
  target: string;
  message: string;
  context?: string;
  attachments?: string[];
  /** 幂等键：Host 侧 TTL 内重复投递静默丢弃（spec R5）。 */
  toolCallId: string;
}

export interface BotHandoffOutput {
  status: "submitted" | "queued" | "duplicate";
  target: string;
}
