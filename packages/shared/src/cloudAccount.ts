import { z } from "zod";

/**
 * Mode Cloud 个人账号 + 设备目录（docs/specs/cloud-personal-accounts.md，v2）。
 *
 * 身份方式：仅 GitHub OAuth Device Flow（云端验证 access token，JIT 建号）。
 * 同账号即信任：设备 enroll 时自报本机 API 令牌，目录把令牌下发给同账号主控；
 * 远程控制不再有 6 位配对码。密码/微信链路已删除。
 */

/** 控制面默认地址（与内置中继同域）。 */
export const DEFAULT_CLOUD_CONTROL_BASE_URL = "https://relay.mymode.cloud";
export const CLOUD_ENROLL_PATH = "/v1/devices/enroll";
export const CLOUD_DEVICES_PATH = "/v1/devices";
export const CLOUD_HEALTH_PATH = "/v1/health";

/** 个人账号控制面路径（仅 GitHub 登录与会话管理）。 */
export const CLOUD_AUTH_GITHUB_PATH = "/v1/auth/github";
export const CLOUD_AUTH_REFRESH_PATH = "/v1/auth/refresh";
export const CLOUD_AUTH_LOGOUT_PATH = "/v1/auth/logout";
export const CLOUD_AUTH_SESSIONS_PATH = "/v1/auth/sessions";

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
    /** 公网中继地址：https://<relay>/d/<tunnelId>；设备未上隧道时连接会 503。 */
    relayUrl: z.string().url(),
    /** 设备自报可直连地址（host:port）；LAN/Tailscale 直连优先于中继。 */
    addresses: z.array(z.string().trim().min(1).max(64)).max(8).optional(),
    /** 设备本机 API 令牌（云端解密后仅发给同账号）；缺失 = 设备需重新 enroll。 */
    accessToken: z.string().trim().min(1).max(256).optional(),
    /** 云端密钥丢失等导致令牌不可解密时的显式降级标注。 */
    tokenAvailable: z.boolean().optional(),
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
        displayName: z.string().trim().optional(),
      })
      .strict()
      .optional(),
    devices: z.array(cloudDeviceSummarySchema).optional(),
    error: z.string().optional(),
    /** 最近一次登录流程的失败原因（登录端自动轮询停止时展示）；成功后清除。 */
    loginError: z.string().optional(),
  })
  .strict();

export type CloudAccountState = z.infer<typeof cloudAccountStateSchema>;

/** GitHub Device Flow 挑战：显示 user code；verificationUri 由登录端自动打开。 */
export interface CloudGithubLoginChallenge {
  userCode: string;
  verificationUri: string;
  expiresAt: number;
}

/** GitHub 登录换会话的统一响应。 */
export interface CloudSessionAccount {
  id: string;
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

/** 设备 → 云端注册请求（会话鉴权；断言 payload 不含 account 块）。 */
export const cloudEnrollRequestSchema = z
  .object({
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
        /** 本机 API 令牌：同账号主控经目录读回，用于直连本设备。 */
        accessToken: z.string().trim().min(16).max(256),
        /** 自报可直连地址（host:port），≤8 个。 */
        addresses: z.array(z.string().trim().min(1).max(64)).max(8).optional(),
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
