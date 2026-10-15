/**
 * 远程桌面平台面派生类型。
 *
 * 形状真源只有一处：`@mode/shared` 的 `IPlatformService` 里的 `rd*` 声明（对应 desktop 侧
 * 冻结的 `remoteDesktop/ipcChannels.ts`）。shared 的包入口只按白名单再导出，因此这里用
 * 派生类型引用它，避免在 ui 侧维护第二份手写形状导致漂移。
 */
import type { IPlatformService } from "@mode/shared";

/** `rdGetPeerCapability` 结果：能力位 + 在线状态（+ 对端自报名）。 */
export type RemoteDesktopPeerCapability = Awaited<
  ReturnType<IPlatformService["rdGetPeerCapability"]>
>;

/** 被控设置面状态（总开关 + 已授权设备）。 */
export type RemoteDesktopServerState = Awaited<ReturnType<IPlatformService["rdGetServerState"]>>;

/** 主控会话事件（state / signal），与 main 的 `rd/session-event` 推送一致。 */
export type RemoteDesktopControllerEvent = Parameters<
  Parameters<IPlatformService["onRemoteDesktopSessionEvent"]>[0]
>[0];
