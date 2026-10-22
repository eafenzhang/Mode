import type { BotChannelState, BotConfig, BotRuntimeInfo } from "@mode/shared";

/**
 * 运行状态投影：`runtimeByBotId` 里的条目只由 `setRuntimeStatus` 写入运行态字段，
 * 微信激活标记却是通道级状态（写在 bot-state 的 `weixinActivatedAt`）。
 *
 * 修复原因（2026-10-21）：此前激活标记只挂在「没有 runtime」的兜底分支上，
 * 微信 Bot 一旦启用就必然有 runtime（长轮询/错误态），首条入站消息写入的
 * `weixinActivatedAt` 因此永远投影不到 UI，卡片一直停在「待激活」。
 */
export function projectBotRuntimeInfo(input: {
  bot: Pick<BotConfig, "id" | "provider" | "enabled">;
  runtime: BotRuntimeInfo | undefined;
  channelState: Pick<BotChannelState, "weixinActivatedAt"> | undefined;
}): BotRuntimeInfo {
  const { bot, runtime, channelState } = input;
  const base: BotRuntimeInfo =
    runtime ?? {
      botId: bot.id,
      provider: bot.provider,
      status: bot.enabled ? "idle" : "disabled",
      message: bot.enabled ? "Bot is configured." : "Bot is disabled.",
    };
  const activatedAt = channelState?.weixinActivatedAt;
  return activatedAt === undefined ? base : { ...base, weixinActivatedAt: activatedAt };
}
