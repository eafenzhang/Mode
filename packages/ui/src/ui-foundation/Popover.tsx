// P1.1 Popover primitive（spec: docs/specs/m4-ui-foundation.md §2.4）。
// 独立浮层从 rounded-xl 起；与 menu 的 shell 语言区分。

import { forwardRef, type HTMLAttributes } from "react";

import type { RadiusLevel } from "../lib/uiFoundation.js";
import { popoverClassName } from "./surfaceStyles.js";

export interface PopoverProps extends HTMLAttributes<HTMLDivElement> {
  parentRadius?: RadiusLevel;
}

export const Popover = forwardRef<HTMLDivElement, PopoverProps>(function Popover(
  { parentRadius, className, ...rest },
  ref,
) {
  return <div {...rest} ref={ref} className={popoverClassName(parentRadius, className)} />;
});
