import type { Event } from "@mode/rpc";
import type {
  RemoteV3BeginLoginParams,
  RemoteV3ConfigureParams,
  RemoteV3DeviceSummary,
  RemoteV3GithubChallenge,
  RemoteV3HostState,
  RemoteV3HttpRequestParams,
  RemoteV3HttpResponse,
  RemoteV3RemoteDesktopSession,
  RemoteV3RemoteDesktopStartParams,
  RemoteV3RequestParams,
  RemoteV3RequestResult,
  RemoteV3ResetPinParams,
  RemoteV3TunnelEvent,
  RemoteV3TunnelOpenParams,
} from "@mode/shared";
import { ServiceChannels } from "@mode/shared";

import { createServiceDescriptor } from "../descriptors.js";

/**
 * 远程控制 v3 Host 管理面（local-only）。
 *
 * 状态所有者是 Host：凭据服务保存设备身份私钥、设备 token、服务地址与 peer pin；
 * `ControlLink` 持有当前 Control 连接与活动会话。renderer 只读快照并下发登录/解绑命令。
 *
 * 该频道不得进入 LAN 对端可见的服务集合：返回值包含设备身份与服务地址。
 */
export interface IRemoteV3Service {
  getState(): Promise<RemoteV3HostState>;
  /** 用已有 Web 会话注册本机 Host 设备并启动 Control 链路。 */
  configure(params: RemoteV3ConfigureParams): Promise<RemoteV3HostState>;
  /** 发起 GitHub Device Flow；Host 自动轮询并在授权后注册设备。 */
  beginGithubLogin(params?: RemoteV3BeginLoginParams): Promise<RemoteV3GithubChallenge>;
  /** 同账号异角色设备目录（只含 descriptor、在线状态与地址）。 */
  /** 本机 Client 角色可访问的同账号 Host 目录。 */
  listHosts(): Promise<RemoteV3DeviceSummary[]>;
  /** 发起 v3 远程桌面会话，返回本机回环信令/媒体代理地址。 */
  startRemoteDesktop(
    params: RemoteV3RemoteDesktopStartParams,
  ): Promise<RemoteV3RemoteDesktopSession>;
  /** 手动步进一次 Device Flow（自动轮询由 Host 驱动；测试/兜底用）。 */
  pollGithubLogin(): Promise<RemoteV3HostState>;
  cancelGithubLogin(): void;
  /** 停止 Control 链路并清除本机设备 token；保留设备身份与 peer pin。 */
  disconnectAccount(): Promise<RemoteV3HostState>;
  /** 移除某对端设备的本地公钥 pin；下次连接会重新固定。 */
  resetPeerPin(params: RemoteV3ResetPinParams): Promise<void>;
  /** 本机作为 Client 角色连接目标 Host 并执行一次 allowlist RPC。 */
  request(params: RemoteV3RequestParams): Promise<RemoteV3RequestResult>;
  /** 通过 E2EE 通道调用目标 Host 的本机 `/api/*`（自动注入 Host 本地令牌）。 */
  httpRequest(params: RemoteV3HttpRequestParams): Promise<RemoteV3HttpResponse>;
  /** 打开一条到目标 Host 本机 `/ws*` 的 E2EE WebSocket 隧道。 */
  openTunnel(params: RemoteV3TunnelOpenParams): Promise<{ tunnelId: string }>;
  /** 向隧道发送一帧（base64url，首字节 0=text、1=binary）。 */
  sendTunnel(tunnelId: string, data: string): Promise<void>;
  closeTunnel(tunnelId: string): Promise<void>;
  /** 隧道打开/消息/关闭事件；消息 data 为 base64url。 */
  onTunnelEvent: Event<RemoteV3TunnelEvent>;
  /** 幂等状态快照。 */
  onDidChange: Event<RemoteV3HostState>;
}

export const IRemoteV3Service = createServiceDescriptor<IRemoteV3Service>(ServiceChannels.RemoteV3);
