import { cn } from "@/components/lib/utils.js";

// P1.1 基础件补漏：Avatar / AvatarGroup 的类名契约（adapter map §1 缺口）。

export type AvatarSize = "sm" | "default" | "lg";
export type AvatarBadgeTone = "neutral" | "success" | "warning" | "destructive";

const AVATAR_SIZE_CLASSES: Record<AvatarSize, string> = {
  sm: "size-6 text-ui-xs",
  default: "size-8 text-ui-xs",
  lg: "size-10 text-ui-sm",
};

export function avatarClassName(size: AvatarSize = "default", className?: string): string {
  return cn([
    "relative inline-flex shrink-0 items-center justify-center overflow-hidden rounded-full bg-surface font-medium text-foreground-subtle",
    AVATAR_SIZE_CLASSES[size],
    className,
  ]);
}

export function avatarImageClassName(className?: string): string {
  return cn(["size-full object-cover", className]);
}

export function avatarFallbackClassName(className?: string): string {
  return cn(["flex size-full items-center justify-center bg-surface", className]);
}

export function avatarGroupClassName(className?: string): string {
  return cn(["flex items-center gap-1", className]);
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
  return cn([
    "absolute -end-0.5 -bottom-0.5 size-2 rounded-full border border-background",
    AVATAR_BADGE_TONE_CLASSES[tone],
    className,
  ]);
}
