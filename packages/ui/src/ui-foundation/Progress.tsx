// P1.1 Progress（adapter map §1 缺口）。

import { forwardRef, type HTMLAttributes } from "react";

import {
  progressIndicatorClassName,
  progressPercent,
  progressTrackClassName,
} from "./formStyles.js";

export interface ProgressProps extends HTMLAttributes<HTMLDivElement> {
  value: number;
  max?: number;
  /** 可访问名称（i18n）。 */
  label?: string;
}

export const Progress = forwardRef<HTMLDivElement, ProgressProps>(function Progress(
  { value, max = 100, label, className, ...rest },
  ref,
) {
  const percent = progressPercent(value, max);
  return (
    <div
      {...rest}
      ref={ref}
      role="progressbar"
      aria-label={label}
      aria-valuenow={value}
      aria-valuemin={0}
      aria-valuemax={max}
      className={progressTrackClassName(className)}
    >
      <div className={progressIndicatorClassName()} style={{ width: `${percent}%` }} />
    </div>
  );
});
