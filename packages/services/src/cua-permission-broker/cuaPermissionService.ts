// Computer Use Helper macOS permission service — services-side descriptor registration.
//
// As part of the single-package merge the type definitions + functional helpers
// (CuaPermissionStatus, CuaPermissionStatusResult, isCuaPermissionStatusAvailable,
// shouldRunCuaScreenCaptureProbe, ICuaPermissionService interface, etc.) moved
// to @mode/cua/src/broker/ports.ts. The descriptor registration itself stays in
// services (host control plane — depends on services' createServiceDescriptor
// + @mode/shared ServiceChannels), so services internal callers (node.ts,
// accessor.ts, services/index.ts) and ui consumers (via @mode/services root
// export) keep importing `ICuaPermissionService` from this exact path.
//
// producer 只拥有 type contract；VALUE descriptor 继续由 services 的
// createServiceDescriptor 创建，避免 producer 反向依赖 RPC/service registry。

import { ServiceChannels } from "@mode/shared";

import { createServiceDescriptor } from "../descriptors.js";

// Type layer — type-only imports from the consolidated package (erased by TS
// at compile time; Vite never resolves @mode/cua for these).
import type {
  CuaPermissionState,
  CuaPermissionStatus,
  CuaPermissionStatusUnavailable,
  CuaPermissionStatusResult,
  CuaPermissionStatusQueryOptions,
  CuaPermissionRestartResult,
  CuaPermissionRestartOptions,
  ICuaPermissionService as BrokerICuaPermissionService,
} from "@mode/cua/broker";

// Re-export types for consumers.
export type {
  CuaPermissionState,
  CuaPermissionStatus,
  CuaPermissionStatusUnavailable,
  CuaPermissionStatusResult,
  CuaPermissionStatusQueryOptions,
  CuaPermissionRestartResult,
  CuaPermissionRestartOptions,
};

// 只从 producer 的纯 ports subpath 复用值谓词。这里不能从 Node-only broker barrel
// re-export，否则 renderer bundle 会引入 process/node:path/node:crypto；也不能再复制实现，
// 否则“省略 options 是否主动抓屏”这种隐私契约会再次漂移。
export {
  isCuaPermissionStatusAvailable,
  shouldRunCuaScreenCaptureProbe,
} from "@mode/cua/broker/ports";

export interface ICuaPermissionService extends BrokerICuaPermissionService {
  /**
   * 打开**本机** Computer Use 权限引导（动作落在该 service 所在 host 的主进程）。
   * LAN 远端面板经透传调用时，窗口开在对端机器上——不依赖 renderer preload，
   * 因此不能用 IPlatformService.openCuaPermissionOnboarding（那只会开本机的）。
   * docs/specs/lan-remote-computer-use.md 接口变更 4。
   */
  openPermissionOnboarding?(
    workspacePath?: string,
    workspaceIdentity?: string,
    options?: { initialPermission?: string; requiredPermissions?: string[] },
  ): Promise<{ success: boolean; error?: string }>;
}

export const ICuaPermissionService = createServiceDescriptor<
  BrokerICuaPermissionService & Pick<ICuaPermissionService, "openPermissionOnboarding">
>(ServiceChannels.CuaPermission);
