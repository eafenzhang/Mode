// P1.1 Tooltip primitive（spec: docs/specs/m4-ui-foundation.md §2.6）。

import { forwardRef, type HTMLAttributes, type ReactNode } from "react";

import { tooltipClassName, tooltipShortcutClassName } from "./feedbackStyles.js";

export interface TooltipProps extends HTMLAttributes<HTMLDivElement> {
  /** 键盘快捷键标签（text-ui-xs）。 */
  shortcut?: string;
  children?: ReactNode;
}

export const Tooltip = forwardRef<HTMLDivElement, TooltipProps>(function Tooltip(
  { shortcut, className, children, ...rest },
  ref,
) {
  return (
    <div {...rest} ref={ref} role="tooltip" className={tooltipClassName(className)}>
      {children}
      {shortcut ? (
        <kbd className={tooltipShortcutClassName()}>{shortcut}</kbd>
      ) : null}
    </div>
  );
});
