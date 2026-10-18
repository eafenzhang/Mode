// Tabs adapter 的选中上下文（旧 API：value/defaultValue/onValueChange）。

import { createContext, useContext } from "react";

export interface TabsAdapterState {
  value: string | undefined;
  setValue: (value: string) => void;
}

export const TabsAdapterContext = createContext<TabsAdapterState | null>(null);

export function useTabsAdapterState(): TabsAdapterState | null {
  return useContext(TabsAdapterContext);
}
