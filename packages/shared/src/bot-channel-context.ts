/**
 * Bot 回合注入给模型的渠道上下文（群/私聊 system-reminder、引用消息行）。
 *
 * 这些内容必须进入模型 prompt，但不是用户真正输入的内容。桌面会话、手机远控和
 * 任务标题都按“用户可见文本”展示，不能把注入块原样渲染成用户气泡。
 * 纯函数、幂等；只剥离我们自己的标记，不碰普通 `<system-reminder>`。
 */
const BOT_CONTEXT_MARKERS = [
  "[渠道信息]",
  "[Channel]",
  "[群聊信息]",
  "[Group chat]",
] as const;

const QUOTE_PREFIXES = ["[引用消息] ", "[Quoted message] "] as const;

const SYSTEM_REMINDER_OPEN = "<system-reminder>";
const SYSTEM_REMINDER_CLOSE = "</system-reminder>";

const CURRENT_MESSAGE_MARKER = /^\[(?:本条消息|This message)[^\]]*\]$/u;

export function stripBotChannelContextForDisplay(text: string): string {
  let result = text;
  let sawBotContext = false;
  // 允许未来同时注入多个 bot 上下文块；普通 system-reminder 不匹配标记时原样保留。
  for (;;) {
    const trimmed = result.trimStart();
    if (!trimmed.startsWith(SYSTEM_REMINDER_OPEN)) {
      break;
    }
    const closeIndex = trimmed.indexOf(SYSTEM_REMINDER_CLOSE);
    if (closeIndex < 0) {
      break;
    }
    const block = trimmed.slice(0, closeIndex);
    if (!BOT_CONTEXT_MARKERS.some((marker) => block.includes(marker))) {
      break;
    }
    sawBotContext = true;
    result = trimmed.slice(closeIndex + SYSTEM_REMINDER_CLOSE.length).trimStart();
  }
  if (sawBotContext) {
    // 群聊 prompt 的结构是：历史（多行 [from: ...]）+ 当前消息 [from: ...] + @ 标记 + 正文。
    // 可见气泡只保留最后一条 [from: ...] 之后的正文，历史行与发送者元数据都不进 UI。
    const fromLines = [...result.matchAll(/^\[from: .*\]$/gmu)];
    const lastFrom = fromLines.at(-1);
    if (lastFrom?.index !== undefined) {
      result = result.slice(lastFrom.index + lastFrom[0].length).trimStart();
    }
    for (;;) {
      const lineEnd = result.indexOf("\n");
      const line = (lineEnd >= 0 ? result.slice(0, lineEnd) : result).trim();
      if (!CURRENT_MESSAGE_MARKER.test(line)) {
        break;
      }
      result = lineEnd >= 0 ? result.slice(lineEnd + 1).trimStart() : "";
    }
  }
  for (const prefix of QUOTE_PREFIXES) {
    if (!result.startsWith(prefix)) {
      continue;
    }
    const separator = result.indexOf("\n\n");
    if (separator >= 0) {
      result = result.slice(separator + 2).trimStart();
    }
    break;
  }
  return result;
}
