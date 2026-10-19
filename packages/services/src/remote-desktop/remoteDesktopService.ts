import type { Event } from "@mode/rpc";
import type {
  RemoteDesktopCodec,
  RemoteDesktopConsentRequest,
  RemoteDesktopConsentResponse,
  RemoteDesktopLocalDisplay,
  RemoteDesktopPeerCapability,
  RemoteDesktopQuality,
  RemoteDesktopServerState,
  RemoteDesktopSessionEvent,
  RemoteDesktopSignalMessage,
  RemoteDesktopStartSessionResult,
} from "@mode/shared/remote-desktop";
import { ServiceChannels } from "@mode/shared";

import { createServiceDescriptor } from "../descriptors.js";

/**
 * 远程桌面面（LR-2/LR-3，浏览器安全契约）。
 *
 * 状态所有者仍是 Host 进程里的 `RemoteDesktopHostServer` 与主控 controller：
 * - 总开关落凭据键 `remote-desktop:server-enabled`；
 * - 逐设备授权写既有 lan client 记录的 `remoteDesktopAuthorized` 字段；
 * - 编码能力缓存由真实会话观测写入；
 * - 主控会话的 WS/信令/媒体地址由 Host controller 持有（LR-3）。
 *
 * 本频道只注册给窗口 renderer（local-only）：授权列表与远程桌面管理面不对 LAN 对端开放。
 */

export interface RemoteDesktopSetServerEnabledParams {
  enabled: boolean;
}

export interface RemoteDesktopClientParams {
  clientId: string;
}

export interface RemoteDesktopPeerParams {
  serverId: string;
}

export interface RemoteDesktopStartSessionParams {
  serverId: string;
  quality?: RemoteDesktopQuality;
  codec?: RemoteDesktopCodec;
}

export interface RemoteDesktopSendSignalParams {
  message: RemoteDesktopSignalMessage;
}

export interface RemoteDesktopEndSessionParams {
  reason?: "user-end" | "error";
}

export interface RemoteDesktopControlledState {
  /** 本机处于被控中（含同意弹窗等待）；设备页据此整卡置灰。 */
  controlled: boolean;
}

export interface IRemoteDesktopService {
  /** 读被控设置面；读失败抛错，调用方自行按 fail-closed 展示。 */
  getServerState(): Promise<RemoteDesktopServerState>;
  /** 写总开关；关闭时立即终止进行中的被控会话。 */
  setServerEnabled(params: RemoteDesktopSetServerEnabledParams): Promise<RemoteDesktopServerState>;
  /** 授权设备（与同意窗「记住此设备」写同一字段）。 */
  authorizeClient(params: RemoteDesktopClientParams): Promise<RemoteDesktopServerState>;
  /** 撤销授权（下次连接重新弹同意窗）。 */
  revokeClient(params: RemoteDesktopClientParams): Promise<RemoteDesktopServerState>;
  /** 探测已配对对端的能力位与在线状态（失败归为 offline，不抛）。 */
  getPeerCapability(params: RemoteDesktopPeerParams): Promise<RemoteDesktopPeerCapability>;
  getControlledState(): Promise<RemoteDesktopControlledState>;
  /**
   * 被控会话状态变化（同意等待 / 已接管 / 已结束）：指示条据此显示/隐藏。
   * 幂等事实，首个订阅者补当前值。
   */
  onDidChangeControlledState: Event<RemoteDesktopControlledState>;
  /**
   * 发起主控会话：Host 负责对端票据、信令 WS 与媒体地址。
   * 本机被控中（含同意等待）时返回 `locally_controlled`。
   */
  startSession(params: RemoteDesktopStartSessionParams): Promise<RemoteDesktopStartSessionResult>;
  /** 写出信令帧；未连接时静默丢弃（与 Electron main 语义一致）。 */
  sendSignal(params: RemoteDesktopSendSignalParams): Promise<void>;
  /** 结束主控会话：先带出 bye，再收口并推 closed 终态。 */
  endSession(params?: RemoteDesktopEndSessionParams): Promise<void>;
  /** 回应被控同意请求（60s 未响应由 Host 按 timeout 拒绝）。 */
  respondConsent(params: RemoteDesktopConsentResponse): Promise<void>;
  /**
   * 被控端显示器快照：renderer 从壳读到后推给 Host（D7：Host 不直接枚举显示器）。
   * 会话建立时 Host 用最近一次快照回 `monitors` 并解析 `select-monitor`。
   */
  setLocalDisplays(params: { displays: RemoteDesktopLocalDisplay[] }): Promise<void>;
  /**
   * 非幂等主控会话事件（动态事件）：只发给当前订阅者，不缓冲、不重放旧信令。
   * `sessionId` 为将来的多会话预留；当前单会话实现忽略该参数。
   */
  onDynamicSessionEvent(sessionId?: string): Event<RemoteDesktopSessionEvent>;
  /** 被控同意请求（幂等事实）；60s 内没有 `respondConsent` 则按超时拒绝。 */
  onConsentRequest: Event<RemoteDesktopConsentRequest>;
  /** 幂等设置快照：set/authorize/revoke 成功后触发；首个订阅者拿到最近一次。 */
  onDidChangeServerState: Event<RemoteDesktopServerState>;
}

export const IRemoteDesktopService = createServiceDescriptor<IRemoteDesktopService>(
  ServiceChannels.RemoteDesktop,
);
