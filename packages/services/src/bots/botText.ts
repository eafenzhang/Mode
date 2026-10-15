/**
 * Bot 出站文本工具：跨平台共用的分片、"首帧是否成形"判断、片段切分与纯文本降级。
 *
 * 分片按 markdown 结构优先级切分（段落 → 行 → 硬切），避免把代码块、列表或单词
 * 拦腰截断；首帧判断用于流式回复的创建门槛，避免出现只有一两个字符的占位首帧；
 * 片段切分用于离散气泡档的逐块刷出（见 docs/specs/bot-reply-granularity.md）；
 * 纯文本降级给不渲染 markdown 的通道（微信）用（见 docs/specs/bot-reply-text-format.md）。
 */

/** 首帧最小字符数（按码点计数，中文与 emoji 都算一个字符）。 */
export const BOT_FIRST_SEND_MIN_CHARS = 20;

/** 逐块刷出的最小片段长度：小于它不单独成气泡，避免一句话一个气泡的碎片化。 */
export const BOT_REPLY_SEGMENT_MIN_CHARS = BOT_FIRST_SEND_MIN_CHARS;

/** 逐块刷出的兜底长度：攒够它仍无完整边界时退到弱边界切一刀，保证长段落也有进度。 */
export const BOT_REPLY_SEGMENT_MAX_CHARS = 600;

/** 强边界：切在这里的片段语义完整，可以独立成一个气泡。 */
const STRONG_BOUNDARY = /[\n。！？；;.!?]/u;

/** 弱边界：仅在兜底切分时使用；宁可断在标点/空白处，也不硬切词。 */
const WEAK_BOUNDARY = /[\s，、：,:]/u;

/**
 * 强调标记的左边界：行首，或空白 / 常见标点与开括号之后。
 * 用它保护 globstar 通配路径——那里的双星号前面是斜杠，不算强调标记。
 */
const EMPHASIS_BOUNDARY = String.raw`(^|[\s，。；：、！？,;:!?)（(\[【"'—-])`;

/**
 * 内容是否已"成形"，值得作为流式首帧发出：
 * 达到最小长度，或已出现句子边界（行尾换行或句末标点）。
 */
export function hasSentenceBoundary(text: string): boolean {
  if (!text.trim()) {
    return false;
  }
  const trimmed = text.trimEnd();
  if ([...trimmed].length >= BOT_FIRST_SEND_MIN_CHARS) {
    return true;
  }
  // 行尾换行本身就是"一整行写完了"的边界；trimEnd 会吃掉它，必须在原文上判断。
  if (text.endsWith("\n")) {
    return true;
  }
  return /[。，！？；：,.!?;:]$/u.test(trimmed);
}

/**
 * 按边界优先级分片：`\n\n` → `\n` → 硬切。
 * 归并分片边界处的换行后，内容与原文一致（不丢字符）。
 */
export function splitBotText(text: string, limit: number): string[] {
  if (limit <= 0) {
    return text.trim() ? [text] : [];
  }
  if ([...text].length <= limit) {
    return text.trim() ? [text] : [];
  }
  const chunks: string[] = [];
  let remaining = text;
  while ([...remaining].length > limit) {
    let cut = remaining.lastIndexOf("\n\n", limit);
    if (cut < limit * 0.5) {
      cut = remaining.lastIndexOf("\n", limit);
    }
    if (cut < limit * 0.5) {
      cut = limit;
    }
    const chunk = remaining.slice(0, cut);
    if (chunk.trim()) {
      chunks.push(chunk);
    }
    remaining = remaining.slice(cut).replace(/^\n+/u, "");
  }
  if (remaining.trim()) {
    chunks.push(remaining);
  }
  return chunks;
}

export interface BotReplySegmentDrain {
  /** 可以立刻发出的完整片段（已按长度上限分片）。 */
  messages: string[];
  /** 尚未成形的尾巴，留在调用方的缓冲里等下一批增量。 */
  rest: string;
}

