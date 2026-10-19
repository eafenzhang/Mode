import { cn } from "@/components/lib/utils.js";

// P1.7 quickpick 的类名契约（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.7）。

export function quickpickPanelClassName(className?: string): string {
  return cn([
    "w-80 overflow-hidden rounded-lg border border-popover-border bg-popover shadow-md",
    className,
  ]);
}

export function quickpickInputClassName(className?: string): string {
  return cn([
    "w-full border-b border-border bg-transparent px-3 py-2 text-ui-base text-foreground outline-none",
    "placeholder:text-foreground-subtlest focus:border-input-border-focused",
    className,
  ]);
}

export function quickpickListClassName(className?: string): string {
  return cn(["max-h-80 overflow-y-auto p-1", className]);
}

export interface QuickpickItemClassNameOptions {
  selected?: boolean;
  className?: string;
}

export function quickpickItemClassName(options: QuickpickItemClassNameOptions = {}): string {
  return cn([
    "flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-ui-sm text-foreground",
    "hover:bg-menu-hover focus:bg-menu-hover focus:outline-none",
    options.selected && "bg-selected",
    options.className,
  ]);
}

export function quickpickEmptyClassName(className?: string): string {
  return cn(["px-3 py-6 text-center text-ui-sm text-foreground-subtle", className]);
}
