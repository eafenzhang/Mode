// M4 adapter：Tooltip 的共享状态（旧 Radix API 形状）。
import { createContext, useContext } from "react";
import type { MutableRefObject } from "react";

export interface TooltipAdapterState {
  open: boolean;
  setOpen: (open: boolean) => void;
  scheduleOpen: (delayMs?: number) => void;
  cancelPending: () => void;
  triggerRef: MutableRefObject<HTMLElement | null>;
  contentId: string;
  delayDuration: number;
}

export const TooltipAdapterContext = createContext<TooltipAdapterState | null>(null);

export function useTooltipAdapterState(): TooltipAdapterState | null {
  return useContext(TooltipAdapterContext);
}

export interface TooltipProviderState {
  delayDuration: number;
  skipDelayDuration: number;
}

export const TooltipProviderContext = createContext<TooltipProviderState | null>(null);

export function useTooltipProviderState(): TooltipProviderState | null {
  return useContext(TooltipProviderContext);
}
