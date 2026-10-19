// P1.1 Spinner primitive（spec: docs/specs/m4-ui-foundation.md §2.7）。
// 颜色继承 currentColor，尺寸完全由调用方 className 控制。

import { forwardRef, type SVGProps } from "react";
import { cn } from "@/components/lib/utils.js";

export interface SpinnerProps extends SVGProps<SVGSVGElement> {
  /** 提供时作为 role=status 的可读文案；缺省视为纯装饰。 */
  label?: string;
}

export const Spinner = forwardRef<SVGSVGElement, SpinnerProps>(function Spinner(
  { label, className, ...rest },
  ref,
) {
  return (
    <svg
      {...rest}
      ref={ref}
      viewBox="0 0 16 16"
      fill="none"
      role={label ? "status" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      focusable="false"
      className={cn(["animate-spin", className])}
    >
      <circle cx="8" cy="8" r="6" stroke="currentColor" strokeOpacity="0.25" strokeWidth="2" />
      <path d="M14 8a6 6 0 0 0-6-6" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
    </svg>
  );
});
