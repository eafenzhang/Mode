// P1.1 基础件补漏：Badge 组件（adapter map §5）。

import { forwardRef, type HTMLAttributes } from "react";

import { badgeClassName, type BadgeTone } from "./badgeStyles.js";

export interface BadgeProps extends HTMLAttributes<HTMLSpanElement> {
  tone?: BadgeTone;
  pill?: boolean;
}

export const Badge = forwardRef<HTMLSpanElement, BadgeProps>(function Badge(
  { tone, pill, className, ...rest },
  ref,
) {
  return <span {...rest} ref={ref} className={badgeClassName({ tone, pill, className })} />;
});
