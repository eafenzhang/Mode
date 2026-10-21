import { z } from "zod";

/**
 * Mode Cloud 账号 + 设备目录（docs/specs/mode-cloud-account-directory.md）。
 *
 * 本期账号锚点只支持微信 Bot 绑定的用户 ID；云端只存 HMAC，不存原文。
 * 目录只提供设备公网地址与在线状态，配对仍由被控端 6 位码授权。
 */

/** 本期唯一账号锚点。 */
export const cloudAccountProviderSchema = z.literal("weixin");
export type CloudAccountProvider = z.infer<typeof cloudAccountProviderSchema>;

/** 控制面默认地址（与内置中继同域）。 */
export const DEFAULT_CLOUD_CONTROL_BASE_URL = "https://relay.mymode.cloud";
export const CLOUD_ENROLL_PATH = "/v1/devices/enroll";
export const CLOUD_DEVICES_PATH = "/v1/devices";
export const CLOUD_HEALTH_PATH = "/v1/health";

/** 本机凭据键：云账号 ID / 设备令牌 / 设备私钥。 */
export const CLOUD_ACCOUNT_ID_CREDENTIAL_KEY = "cloud:account:id";
export const CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY = "cloud:device:token";
export const CLOUD_DEVICE_PRIVATE_KEY_CREDENTIAL_KEY = "cloud:device:private-key";
/** 云登录成功后云端下发的共享中继令牌；Desktop 用它启动隧道。 */
export const CLOUD_RELAY_TOKEN_CREDENTIAL_KEY = "cloud:relay-token";

export const cloudDeviceSummarySchema = z
  .object({
    id: z.string().trim().min(1).max(128),
    name: z.string().trim().min(1).max(128),
    platform: z.string().trim().max(64),
    version: z.string().trim().max(64).optional(),
    online: z.boolean(),
    /** 被控端公网地址：https://<relay>/d/<tunnelId>。 */
    relayUrl: z.string().url(),
    lastSeenAt: z.number().int().nonnegative().nullable().optional(),
  })
  .strict();

export type CloudDeviceSummary = z.infer<typeof cloudDeviceSummarySchema>;

export const cloudAccountStateSchema = z
  .object({
    status: z.enum(["unavailable", "unbound", "pending", "bound", "error"]),
    account: z
      .object({
        id: z.string().trim().min(1),
        displayName: z.string().trim().optional(),
      })
      .strict()
      .optional(),
    devices: z.array(cloudDeviceSummarySchema).optional(),
    error: z.string().optional(),
    /** 本机是否配置了 enrollment secret；false 时 UI 显示“未配置”。 */
    enrollmentConfigured: z.boolean(),
    /** 本机微信 Bot 已绑定的展示名（即使尚未绑定云账号）。 */
    boundWeixinName: z.string().trim().optional(),
  })
  .strict();

export type CloudAccountState = z.infer<typeof cloudAccountStateSchema>;

export interface CloudPairDeviceParams {
  deviceId: string;
  code: string;
}

/** 微信登录挑战：桌面显示 6 位码，用户在微信里发给云端 Bot。 */
export interface CloudLoginChallenge {
  code: string;
  expiresAt: number;
}

/** 本机可同步为“云端登录 Bot”的微信 Bot（只暴露展示信息，不含 token）。 */
export interface CloudBotCandidate {
  id: string;
  name: string;
  boundUser?: string;
  enabled: boolean;
  hasToken: boolean;
}

/** 设备 → 云端注册请求（由被控端 Host 发起）。 */
export const cloudEnrollRequestSchema = z
  .object({
    account: z
      .object({
        provider: cloudAccountProviderSchema,
        providerUserId: z.string().trim().min(1).max(256),
        displayName: z.string().trim().max(64).optional(),
      })
      .strict(),
    device: z
      .object({
        id: z.string().trim().min(1).max(128),
        name: z.string().trim().min(1).max(64),
        platform: z.string().trim().min(1).max(32),
        version: z.string().trim().max(64).optional(),
        publicKey: z.string().trim().min(1).max(4096),
        relayTunnelId: z
          .string()
          .trim()
          .regex(/^[A-Za-z0-9_-]{16,128}$/u, "invalid relayTunnelId"),
      })
      .strict(),
    assertion: z
      .object({
        payload: z.string().min(1).max(8192),
        signature: z.string().min(1).max(4096),
      })
      .strict(),
    nonce: z.string().trim().min(8).max(128),
    issuedAt: z.number().int().positive(),
  })
  .strict();

export type CloudEnrollRequest = z.infer<typeof cloudEnrollRequestSchema>;

export interface CloudEnrollResponse {
  accountId: string;
  deviceId: string;
  /** 只在注册响应下发一次；Host 存加密凭据。 */
  deviceToken: string;
}
