/**
 * 远程桌面（系统 RDP）平台面派生类型。
 *
 * 形状真源只有一处：`@mode/shared` 的 `IPlatformService` 里的 `rd*` 声明（对应 desktop 侧
 * 冻结的 `remoteDesktop/ipcChannels.ts`）。shared 的包入口只按白名单再导出，因此这里用
 * 派生类型引用它，避免在 ui 侧维护第二份手写形状导致漂移。
 */
import type { IPlatformService } from "@mode/shared";

/** `rdGetPeerCapability` 结果：能力位 + 系统 RDP 状态 + 在线状态（+ 对端自报名）。 */
export type RemoteDesktopPeerCapability = Awaited<
  ReturnType<IPlatformService["rdGetPeerCapability"]>
>;

/** 被控设置面状态（提示性共享开关 + 系统 RDP 真实状态）。 */
export type RemoteDesktopServerState = Awaited<ReturnType<IPlatformService["rdGetServerState"]>>;

/** 每设备连接选项（主控端偏好，按 serverId 记忆）。 */
export type RemoteDesktopConnectionOptions = Awaited<
  ReturnType<IPlatformService["rdGetRdpOptions"]>
>;

/** 一条已启动的 RDP 会话记录（应用内跟踪视图）。 */
export type RemoteDesktopSessionRecord = Awaited<
  ReturnType<IPlatformService["rdGetSessions"]>
>[number];

/** 连接失败原因（稳定值，UI 据此出文案）。 */
export type RemoteDesktopConnectFailure = Extract<
  Awaited<ReturnType<IPlatformService["rdConnect"]>>,
  { ok: false }
>["reason"];
