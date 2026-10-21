import type { Event } from "@mode/rpc";
import type {
  CloudAccountState,
  CloudBotCandidate,
  CloudDeviceSummary,
  CloudLoginChallenge,
  LanAccessPairResult,
} from "@mode/shared";
import { ServiceChannels } from "@mode/shared";

import { createServiceDescriptor } from "../descriptors.js";

/**
 * Mode Cloud 账号与设备目录管理面（local-only）。
 *
 * 状态所有者是 Host：凭据服务保存账号 ID / 设备令牌 / 设备私钥；云目录只读缓存
 * 在内存。renderer 只读快照并下发绑定/配对命令。
 *
 * **local-only**：该频道只注册给窗口 renderer，不得进入 LAN HTTP 服务集合
 * （设备令牌等于云账号凭据）。
 */
export interface ICloudAccountService {
  /** 当前绑定状态 + 目录（不可用/未绑定/已绑定/错误）。 */
  getState(): Promise<CloudAccountState>;
  /** 旧路径：用本机已绑定的微信 Bot + 部署 enrollment secret 注册。 */
  bind(): Promise<CloudAccountState>;
  /** 新路径：向云端 Bot 发起微信登录，返回桌面显示的 6 位码。 */
  beginWechatLogin(): Promise<CloudLoginChallenge>;
  /**
   * 轮询云端登录结果：pending = 等用户在微信里发码；bound = 成功并保存账号/令牌。
   * 成功后会启动中继隧道；云端不可达/码过期时抛出可读错误。
   */
  pollWechatLogin(): Promise<CloudAccountState>;
  /** 取消本机等待中的登录码（不通知云端）。 */
  cancelWechatLogin(): void;
  /** 本机可同步为云端登录 Bot 的微信 Bot（不含 token）。 */
  listCloudBotCandidates(): Promise<CloudBotCandidate[]>;
  /**
   * 把一个已绑定微信的 Bot 的 iLink token 上传到云端，并把本地副本停用，
   * 避免桌面 Bot 与云端 Bot 同时轮询同一条 token。
   */
  syncCloudLoginBot(params: { botId: string }): Promise<CloudAccountState>;
  /** 只清除本机云凭据；不撤销云端设备（撤销走目录/后续 UI）。 */
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
