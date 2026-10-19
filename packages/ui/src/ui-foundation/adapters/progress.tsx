// M4 adapter：Progress（旧导出名；兼容 indicatorClassName / segments）。

import { forwardRef, type CSSProperties, type HTMLAttributes } from "react";

import {
  progressIndicatorClassName,
  progressPercent,
  progressTrackClassName,
} from "../formStyles.js";
import { cn } from "@/components/lib/utils.js";

export interface ProgressSegment {
  id: string;
  percent: number;
  style?: CSSProperties;
}

export interface ProgressProps extends HTMLAttributes<HTMLDivElement> {
  value: number;
  max?: number;
  label?: string;
  /** 旧 API：指示条额外类名。 */
  indicatorClassName?: string;
  /** 旧 API：分段进度（每段自带样式/百分比）。 */
  segments?: ProgressSegment[];
}

export const Progress = forwardRef<HTMLDivElement, ProgressProps>(function Progress(
  { value, max = 100, label, indicatorClassName, segments, className, ...rest },
  ref,
) {
  const percent = progressPercent(value, max);
  const hasSegments = (segments?.length ?? 0) > 0;
  return (
    <div
      {...rest}
      ref={ref}
      role="progressbar"
      aria-label={label}
      aria-valuenow={value}
      aria-valuemin={0}
      aria-valuemax={max}
      className={progressTrackClassName(cn([hasSegments ? "flex" : null, className]))}
    >
      {hasSegments ? (
        segments!.map((segment) => (
          <div
            key={segment.id}
            data-slot="progress-indicator"
            className={progressIndicatorClassName(indicatorClassName)}
            style={{ ...segment.style, width: `${progressPercent(segment.percent, 100)}%` }}
          />
        ))
      ) : (
        <div
          data-slot="progress-indicator"
          className={progressIndicatorClassName(indicatorClassName)}
          style={{ width: `${percent}%` }}
        />
      )}
    </div>
  );
});
