/**
 * IM 绑定菜单的对话展示降级（docs/specs/bot-binding-menu-display.md）。
 *
 * 修复原因：同一 bot 的多条对话若都没有 conversationLabel，菜单会回退显示裸
 * conversationId——实机（2026-10-10 截图）里一绑一解两行的 id 在 w-80 截断宽度内
 * 可见前缀逐字相同，叠加右侧相同的 bot id，被误判为重复条目。
 * 降级 token 取 `@` 前本地段的尾 6 字符 + `@域`：本地段先分叉，截断前即可区分；
 * 全键交给调用方放 title 悬停，诊断信息不丢。
 */
export function shortenConversationId(conversationId: string): string {
  const trimmed = conversationId.trim();
  const at = trimmed.lastIndexOf("@");
  const local = at > 0 ? trimmed.slice(0, at) : trimmed;
  const domain = at > 0 ? trimmed.slice(at) : "";
  // 过短不缩：尾 6 反而丢信息（本地段 ≤6 时原样展示已足够短）。
  if (local.length <= 6) return trimmed;
  return `…${local.slice(-6)}${domain}`;
}