/** 从 end-1 往前找边界，返回"切到这里"的独占下标；找不到返回 0。 */
function lastBoundaryIndex(chars: readonly string[], boundary: RegExp, end: number): number {
  for (let index = end - 1; index >= 0; index -= 1) {
    if (boundary.test(chars[index]!)) {
      return index + 1;
    }
  }
  return 0;
}

/**
 * 逐块刷出：把已成形的前缀切出来立刻发，尾巴留给下一次增量。
 *
 * 动机是离散气泡档（`assistant_changes` / `assistant_toolcalls_changes`）此前只在工具事件
 * 或回合末成块发出，纯文本回合表现为"整段一次性冒出"，长回答期间用户看不到任何正文
 * （docs/specs/bot-reply-granularity.md「逐块刷出」）。
 *
 * 规则：优先切在强边界（换行 / 句末标点）；前缀不足 `BOT_REPLY_SEGMENT_MIN_CHARS` 不单独成气泡；
 * 攒够 `limit` 仍无强边界时退到弱边界或空白处切一刀；**纯 CJK 无空白无标点时保持等待**，
 * 不硬切汉字。边界处的空白与换行不保留（气泡本就是独立消息）。
 */
export function drainCompleteBotReplySegments(
  buffer: string,
  limit = BOT_REPLY_SEGMENT_MAX_CHARS,
): BotReplySegmentDrain {
  const normalized = buffer.replace(/\r\n/g, "\n");
  const chars = [...normalized];
  let cut = lastBoundaryIndex(chars, STRONG_BOUNDARY, chars.length);
  if (cut <= 0) {
    if (chars.length < limit) {
      return { messages: [], rest: normalized };
    }
    cut = lastBoundaryIndex(chars, WEAK_BOUNDARY, Math.min(chars.length, limit));
  }
  if (cut <= 0) {
    return { messages: [], rest: normalized };
  }
  const prefix = chars.slice(0, cut).join("").trim();
  if ([...prefix].length < BOT_REPLY_SEGMENT_MIN_CHARS) {
    return { messages: [], rest: normalized };
  }
  return {
    messages: splitBotText(prefix, limit),
    rest: chars.slice(cut).join("").replace(/^\n+/u, ""),
  };
}

const codeFenceLine = /^[ \t]*```[^\n]*$/gmu;
const inlineCode = /`([^`\n]+)`/gu;
const boldAsterisk = new RegExp(`${EMPHASIS_BOUNDARY}\\*\\*([^*\\n]+)\\*\\*`, "gmu");
const boldUnderscore = new RegExp(`${EMPHASIS_BOUNDARY}__([^_\\n]+)__`, "gmu");
const headingMarker = /^[ \t]{0,3}#{1,6}[ \t]+/gmu;
const markdownLink = /\[([^\]\n]*)\]\(([^)\s]+)\)/gu;

/**
 * 纯文本通道的 markdown 降级（docs/specs/bot-reply-text-format.md）。
 *
 * 业务层文案是按 markdown 写的（粗体包命令、行内代码包路径、井号标题），
 * 但微信 iLink 的 `text_item` 是纯文本，不降级就会把标记原样显示给用户。
 *
 * 只剥离我们主动产生的标记；单个星号/下划线的斜体、行首的列表符一律不碰——
 * 回复里经常出现 glob（`src/*.ts`）与文件名（`my_file_name.md`），按斜体配对会吃掉它们。
 * 幂等：剥离后的文本再降级不变。
 */
export function toPlainTextReply(text: string): string {
  return text
    // 代码围栏行整行去掉，围栏内部内容原样保留。
    .replace(codeFenceLine, "")
    .replace(inlineCode, "$1")
    .replace(boldAsterisk, "$1$2")
    .replace(boldUnderscore, "$1$2")
    // 标题标记去掉，标题文字保留。
    .replace(headingMarker, "")
    .replace(markdownLink, "$1 ($2)");
}
