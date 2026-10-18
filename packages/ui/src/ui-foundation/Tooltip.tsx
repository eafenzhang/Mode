// P1.1 Tooltip primitive（spec: docs/specs/m4-ui-foundation.md §2.6）。

import {
  cloneElement,
  forwardRef,
  type HTMLAttributes,
  type ReactElement,
  type ReactNode,
} from "react";

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

/** 内容区沿用 Tooltip 的视觉契约。 */
export const TooltipContent = Tooltip;

export interface TooltipTriggerProps {
  children: ReactElement;
  /** 内容区 id，用于 aria-describedby。 */
  contentId?: string;
}

/**
 * 只做静态可访问性挂点：把 aria-describedby 与 data 标记克隆到子元素上，
 * 显示/隐藏与延迟由接线层实现（不在本组件里偷偷加事件）。
 */
export function TooltipTrigger({ children, contentId }: TooltipTriggerProps): ReactElement {
  return cloneElement(children as ReactElement<Record<string, unknown>>, {
    "aria-describedby": contentId,
    "data-tooltip-trigger": "true",
  });
}
