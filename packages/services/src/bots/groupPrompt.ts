import type { BotGroupActivation, Locale } from "@mode/shared";
import { formatBotMessage as msg } from "./messages.js";

/**
 * 群聊回合的提示词装配与"沉默回执"判定（参考 MyAgents 的群聊上下文注入）：
 *
 * - 被触发时把规则块 + 未参与期间的历史 + 带发送者/时间/@ 标记的当前消息拼成一次输入；
 * - 「全部消息」模式下模型可以主动保持沉默：判定标记见 isGroupSilenceReply；
 *   该模式下我们不出对话卡片，因此沉默回合在用户侧完全无痕。
 *
 * 全部是纯函数，便于单测；副作用（历史 drain、工具禁用）留在调用方。
 */

/**
 * 群聊回合默认隐藏的工具：群成员（尤其"全部用户"放开的场景）不该驱动本机读写。
 * 与 MyAgents 的 DEFAULT_GROUP_TOOLS_DENY 对齐，并补上 ApplyPatch（本仓库另一种改文件工具）。
 */
export const GROUP_CHAT_TOOL_DENYLIST: readonly string[] = ["Bash", "Edit", "Write", "ApplyPatch"];

/**
 * 模型主动沉默的标记。归一化后与 `<NO_REPLY>` / `NO_REPLY` 相等才算沉默，
 * 避免把"正文里提到 NO_REPLY"误判成不回复。
 */
export function isGroupSilenceReply(text: string): boolean {
  // 只归一化 markdown/空白包装（模型常写成 **`NO_REPLY`**）；下划线是标记本体，不能剥。
  const stripped = text
    .replace(/[*`~\s]+/gu, "")
    .trim()
    .toUpperCase();
  return stripped === "<NO_REPLY>" || stripped === "NO_REPLY";
}

/** 群聊里的时间标记：本地时区的 YYYY-MM-DD HH:mm:ss（与历史行同一格式）。 */
export function formatGroupTimestamp(at: number): string {
  const date = new Date(at);
  const pad = (value: number) => String(value).padStart(2, "0");
  return [
    `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`,
    `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`,
  ].join(" ");
}

/** 通道展示名（进提示词，告诉模型"在哪个平台的群里"）。 */
export function formatBotProviderLabel(provider: string): string {
  switch (provider) {
    case "telegram":
      return "Telegram";
    case "feishu":
      return "飞书";
    case "lark":
      return "Lark";
    case "wecom":
      return "企业微信";
    case "weixin":
      return "微信";
    case "dingtalk":
      return "钉钉";
    case "webhook":
      return "Webhook";
    case "astrbot":
      return "AstrBot";
    default:
      return provider;
  }
}

/** 清洗规则块里的外部文本：标签替换为空格并折叠空白，避免群 ID / 昵称破坏 prompt 结构。 */
function sanitizeGroupField(value: string | undefined): string {
  return (value ?? "")
    .replace(/[<>[\]]/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

export interface GroupTurnPromptInput {
  botName?: string;
  activation: Extract<BotGroupActivation, "mention" | "always">;
  /** 本条消息是否 @ 了机器人（无法识别的平台按 true 处理）。 */
  isMention: boolean;
  senderName?: string;
  chatId: string;
  providerLabel: string;
  receivedAt: number;
  locale: Locale | undefined;
  /** formatGroupHistoryContext 的输出；无历史传 null。 */
  historyContext: string | null;
  content: string;
}

/**
 * 装配群聊回合的完整输入：规则块（system-reminder）→ 历史 → 当前消息（带 from/@ 标记）。
 * @提及 模式只给群身份信息；「全部消息」模式额外给回复规则与 NO_REPLY 契约。
 */
export function buildGroupTurnPrompt(input: GroupTurnPromptInput): string {
  const locale = input.locale;
  const group = sanitizeGroupField(input.chatId) || "-";
  const botName = sanitizeGroupField(input.botName) || msg(locale, "groupPromptDefaultBotName");
  const platform = sanitizeGroupField(input.providerLabel) || "-";
  const ruleLines = [
    msg(locale, "groupPromptHeader", { group, platform, botName }),
    input.activation === "always"
      ? msg(locale, "groupPromptActivationAlways")
      : msg(locale, "groupPromptActivationMention"),
    msg(locale, "groupPromptSenderNote"),
  ];
  if (input.activation === "always") {
    ruleLines.push(msg(locale, "groupPromptReplyRules", { botName }));
  }
  const parts: string[] = [`<system-reminder>\n${ruleLines.join("\n")}\n</system-reminder>`];
  if (input.historyContext) {
    parts.push(input.historyContext);
  }
  const messageLines = [
    msg(locale, "groupPromptFromLine", {
      sender:
        sanitizeGroupField(input.senderName) || msg(locale, "groupPromptUnknownSender"),
      time: formatGroupTimestamp(input.receivedAt),
    }),
  ];
  if (input.activation === "always") {
    messageLines.push(
      input.isMention
        ? msg(locale, "groupPromptMentionedMarker")
        : msg(locale, "groupPromptNotMentionedMarker"),
    );
  }
  messageLines.push(input.content);
  parts.push(messageLines.join("\n"));
  return parts.join("\n\n");
}
