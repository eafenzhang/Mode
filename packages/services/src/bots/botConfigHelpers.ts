import {
  normalizeBotReplyGranularity,
  resolveBotAccessMode,
  type BotActor,
  type BotCommandPolicy,
  type BotConfig,
  type BotProvider,
  type BotsConfigFile,
} from "@mode/shared";
import { normalizeBotCommandPolicy, normalizeBotCurrentOptions } from "./config.js";
import { normalizeAllowedWorkspaces } from "./workspaceHelpers.js";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function getContextKey(bot: Pick<BotConfig, "id">): string {
  return bot.id;
}

export function findBot(config: BotsConfigFile, botId: string): BotConfig | null {
  return config.bots.find((bot) => bot.id === botId) ?? null;
}

export function findCallbackBot(
  config: BotsConfigFile,
  provider: BotProvider,
  payload: unknown,
): BotConfig | null {
  if (!isRecord(payload)) {
    return null;
  }
  const botId = typeof payload.botId === "string" ? payload.botId : "";
  if (botId) {
    return findBot(config, botId);
  }
  return config.bots.filter((bot) => bot.provider === provider && bot.enabled).at(0) ?? null;
}

export function findAuthorizedBot(config: BotsConfigFile, actor: BotActor): BotConfig | null {
  // 通道身份由 botId 精确定位；发送者是否有权驱动由 findBoundUser / 群授权闸门决定。
  // 不能在这里按 providerUserId 直接找 bot：否则未绑定用户的群消息会提前返回
  // “botDisabled”，既无法进入群历史，也无法走群授权流程。
  return (
    config.bots.find(
      (bot) => bot.enabled && bot.id === actor.botId && bot.provider === actor.provider,
    ) ?? null
  );
}

export function findBoundUser(bot: BotConfig, actor: BotActor): BotConfig | null {
  // 绑定用户、allowedUsers、free 三种放行方式对所有平台一致。
  // 微信不再无条件放行：A+ 要求微信/企微/飞书/钉钉共用同一套私聊准入；
  // 访问模式由共享解析函数 resolveBotAccessMode 统一给出（spec bot-access-mode R1），
  // 不在各调用点手写 provider 分支。
  if (bot.providerUserId === actor.providerUserId) {
    return bot;
  }
  // free 模式任何私聊用户可驱动（原 all_users 语义，已切换到统一 accessMode，spec R1）。
  if (actor.chatType === "private" && resolveBotAccessMode(bot) === "free") {
    return bot;
  }
  return bot.allowedUsers?.includes(actor.providerUserId) ? bot : null;
}

export function normalizeBotConfig(bot: BotConfig): BotConfig {
  const normalized = {
    ...bot,
    // Bot 配置化后 allowedWorkspaces 是唯一 workspace 权限边界；空数组统一落成 "*"。
    allowedWorkspaces: normalizeAllowedWorkspaces(bot.allowedWorkspaces),
    allowedCommands: normalizeBotCommandPolicy(bot.allowedCommands),
    currentOptions: normalizeBotCurrentOptions(bot.currentOptions),
    // Bugfix: Feishu/Lark 的回复颗粒度依赖 Card JSON 2.0 单卡更新，旧配置不能继续保留普通消息模式。
    replyMode: normalizeBotReplyGranularity(bot.provider, bot.replyMode),
    // 读取期回填访问模式：存量推导结果随下次写盘持久化（spec R5），显式值原样保留。
    accessMode: resolveBotAccessMode(bot),
  };
  if (normalized.provider !== "weixin") {
    return normalized;
  }
  // 微信使用内置 iLink 地址，保存时清理 webhookUrl，避免把其他 provider 的出站字段残留到微信配置。
  delete normalized.webhookUrl;
  return normalized;
}

function getUserCommandPolicy(bot: BotConfig): BotCommandPolicy {
  return normalizeBotCommandPolicy(bot.allowedCommands);
}

export function isUserCommandAllowed(
  bot: BotConfig,
  requestedCommand:
    | "help"
    | "status"
    | "new"
    | "reconnect"
    | "workspace"
    | "model"
    | "mode"
    | "thoughtLevel"
    | "task"
    | "reply"
    | "stop"
    | "message"
    | "approve",
): boolean {
  if (
    requestedCommand === "help" ||
    requestedCommand === "message" ||
    requestedCommand === "approve" ||
    requestedCommand === "task" ||
    requestedCommand === "stop"
  ) {
    return true;
  }
  if (requestedCommand === "reconnect") {
    return getUserCommandPolicy(bot).workspace !== false;
  }
  return getUserCommandPolicy(bot)[requestedCommand] !== false;
}

export function normalizeConfigBots(config: BotsConfigFile): BotsConfigFile {
  return {
    ...config,
    bots: config.bots.map(normalizeBotConfig),
  };
}

/** 私聊黑名单命中（spec bot-access-mode R2）：纯否决层，无字段视为未拉黑。 */
export function isUserBlocked(bot: BotConfig, providerUserId: string): boolean {
  return bot.blockedUsers?.includes(providerUserId) ?? false;
}

/** 群黑名单命中（spec bot-access-mode R2）；groupId 为空串不命中。 */
export function isGroupBlocked(bot: BotConfig, groupId: string): boolean {
  if (!groupId) {
    return false;
  }
  return bot.blockedGroups?.some((item) => item.groupId === groupId) ?? false;
}
