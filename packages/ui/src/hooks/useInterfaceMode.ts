import { useModeStoreWithDefault } from "@/store/StoreProvider.js";

export function useIsOfficeMode(): boolean {
  return useModeStoreWithDefault((state) => state.interfaceMode === "office", false);
}
