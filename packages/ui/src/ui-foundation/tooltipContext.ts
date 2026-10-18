// P1.1 Tooltip 配置上下文（adapter map §5）。
// 显示/隐藏策略留给接线层；这里只约定延迟与可访问性挂点。

import { createContext, useContext } from "react";

export const DEFAULT_TOOLTIP_DELAY_MS = 500;
export const MAX_TOOLTIP_DELAY_MS = 2000;

export interface TooltipConfig {
  /** 触发到显示的延迟（毫秒）。 */
  delayDuration: number;
}

export function resolveTooltipDelay(value?: number): number {
  if (value === undefined || !Number.isFinite(value)) {
    return DEFAULT_TOOLTIP_DELAY_MS;
  }
  return Math.min(MAX_TOOLTIP_DELAY_MS, Math.max(0, Math.round(value)));
}

export const TooltipConfigContext = createContext<TooltipConfig>({
  delayDuration: DEFAULT_TOOLTIP_DELAY_MS,
});

export function useTooltipConfig(): TooltipConfig {
  return useContext(TooltipConfigContext);
}
