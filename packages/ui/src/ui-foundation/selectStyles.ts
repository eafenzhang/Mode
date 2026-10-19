// P1.1 composable 补漏：Select 内容区/选项/组/值的类名契约（adapter map §5）。

import { menuItemClassName } from "./surfaceStyles.js";
import { cn } from "@/components/lib/utils.js";

export function selectContentClassName(className?: string): string {
  return cn([
    "z-50 max-h-60 overflow-y-auto rounded-lg border border-popover-border bg-menu p-1 shadow-md",
    className,
  ]);
}

/** 选项沿用菜单行语言；选中优先 check 标记，不做整行强填充。 */
export function selectItemClassName(className?: string): string {
  return cn([menuItemClassName(), "aria-selected:font-medium", className]);
}

export function selectGroupLabelClassName(className?: string): string {
  return cn(["px-2 py-1 text-ui-xs text-foreground-subtlest", className]);
}

export interface SelectValueClassNameOptions {
  placeholder?: boolean;
  className?: string;
}

export function selectValueClassName(options: SelectValueClassNameOptions = {}): string {
  return cn([
    "min-w-0 truncate",
    options.placeholder ? "text-foreground-subtlest" : "text-foreground",
    options.className,
  ]);
}
