import type { BotHandoffPort } from "@mode/contracts";
import { modeBotHandoffResultSchema, modeProtocolMethods } from "@mode/shared";
import {
  ProtocolRequestError,
  type ModeProtocolAgentServerContext,
  type ModeProtocolSessionRecord,
} from "./server-types.js";

/**
 * HandoffMessage 的协议侧实现。
 *
 * 目标对话键来自模型，但源 bot 身份（provider/botId）只从本会话的
 * activeBotDeliveryTarget 读取——它由 Host 在 bot prompt 时注入，模型无法冒充别的 bot。
 * 目标归属校验、黑名单、附件白名单与准入全部留给 Host 的 botsService.handoffMessage；
 * 这里只负责把意图送到 Host，并把旧 Host 的 method-not-found 翻译成可行动提示。
 */
export function createProtocolBotHandoffPort(
  context: ModeProtocolAgentServerContext,
  resolveOwnSession?: () => ModeProtocolSessionRecord | undefined,
): BotHandoffPort {
  return {
    async handoff(input) {
      const target = resolveOwnSession?.()?.activeBotDeliveryTarget;
      if (!target) {
        // 正常路径下 HandoffMessage 只会在带 botDeliveryTarget 的回合可见；
        // 这里再拒绝一次，防止旧 Host 或异常工具面绕过 turn 门控。
        throw new Error("No active IM bot delivery target for this session.");
      }
      try {
        const result = await context.requestClient(
          modeProtocolMethods.botHandoff,
          {
            target,
            conversationKey: input.target,
            message: input.message,
            ...(input.context ? { context: input.context } : {}),
            ...(input.attachments ? { attachments: input.attachments } : {}),
            toolCallId: input.toolCallId,
          },
          modeBotHandoffResultSchema,
        );
        return { status: result.status, target: result.conversationKey };
      } catch (error) {
        if (error instanceof ProtocolRequestError && error.code === -32601) {
          // 旧 Host（远端机器未升级）没有 bots/handoff：给出可行动提示，
          // 不把 JSON-RPC 的 method-not-found 直接抛给模型。
          throw new Error(
            "This host does not support cross-chat handoff yet. Update Mode on the host machine and retry.",
          );
        }
        throw error;
      }
    },
  };
}
