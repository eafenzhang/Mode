// P1.1 ScrollArea（adapter map §1 缺口）。

import { forwardRef, type HTMLAttributes } from "react";

import {
  scrollAreaClassName,
  scrollAreaViewportClassName,
  scrollBarClassName,
  type ScrollOrientation,
} from "./scrollStyles.js";

export interface ScrollAreaProps extends HTMLAttributes<HTMLDivElement> {
  viewportClassName?: string;
}

export const ScrollArea = forwardRef<HTMLDivElement, ScrollAreaProps>(function ScrollArea(
  { className, viewportClassName, children, ...rest },
  ref,
) {
  return (
    <div {...rest} ref={ref} className={scrollAreaClassName(className)}>
      <div className={scrollAreaViewportClassName(viewportClassName)}>{children}</div>
    </div>
  );
});

export interface ScrollBarProps extends HTMLAttributes<HTMLDivElement> {
  orientation?: ScrollOrientation;
}

export const ScrollBar = forwardRef<HTMLDivElement, ScrollBarProps>(function ScrollBar(
  { orientation, className, ...rest },
  ref,
) {
  return (
    <div
      {...rest}
      ref={ref}
      aria-hidden="true"
      className={scrollBarClassName(orientation, className)}
    />
  );
});
