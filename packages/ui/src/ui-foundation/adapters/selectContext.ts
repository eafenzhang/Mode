// M4 adapter：Select 的共享状态（旧 Radix API 形状）。
import { createContext, useContext } from "react";
import type { MutableRefObject } from "react";

export interface SelectAdapterState {
  value: string | undefined;
  open: boolean;
  setOpen: (open: boolean) => void;
  selectValue: (value: string) => void;
  triggerRef: MutableRefObject<HTMLElement | null>;
  contentId: string;
  labels: Map<string, string>;
  registerLabel: (value: string, label: string) => () => void;
  labelFor: (value: string | undefined) => string | undefined;
  disabled: boolean;
}

export const SelectAdapterContext = createContext<SelectAdapterState | null>(null);

export function useSelectAdapterState(): SelectAdapterState | null {
  return useContext(SelectAdapterContext);
}
