// P1.1 基础件补漏：Badge 的类名契约（adapter map §5）。
// 语义色只用于真实状态；默认 neutral。

export type BadgeTone = "neutral" | "success" | "warning" | "destructive";

const BADGE_TONE_CLASSES: Record<BadgeTone, string> = {
  neutral: "text-foreground-subtle",
  success: "text-success",
  warning: "text-warning",
  destructive: "text-destructive",
};

export interface BadgeClassNameOptions {
  tone?: BadgeTone;
  /** 刻意 pill 形状时才使用 rounded-full。 */
  pill?: boolean;
  className?: string;
}

export function badgeClassName(options: BadgeClassNameOptions = {}): string {
  const tone = options.tone ?? "neutral";
  return [
    "inline-flex items-center gap-1 border border-border bg-surface px-1.5 py-0.5 text-ui-xs font-medium",
    options.pill ? "rounded-full" : "rounded-md",
    BADGE_TONE_CLASSES[tone],
    options.className,
  ]
    .filter(Boolean)
    .join(" ");
}
