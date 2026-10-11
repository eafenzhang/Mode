import type { BotConfig, BotsStateFile } from "@mode/shared";
import { makeBotConversationKey } from "@mode/shared";

/**
 * 微信 providerUserId 误写迁移规划（docs/specs/weixin-provider-self-id.md）。
 *
 * 纯函数：扫码注册曾把上游 ilink_bot_id（机器人自身 id）双写进 providerUserId
 * 与 displayName，语义上侵占了"绑定用户"字段——预判私聊、心跳目标、绑定展示
 * 全部跟着读错。这里把注册特征命中的值挪进 providerSelfId，并列出状态里以
 * 自身 id 为键的幻影对话（真实入站永远不会命中它，实机日志已证实）。
 *
 * 判别只认注册双写特征，/bind 写入的真实用户 id（displayName 是昵称、
 * id 不以 @im.bot 结尾）绝不触碰；非微信 provider 永不进入迁移。
 */

export interface WeixinSelfIdMigrationPlan {
  /** 迁移后的 bot 列表；未变更时与输入相同（引用相等，便于调用方判 changed）。 */
  bots: BotConfig[];
  changed: boolean;
  /** 需要删除的幻影对话槽位（先 writeDraftContext 撤订阅，再删键）。 */
  removedConversations: Array<{ botId: string; conversationKey: string }>;
}

/** 注册误写判别：双写特征（与 displayName 相同）或上游 bot id 的域后缀。 */
function isRegistrationSelfId(bot: BotConfig): boolean {
  const value = bot.providerUserId?.trim();
  if (!value) {
    return false;
  }
  if (bot.displayName && value === bot.displayName.trim()) {
    return true;
  }
  return value.endsWith("@im.bot");
}

export function planWeixinSelfIdMigration(
  bots: readonly BotConfig[],
  state: BotsStateFile,
): WeixinSelfIdMigrationPlan {
  const nextBots: BotConfig[] = [];
  const removedConversations: Array<{ botId: string; conversationKey: string }> = [];
  let changed = false;

  for (const bot of bots) {
    if (bot.provider !== "weixin") {
      nextBots.push(bot);
      continue;
    }
    const existingSelfId = bot.providerSelfId?.trim();
    const shouldMigrate = !existingSelfId && isRegistrationSelfId(bot);
    const providerSelfId = existingSelfId ?? (shouldMigrate ? bot.providerUserId!.trim() : "");
    if (shouldMigrate) {
      nextBots.push({ ...bot, providerSelfId, providerUserId: undefined });
      changed = true;
    } else {
      nextBots.push(bot);
    }
    // 幻影对话 = 以 bot 自身 id 为键的私聊槽位；只删这一条，真实对话不受影响。
    // 已迁移配置也重查：配置写入与状态删除非同一事务，失败重跑要能补齐清理。
    if (providerSelfId) {
      const phantomKey = makeBotConversationKey("private", providerSelfId);
      if (state.bots[bot.id]?.conversations[phantomKey]) {
        removedConversations.push({ botId: bot.id, conversationKey: phantomKey });
      }
    }
  }

  return { bots: nextBots, changed, removedConversations };
}
