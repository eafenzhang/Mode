import type { Locale } from "@mode/shared";
import { formatBotMessage as msg } from "./messages.js";

/**
 * 私聊回合的渠道上下文（docs/specs/im-bot-agent-channel-aplus.md 规则 9）。
 *
 * 群聊一直有 `<system-reminder>` 规则块，私聊此前只发原始文本，模型并不知道自己
 * 在 IM 里回答——容易输出“打开桌面端/点击侧栏”这类桌面指令，也不会主动用
 * SendFile 交付文件。这里补上与群聊同构的最小上下文，不引入第二份会话状态。
 */
export interface PrivateTurnPromptInput {
  providerLabel: string;
  /** 该通道是否实现了出站媒体；没有时不要提示模型调用 SendFile，否则必失败。 */
  mediaEnabled: boolean;
  content: string;
  locale?: Locale;
}

export function buildPrivateTurnPrompt(input: PrivateTurnPromptInput): string {
  const rules = [
    msg(input.locale, "privatePromptHeader", { platform: input.providerLabel }),
    msg(input.locale, "privatePromptReplyStyle"),
    ...(input.mediaEnabled ? [msg(input.locale, "privatePromptFileHint")] : []),
  ];
  return [
    `<system-reminder>\n${rules.join("\n")}\n</system-reminder>`,
    input.content,
  ].join("\n\n");
}
