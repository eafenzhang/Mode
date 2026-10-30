import type { Event } from "@mode/rpc";
import type {
  CloudAccountState,
  CloudDeviceSummary,
  CloudGithubLoginChallenge,
} from "@mode/shared";
import { ServiceChannels } from "@mode/shared";

import { createServiceDescriptor } from "../descriptors.js";

/**
 * Mode Cloud 个人账号与设备目录管理面（local-only）。
 *
 * 状态所有者是 Host：凭据服务保存账号 ID / 会话 / 设备令牌 / 设备私钥；云目录只读
 * 缓存在内存。renderer 只读快照并下发登录/连接命令。
 *
 * **local-only**：该频道只注册给窗口 renderer，不得进入 LAN HTTP 服务集合
 * （目录里携带被控端访问令牌，等于云账号凭据）。
 */
export interface ICloudAccountService {
  /** 当前绑定状态 + 目录（不可用/未绑定/已绑定/错误）。 */
  getState(): Promise<CloudAccountState>;
  /**
   * GitHub Device Flow 登录：返回 user code 与 verificationUri；Host 会自动打开
   * 授权页并自动轮询（v2 交互），授权成功后状态自动变为 bound。
   */
  beginGithubLogin(): Promise<CloudGithubLoginChallenge>;
  /** 手动步进一次 Device Flow 轮询（自动轮询由 Host 驱动；测试/兜底用）。 */
  pollGithubLogin(): Promise<CloudAccountState>;
  /** 取消本机等待中的 GitHub 登录（停止自动轮询）。 */
  cancelGithubLogin(): void;
  /** 撤销云端会话并清除本机云凭据；不撤销云端设备（撤销走目录）。 */
  unbind(): Promise<CloudAccountState>;
  /** 强制刷新目录。 */
  listDevices(): Promise<CloudDeviceSummary[]>;
  /**
   * 目录直连（替代 6 位配对码）：读取目标设备的访问令牌与地址写入既有
   * lan:peer:* 凭据，成功后设备进入既有已配对列表（docs/specs/cloud-personal-accounts.md）。
   */
  connectDevice(deviceId: string): Promise<{ serverId: string; name: string }>;
  /** 幂等状态快照。 */
  onDidChange: Event<CloudAccountState>;
}

export const ICloudAccountService = createServiceDescriptor<ICloudAccountService>(
  ServiceChannels.CloudAccount,
);
