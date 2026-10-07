/**
 * Bot 出站文本工具：跨平台共用的分片与"首帧是否成形"判断。
 *
 * 分片按 markdown 结构优先级切分（段落 → 行 → 硬切），避免把代码块、列表或单词
 * 拦腰截断；首帧判断用于流式回复的创建门槛，避免出现只有一两个字符的占位首帧。
 */

/** 首帧最小字符数（按码点计数，中文与 emoji 都算一个字符）。 */
export const BOT_FIRST_SEND_MIN_CHARS = 20;

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
