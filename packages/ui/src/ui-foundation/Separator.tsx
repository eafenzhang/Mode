// P1.1 基础件补漏：Separator 组件（adapter map §5）。

import { forwardRef, type HTMLAttributes } from "react";

import { separatorClassName, type SeparatorOrientation } from "./separatorStyles.js";

export interface SeparatorProps extends HTMLAttributes<HTMLDivElement> {
  orientation?: SeparatorOrientation;
  /** 纯装饰分隔线：不进入无障碍树。 */
  decorative?: boolean;
}

export const Separator = forwardRef<HTMLDivElement, SeparatorProps>(function Separator(
  { orientation = "horizontal", decorative, className, ...rest },
  ref,
) {
  return (
    <div
      {...rest}
      ref={ref}
      role={decorative ? "none" : "separator"}
      aria-orientation={decorative ? undefined : orientation}
      className={separatorClassName(orientation, className)}
    />
  );
});
