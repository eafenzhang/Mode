// P1.3 会话核心的类名契约（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.3）。
// 只使用 DESIGN.md 的语义 token；消息容器从 rounded-xl 起，用户气泡按容器层级。

export type ChatRole = "user" | "assistant" | "system" | "tool";

export function messageListClassName(className?: string): string {
  return ["flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-4 py-4", className]
    .filter(Boolean)
    .join(" ");
}

export function messageRowClassName(role: ChatRole, className?: string): string {
  const alignment = role === "user" ? "justify-end" : "justify-start";
  return ["flex w-full", alignment, className].filter(Boolean).join(" ");
}

export function messageBubbleClassName(role: ChatRole, className?: string): string {
  const base: Record<ChatRole, string> = {
    user: "max-w-2xl rounded-xl border border-card-border bg-card px-4 py-2.5 text-ui-base text-foreground",
    assistant: "max-w-3xl text-ui-base text-foreground",
    system: "max-w-2xl text-ui-sm text-foreground-subtle",
    tool: "max-w-3xl rounded-xl border border-border bg-surface px-3 py-2 font-mono text-ui-sm text-foreground",
  };
  return [base[role], "min-w-0 break-words", className].filter(Boolean).join(" ");
}

export function emptyStateClassName(className?: string): string {
  return [
    "flex min-h-0 flex-1 flex-col items-center justify-center gap-3 px-6 text-center",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function emptyTitleClassName(className?: string): string {
  return ["text-ui-base font-medium text-foreground", className].filter(Boolean).join(" ");
}

export function emptyDescriptionClassName(className?: string): string {
  return ["text-ui-sm text-foreground-subtle", className].filter(Boolean).join(" ");
}

export function streamingIndicatorClassName(className?: string): string {
  return ["inline-flex items-center gap-1 text-ui-xs text-foreground-subtlest", className]
    .filter(Boolean)
    .join(" ");
}

/** 主聊天输入 shell：DESIGN.md 批准的 rounded-2xl 例外。 */
export function chatInputShellClassName(className?: string): string {
  return [
    "rounded-2xl border border-input-border bg-input px-3 py-2",
    "focus-within:border-input-border-focused focus-within:bg-input-focused",
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function chatInputToolbarClassName(className?: string): string {
  return ["flex items-center gap-1", className].filter(Boolean).join(" ");
}
