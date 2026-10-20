import { useCallback, useState } from "react";

export type DeviceViewMode = "list" | "grid";

const DEVICE_VIEW_STORAGE_KEY = "mode-remote-control-device-view";

function readDeviceViewMode(): DeviceViewMode {
  try {
    return window.localStorage.getItem(DEVICE_VIEW_STORAGE_KEY) === "grid" ? "grid" : "list";
  } catch {
    return "list";
  }
}

/** 远程控制设备列表的列表/宫格偏好；只影响本机 UI，不进入 Host 状态。 */
export function useDeviceViewMode() {
  const [viewMode, setViewModeState] = useState<DeviceViewMode>(readDeviceViewMode);
  const setViewMode = useCallback((mode: DeviceViewMode) => {
    setViewModeState(mode);
    try {
      window.localStorage.setItem(DEVICE_VIEW_STORAGE_KEY, mode);
    } catch {
      // 偏好写失败不影响切换本身。
    }
  }, []);
  return { viewMode, setViewMode };
}
