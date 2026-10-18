// P1.1 TooltipProvider（adapter map §5）：只提供延迟配置，不实现显示策略。

import { useMemo, type ReactNode } from "react";

import { resolveTooltipDelay, TooltipConfigContext } from "./tooltipContext.js";

export interface TooltipProviderProps {
  delayDuration?: number;
  children?: ReactNode;
}

export function TooltipProvider({ delayDuration, children }: TooltipProviderProps): ReactNode {
  const value = useMemo(
    () => ({ delayDuration: resolveTooltipDelay(delayDuration) }),
    [delayDuration],
  );
  return <TooltipConfigContext.Provider value={value}>{children}</TooltipConfigContext.Provider>;
}
