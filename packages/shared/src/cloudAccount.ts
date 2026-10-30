import { z } from "zod";

/**
 * Mode Cloud 个人账号 + 设备目录（docs/specs/cloud-personal-accounts.md）。
 *
 * 身份方式：GitHub OAuth（Device Flow，云端验证 access token）+ 用户名密码注册；
 * 微信登录已移除（2026-10-30）。目录只提供设备公网地址与在线状态，
 * 配对仍由被控端 6 位码授权。
 */

/** 控制面默认地址（与内置中继同域）。 */
export const DEFAULT_CLOUD_CONTROL_BASE_URL = "https://relay.mymode.cloud";
export const CLOUD_ENROLL_PATH = "/v1/devices/enroll";
export const CLOUD_DEVICES_PATH = "/v1/devices";
export const CLOUD_HEALTH_PATH = "/v1/health";

/** 个人账号控制面路径（GitHub 登录 / 密码注册登录 / 会话）。 */
export const CLOUD_AUTH_REGISTER_PATH = "/v1/auth/register";
export const CLOUD_AUTH_LOGIN_PATH = "/v1/auth/login";
export const CLOUD_AUTH_GITHUB_PATH = "/v1/auth/github";
export const CLOUD_AUTH_REFRESH_PATH = "/v1/auth/refresh";
export const CLOUD_AUTH_LOGOUT_PATH = "/v1/auth/logout";
export const CLOUD_AUTH_SESSIONS_PATH = "/v1/auth/sessions";
export const CLOUD_AUTH_CREDENTIALS_PATH = "/v1/auth/credentials";

/** 本机凭据键：云账号 ID / 人类会话 / 设备令牌 / 设备私钥 / 每设备中继令牌。 */
export const CLOUD_ACCOUNT_ID_CREDENTIAL_KEY = "cloud:account:id";
export const CLOUD_SESSION_TOKEN_CREDENTIAL_KEY = "cloud:session:token";
export const CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY = "cloud:device:token";
export const CLOUD_DEVICE_PRIVATE_KEY_CREDENTIAL_KEY = "cloud:device:private-key";
/** enroll/登录成功后云端下发的每设备中继令牌；Desktop 用它启动隧道。 */
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
    status: z.enum(["unavailable", "unbound", "bound", "error"]),
    account: z
      .object({
        id: z.string().trim().min(1),
        /** 已补设的用户名；缺失时 UI 提供「设置用户名密码」。 */
        username: z.string().trim().min(1).optional(),
        displayName: z.string().trim().optional(),
      })
      .strict()
      .optional(),
    devices: z.array(cloudDeviceSummarySchema).optional(),
    error: z.string().optional(),
  })
  .strict();

export type CloudAccountState = z.infer<typeof cloudAccountStateSchema>;

export interface CloudPairDeviceParams {
  deviceId: string;
  code: string;
}

/** GitHub Device Flow 挑战：桌面显示 user code 并打开 verificationUri。 */
export interface CloudGithubLoginChallenge {
  userCode: string;
  verificationUri: string;
  expiresAt: number;
}

/** 密码注册/登录请求；注册成功即登录。 */
export interface CloudPasswordAuthRequest {
  username: string;
  password: string;
  displayName?: string;
  inviteCode?: string;
}

/** 登录/注册/GitHub 换取会话的统一响应。 */
export interface CloudSessionAccount {
  id: string;
  username?: string;
  displayName?: string;
}

export interface CloudSessionResponse {
  sessionToken: string;
  expiresAt: number;
  account: CloudSessionAccount;
}

/** 会话列表条目（GET /v1/auth/sessions）。 */
export interface CloudSessionInfo {
  id: string;
  clientLabel?: string;
  createdAt: number;
  lastUsedAt?: number | null;
  expiresAt: number;
}

/** 设备 → 云端注册请求（由被控端 Host 发起；会话路径不带 account 块）。 */
export const cloudEnrollRequestSchema = z
  .object({
    account: z
      .object({
        provider: z.enum(["weixin"]),
        providerUserId: z.string().trim().min(1).max(256),
        displayName: z.string().trim().max(64).optional(),
      })
      .strict()
      .optional(),
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
  /** 每设备中继令牌；同样只在响应下发一次（docs/specs/cloud-personal-accounts.md）。 */
  relayToken: string;
}
