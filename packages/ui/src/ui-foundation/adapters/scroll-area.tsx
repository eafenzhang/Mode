// M4 adapter：ScrollArea / ScrollBar（旧导出名）。
// 旧调用方还传 `type`（滚动条可见性策略）与 `scrollbarClassName`；foundation 的静态
// 容器不实现该策略，先接受并静默忽略，登记为待接线差异（只影响滚动条可见性）。

import { forwardRef, type HTMLAttributes } from "react";

import { ScrollArea as FoundationScrollArea } from "../ScrollArea.js";

export { ScrollBar, type ScrollBarProps } from "../ScrollArea.js";

export type ScrollAreaVisibility = "auto" | "always" | "scroll" | "hover";

export interface ScrollAreaProps extends HTMLAttributes<HTMLDivElement> {
  viewportClassName?: string;
  type?: ScrollAreaVisibility;
  scrollbarClassName?: string;
}

export const ScrollArea = forwardRef<HTMLDivElement, ScrollAreaProps>(function ScrollArea(
  { type: _type, scrollbarClassName: _scrollbarClassName, ...rest },
  ref,
) {
  return <FoundationScrollArea {...rest} ref={ref} />;
});
