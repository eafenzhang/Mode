import { cn } from "@/components/lib/utils.js";

// P1.5 设置页与插件商店的类名契约
// （plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.5；
//  卡片/详情的不可安装口径见 docs/specs/plugin-store-availability.md）。

export function settingsLayoutClassName(className?: string): string {
  return cn(["flex min-h-0 flex-1", className]);
}

export function settingsNavClassName(className?: string): string {
  return cn([
    "flex w-56 shrink-0 flex-col gap-0.5 overflow-y-auto border-e border-border p-2",
    className,
  ]);
}

export interface SettingsNavItemClassNameOptions {
  selected?: boolean;
  className?: string;
}

export function settingsNavItemClassName(options: SettingsNavItemClassNameOptions = {}): string {
  return cn([
    "flex h-8 items-center gap-2 rounded-md px-2 text-ui-sm text-foreground",
    "hover:bg-hover focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary",
    options.selected && "bg-selected",
    options.className,
  ]);
}

export function settingsContentClassName(className?: string): string {
  return cn(["min-w-0 flex-1 overflow-y-auto p-4", className]);
}

export function settingsSectionClassName(className?: string): string {
  return cn(["flex flex-col gap-4", className]);
}

export function settingsSectionTitleClassName(className?: string): string {
  return cn(["text-ui-base font-medium text-foreground", className]);
}

export function settingsSectionDescriptionClassName(className?: string): string {
  return cn(["text-ui-sm text-foreground-subtle", className]);
}

export type PluginAvailability = "bundledUnavailable" | "sourceUnavailable";

/**
 * 插件卡片：两类不可安装状态只改变 badge/安装入口，不隐藏卡片
 * （plugin-store-availability 的卡片规则）。
 */
export function pluginCardClassName(className?: string): string {
  return cn(["flex flex-col gap-3 rounded-xl border border-card-border bg-card p-4", className]);
}

export function pluginCardHeaderClassName(className?: string): string {
  return cn(["flex min-w-0 items-start gap-3", className]);
}

/** 品牌图标背板：DESIGN.md 允许的 rounded-2xl 例外。 */
export function pluginIconClassName(className?: string): string {
  return cn(["size-10 shrink-0 rounded-2xl bg-surface", className]);
}

export function pluginCardBodyClassName(className?: string): string {
  return cn(["min-w-0 flex-1", className]);
}

export function pluginCardTitleClassName(className?: string): string {
  return cn(["text-ui-base font-medium text-foreground", className]);
}

export function pluginCardDescriptionClassName(className?: string): string {
  return cn(["line-clamp-2 text-ui-sm text-foreground-subtle", className]);
}

export function pluginCardActionsClassName(className?: string): string {
  return cn(["flex shrink-0 items-center gap-2", className]);
}

/** 不可安装 pill：刻意使用 pill 形状（rounded-full 的白名单用途）。 */
export function pluginAvailabilityBadgeClassName(className?: string): string {
  return cn([
    "inline-flex shrink-0 items-center rounded-full border border-border px-2 py-0.5 text-ui-xs text-foreground-subtle",
    className,
  ]);
}

/** 详情页固定说明（无重试按钮）：surface 容器 + 次要文本。 */
export function pluginDetailHintClassName(className?: string): string {
  return cn([
    "rounded-xl border border-border bg-surface p-3 text-ui-sm text-foreground-subtle",
    className,
  ]);
}
