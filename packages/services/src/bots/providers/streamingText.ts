import type { BotStreamingReplyCardState } from "./types.js";

/**
 * 流式卡片状态 → Markdown 文本的共享渲染（Telegram / 企业微信 / 钉钉卡片共用）。
 * message 块是正文，tools 块渲染为摘要列表；空内容使用调用方给的占位文案。
 */
export function renderStreamingBlocksToMarkdown(
  state: BotStreamingReplyCardState,
  emptyText: string,
): string {
  const parts: string[] = [];
  for (const block of state.blocks) {
    if (block.type === "message") {
      const text = block.text.trim();
      if (text) {
        parts.push(text);
      }
      continue;
    }
    if (block.summaries.length === 0) {
      continue;
    }
    parts.push(
      [`**${block.title ?? "Tool summaries"}**`, ...block.summaries.map((line) => `- ${line}`)].join(
        "\n",
      ),
    );
  }
  const body = parts.join("\n\n");
  if (body) {
    return body;
  }
  return emptyText;
}
