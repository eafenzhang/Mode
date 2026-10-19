import { cn } from "@/components/lib/utils.js";

// P1.1 基础件补漏：Separator 的类名契约（adapter map §5）。

export type SeparatorOrientation = "horizontal" | "vertical";

export function separatorClassName(
  orientation: SeparatorOrientation = "horizontal",
  className?: string,
): string {
  const sizing = orientation === "horizontal" ? "h-px w-full" : "w-px self-stretch";
  return cn(["shrink-0 bg-border", sizing, className]);
}
