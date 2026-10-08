import type { ForceUpdateRequirement, Locale } from "@mode/shared";
import type { ForceAutoUpdateState } from "./autoUpdater.js";

export interface ForceUpdateDialogText {
  title: string;
  message: string;
  detail: string;
  autoUpdateButton: string;
  manualUpdateButton: string;
  quitButton: string;
}

export interface ForceUpdateGuardLogger {
  info: (...args: unknown[]) => void;
  warn: (...args: unknown[]) => void;
}

interface ForceUpdateGuardResult {
  blocked: boolean;
  requirement?: ForceUpdateRequirement;
}

interface ForceUpdateGuardOptions {
  locale: Locale;
  logger: ForceUpdateGuardLogger;
  endpointOrigin?: string;
  fetchRemoteConfig?: () => Promise<unknown>;
  requestAutoUpdate?: (
    onStateChange?: (state: ForceAutoUpdateState) => void,
  ) => (() => void) | void;
  onBlocked?: (requirement: ForceUpdateRequirement) => void;
}

export async function maybeBlockStartupForForceUpdate(
  _options: ForceUpdateGuardOptions,
): Promise<ForceUpdateGuardResult> {
  // Mode 审计版不连接官方配置服务，也不执行注入的远端检查或强更回调。
  // 保留签名及启动调用点，让主窗口始终可以继续启动。
  return { blocked: false };
}
