import type { BotConfig, BotProvider, BotReplyGranularity } from "@zcode/shared";
import { getSupportedBotReplyGranularities } from "@zcode/shared";

export type BotProviderEntryId = BotProvider | "dingding";

/**
 * 通道目录：只保留微信 / 企业微信 / 飞书（中国域）/ 钉钉，且全部一次展示（不再折叠）。
 * 其余 provider（Lark / Telegram / AstrBot / Webhook / Discord）不再出现在新建面板，
 * 已有这些通道的机器人仍照常展示、编辑与删除。
 */
type BotProviderEntry = {
  id: BotProvider;
  label: string;
  implemented: boolean;
};

export const BOT_PROVIDERS: BotProviderEntry[] = [
  { id: "weixin", label: "Weixin", implemented: true },
  { id: "wecom", label: "WeCom", implemented: true },
  { id: "feishu", label: "Feishu", implemented: true },
  { id: "dingtalk", label: "DingTalk", implemented: true },
];

export const BOT_REPLY_GRANULARITIES: Array<{
  id: BotReplyGranularity;
  labelId: string;
  descriptionId: string;
}> = [
  {
    id: "assistant_changes",
    labelId: "bots.replyGranularity.assistantChanges",
    descriptionId: "bots.replyGranularity.assistantChanges.description",
  },
  {
    id: "assistant_toolcalls_changes",
    labelId: "bots.replyGranularity.assistantToolcallsChanges",
    descriptionId: "bots.replyGranularity.assistantToolcallsChanges.description",
  },
  {
    id: "summary_changes",
    labelId: "bots.replyGranularity.summaryChanges",
    descriptionId: "bots.replyGranularity.summaryChanges.description",
  },
  {
    id: "streaming_card",
    labelId: "bots.replyGranularity.streamingCard",
    descriptionId: "bots.replyGranularity.streamingCard.description",
  },
];

export const DEFAULT_BOT_REPLY_GRANULARITY_ENTRY = BOT_REPLY_GRANULARITIES[0]!;

export function getBotReplyGranularitiesForProvider(
  provider: BotProvider,
): typeof BOT_REPLY_GRANULARITIES {
  const supportedIds = new Set(getSupportedBotReplyGranularities(provider));
  return BOT_REPLY_GRANULARITIES.filter((granularity) => supportedIds.has(granularity.id));
}

export function getBotReplyGranularityEntryForProvider(
  provider: BotProvider,
  replyMode: BotReplyGranularity,
) {
  const granularities = getBotReplyGranularitiesForProvider(provider);
  return (
    granularities.find((granularity) => granularity.id === replyMode) ??
    granularities[0] ??
    DEFAULT_BOT_REPLY_GRANULARITY_ENTRY
  );
}

export function getBotProviderRegionTagLabelId(provider: BotProviderEntryId): string | null {
  switch (provider) {
    case "lark":
      return "login.oauth.regionTag.zai";
    case "feishu":
      return "login.oauth.regionTag.bigmodel";
    default:
      return null;
  }
}

export function buildCurrentWorkspaceId(workspacePath: string, workspaceIdentity?: string): string {
  return workspaceIdentity?.trim() || workspacePath;
}

type BotProviderEntryResolution =
  | { mode: "select"; botId: string }
  | { mode: "create"; provider: BotProvider };

export function resolveBotProviderEntry(
  bots: BotConfig[],
  provider: BotProvider,
): BotProviderEntryResolution {
  const existingBot = bots.find((bot) => bot.provider === provider);
  if (existingBot) {
    return { mode: "select", botId: existingBot.id };
  }

  return { mode: "create", provider };
}
