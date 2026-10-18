// P1.1 基础件补漏：ScrollArea 的类名契约（adapter map §1 缺口）。
// 静态容器：滚动行为交给浏览器；滚动条视觉为装饰（thumb 由浏览器绘制）。

export type ScrollOrientation = "vertical" | "horizontal";

export function scrollAreaClassName(className?: string): string {
  return ["relative min-h-0 overflow-hidden", className].filter(Boolean).join(" ");
}

export function scrollAreaViewportClassName(className?: string): string {
  return ["size-full overflow-auto", className].filter(Boolean).join(" ");
}

export function scrollBarClassName(
  orientation: ScrollOrientation = "vertical",
  className?: string,
): string {
  const geometry =
    orientation === "vertical" ? "end-0 top-0 h-full w-2" : "bottom-0 start-0 h-2 w-full";
  return ["pointer-events-none absolute bg-transparent", geometry, className]
    .filter(Boolean)
    .join(" ");
}
