import type { Locale } from "@mode/shared";
import { formatBotMessage as msg } from "./messages.js";

/**
 * 跨聊天交接的回合注入文案（docs/specs/bot-cross-chat-handoff.md R2/R3）。
 * 纯函数、无 IO：注入块包 <system-reminder>——标题推导取原始 text、
 * 展示层 strip 既有 system-reminder，两侧语义自动成立。
 */

export interface HandoffTargetConversation {
  conversationKey: string;
  conversationLabel?: string;
  conversationId: string;
}

export function buildHandoffInboundPrompt(params: {
  source: string;
  context?: string;
  content: string;
  locale?: Locale;
}): string {
  const lines = [
    msg(params.locale, "handoffInboundHeader", { source: params.source }),
    ...(params.context
      ? [msg(params.locale, "handoffInboundContext", { context: params.context })]
      : []),
    msg(params.locale, "handoffInboundNote"),
  ];
  return [`<system-reminder>\n${lines.join("\n")}\n</system-reminder>`, params.content].join("\n\n");
}

export function formatHandoffTargets(
  conversations: ReadonlyArray<HandoffTargetConversation>,
  locale?: Locale,
): string {
  const lines = [
    msg(locale, "handoffTargetsHeader"),
    ...conversations.map((item) =>
      msg(
        locale,
        item.conversationLabel?.trim() ? "handoffTargetsLine" : "handoffTargetsLineNoLabel",
        {
          key: item.conversationKey,
          label: item.conversationLabel?.trim() || item.conversationId,
        },
      ),
    ),
  ];
  return `<system-reminder>\n${lines.join("\n")}\n</system-reminder>`;
}
