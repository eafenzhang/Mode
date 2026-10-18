// Dialog adapter 的开关上下文（旧 API：Dialog open/onOpenChange + DialogContent 组合）。

import { createContext, useContext } from "react";

export interface DialogAdapterState {
  open: boolean;
  setOpen: (open: boolean) => void;
}

export const DialogAdapterContext = createContext<DialogAdapterState | null>(null);

export function useDialogAdapterState(): DialogAdapterState | null {
  return useContext(DialogAdapterContext);
}
