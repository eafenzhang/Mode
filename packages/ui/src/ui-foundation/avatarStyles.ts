// P1.1 基础件补漏：Avatar / AvatarGroup 的类名契约（adapter map §1 缺口）。

export type AvatarSize = "sm" | "default" | "lg";
export type AvatarBadgeTone = "neutral" | "success" | "warning" | "destructive";

const AVATAR_SIZE_CLASSES: Record<AvatarSize, string> = {
  sm: "size-6 text-ui-xs",
  default: "size-8 text-ui-xs",
  lg: "size-10 text-ui-sm",
};

export function avatarClassName(size: AvatarSize = "default", className?: string): string {
  return [
    "relative inline-flex shrink-0 items-center justify-center overflow-hidden rounded-full bg-surface font-medium text-foreground-subtle",
    AVATAR_SIZE_CLASSES[size],
    className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function avatarImageClassName(className?: string): string {
  return ["size-full object-cover", className].filter(Boolean).join(" ");
}

export function avatarFallbackClassName(className?: string): string {
  return ["flex size-full items-center justify-center bg-surface", className]
    .filter(Boolean)
    .join(" ");
}

export function avatarGroupClassName(className?: string): string {
  return ["flex items-center gap-1", className].filter(Boolean).join(" ");
}

const AVATAR_BADGE_TONE_CLASSES: Record<AvatarBadgeTone, string> = {
  neutral: "bg-foreground-subtlest",
  success: "bg-success",
  warning: "bg-warning",
  destructive: "bg-destructive",
};

export function avatarBadgeClassName(
  tone: AvatarBadgeTone = "neutral",
  className?: string,
): string {
  return [
    "absolute -end-0.5 -bottom-0.5 size-2 rounded-full border border-background",
    AVATAR_BADGE_TONE_CLASSES[tone],
    className,
  ]
    .filter(Boolean)
    .join(" ");
}
