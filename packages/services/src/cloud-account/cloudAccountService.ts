import type { Event } from "@mode/rpc";
import type {
  CloudAccountState,
  CloudDeviceSummary,
  CloudGithubLoginChallenge,
  LanAccessPairResult,
} from "@mode/shared";
import { ServiceChannels } from "@mode/shared";

import { createServiceDescriptor } from "../descriptors.js";

/**
 * Mode Cloud 个人账号与设备目录管理面（local-only）。
 *
 * 状态所有者是 Host：凭据服务保存账号 ID / 会话 / 设备令牌 / 设备私钥；云目录只读
 * 缓存在内存。renderer 只读快照并下发登录/绑定/配对命令。
 *
 * **local-only**：该频道只注册给窗口 renderer，不得进入 LAN HTTP 服务集合
 * （设备令牌等于云账号凭据）。
 */
export interface ICloudAccountService {
  /** 当前绑定状态 + 目录（不可用/未绑定/已绑定/错误）。 */
  getState(): Promise<CloudAccountState>;
  /** GitHub Device Flow 登录：返回 user code 与 verificationUri，用户在浏览器授权。 */
  beginGithubLogin(): Promise<CloudGithubLoginChallenge>;
  /**
   * 轮询 GitHub Device Flow：pending 时返回未绑定状态；授权成功后换取云会话、
   * 注册本机设备并启动中继隧道，返回 bound 状态。
   */
  pollGithubLogin(): Promise<CloudAccountState>;
  /** 取消本机等待中的 GitHub 登录。 */
  cancelGithubLogin(): void;
  /** 用户名密码注册（注册即登录并注册本机设备）。 */
  registerWithPassword(params: {
    username: string;
    password: string;
    displayName?: string;
    inviteCode?: string;
  }): Promise<CloudAccountState>;
  /** 用户名密码登录（登录后注册本机设备）。 */
  loginWithPassword(params: { username: string; password: string }): Promise<CloudAccountState>;
  /** 给当前账号补设用户名密码（GitHub/迁移账号的备用登录方式）。 */
  setAccountCredentials(params: { username: string; password: string }): Promise<CloudAccountState>;
  /** 撤销云端会话并清除本机云凭据；不撤销云端设备（撤销走目录）。 */
  unbind(): Promise<CloudAccountState>;
  /** 强制刷新目录。 */
  listDevices(): Promise<CloudDeviceSummary[]>;
  /**
   * 从目录取目标设备的公网地址，复用现有 6 位码配对。
   * 成功后设备进入既有 LAN 配对列表。
   */
  pairDevice(params: { deviceId: string; code: string }): Promise<LanAccessPairResult>;
  /** 幂等状态快照。 */
  onDidChange: Event<CloudAccountState>;
}

export const ICloudAccountService = createServiceDescriptor<ICloudAccountService>(
  ServiceChannels.CloudAccount,
);
