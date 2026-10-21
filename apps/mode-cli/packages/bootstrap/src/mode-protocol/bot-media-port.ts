import type { BotMediaPort } from "@mode/contracts";
import { modeBotSendMediaResultSchema, modeProtocolMethods } from "@mode/shared";
import {
  ProtocolRequestError,
  type ModeProtocolAgentServerContext,
  type ModeProtocolSessionRecord,
} from "./server-types.js";

/**
 * SendFile 的协议侧实现。
 *
 * 目标只从本会话的 activeBotDeliveryTarget 读取——它由 Host 在 bot prompt 时注入，
 * 模型无法指定收件人。路径校验、白名单、限额与 provider 分发全部留给 Host 的
 * botsService.deliverBotMedia；这里只负责把意图送到 Host。
 */
export function createProtocolBotMediaPort(
  context: ModeProtocolAgentServerContext,
  resolveOwnSession?: () => ModeProtocolSessionRecord | undefined,
): BotMediaPort {
  return {
    async sendMedia(input) {
      const target = resolveOwnSession?.()?.activeBotDeliveryTarget;
      if (!target) {
        // 正常路径下 SendFile 只会在带 botDeliveryTarget 的回合可见；
        // 这里再拒绝一次，防止旧 Host 或异常工具面绕过 turn 门控。
        throw new Error("No active IM bot delivery target for this session.");
      }
      try {
        return await context.requestClient(
          modeProtocolMethods.botSendMedia,
          {
            target,
            filePath: input.filePath,
            ...(input.caption ? { caption: input.caption } : {}),
          },
          modeBotSendMediaResultSchema,
        );
      } catch (error) {
        if (error instanceof ProtocolRequestError && error.code === -32601) {
          // 旧 Host（远端机器未升级）没有 bots/sendMedia：给出可行动提示，
          // 不把 JSON-RPC 的 method-not-found 直接抛给模型。
          throw new Error(
            "This host does not support sending files yet. Update Mode on the host machine and retry.",
          );
        }
        throw error;
      }
    },
  };
}
