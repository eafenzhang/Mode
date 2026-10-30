/**
 * Mode Cloud GitHub Device Flow 登录 + 会话路径设备 enroll
 * （docs/specs/cloud-personal-accounts.md，v2：仅 GitHub 登录）。
 *
 * begin：向 GitHub 申请 device code（自动打开授权页由 Host 层回调），返回 user_code；
 * poll：轮询 GitHub token 端点（authorization_pending 继续、slow_down 退避），
 * 拿到 access token 后调 /v1/auth/github 换云会话，再带本机 accessToken enroll。
 */
import { hostname } from "node:os";

import {
  CLOUD_ACCOUNT_ID_CREDENTIAL_KEY,
  CLOUD_AUTH_GITHUB_PATH,
  CLOUD_AUTH_LOGOUT_PATH,
  CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY,
  CLOUD_ENROLL_PATH,
  CLOUD_RELAY_TOKEN_CREDENTIAL_KEY,
  CLOUD_SESSION_TOKEN_CREDENTIAL_KEY,
  MODE_VERSION,
  type CloudAccountState,
} from "@mode/shared";
import type { ICredentialService } from "@mode/services";

import {
  canonicalJson,
  createEnrollmentNonce,
  loadOrCreateCloudDeviceIdentity,
  signCloudEnrollment,
} from "./cloudDeviceIdentity.js";
import { fetchWithTimeout } from "./lanPeerTransport.js";

const AUTH_TIMEOUT_MS = 15_000;

export interface CloudAuthFlowOptions {
  credentials: ICredentialService;
  baseUrl: string;
  /** enroll 成功后启动中继隧道；幂等。 */
  ensureRelayTunnel: () => Promise<void>;
  readState: () => Promise<CloudAccountState>;
  onState: (state: CloudAccountState) => void;
  log: (line: string) => void;
}

export class CloudAuthApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }
}

function clientLabel(): string {
  return `desktop-${process.platform}`;
}

async function readErrorMessage(response: Response, fallback: string): Promise<string> {
  try {
    const body = (await response.json()) as { error?: unknown };
    if (typeof body.error === "string" && body.error) {
      return `${fallback}：${body.error}`;
    }
  } catch {
    // 保留 HTTP 状态错误。
  }
  return fallback;
}

async function postCloudJson(
  options: CloudAuthFlowOptions,
  path: string,
  body: unknown,
  token?: string,
): Promise<Record<string, unknown>> {
  const response = await fetchWithTimeout(
    `${options.baseUrl}${path}`,
    {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-mode-client": clientLabel(),
        ...(token ? { authorization: `Bearer ${token}` } : {}),
      },
      body: JSON.stringify(body),
    },
    AUTH_TIMEOUT_MS,
  );
  if (!response.ok) {
    throw new CloudAuthApiError(
      await readErrorMessage(response, `Mode Cloud 请求失败（HTTP ${response.status}）`),
      response.status,
    );
  }
  const payload = (await response.json()) as unknown;
  if (!payload || typeof payload !== "object") {
    throw new CloudAuthApiError("Mode Cloud 响应格式不正确", 0);
  }
  return payload as Record<string, unknown>;
}

/**
 * 会话路径 enroll：Bearer sessionToken + 无 account 块的 Ed25519 断言。
 * 请求携带本机 API 令牌（同账号主控经目录读回直连）与自报地址。
 * 成功保存 accountId / deviceToken / relayToken 并启动隧道。
 */
export async function enrollDeviceWithSession(
  options: CloudAuthFlowOptions,
  sessionToken: string,
  deviceIdentity: {
    accessToken: string;
    addresses?: string[];
  },
): Promise<void> {
  const identity = await loadOrCreateCloudDeviceIdentity(options.credentials);
  const issuedAt = Date.now();
  const nonce = createEnrollmentNonce();
  const payload = canonicalJson({
    device: { id: identity.deviceId, relayTunnelId: identity.relayTunnelId },
    nonce,
    issuedAt,
  });
  const body = await postCloudJson(
    options,
    CLOUD_ENROLL_PATH,
    {
      device: {
        id: identity.deviceId,
        name: hostname() || "Mode",
        platform: process.platform,
        version: MODE_VERSION,
        publicKey: identity.publicKeyPem,
        relayTunnelId: identity.relayTunnelId,
        accessToken: deviceIdentity.accessToken,
        ...(deviceIdentity.addresses && deviceIdentity.addresses.length > 0
          ? { addresses: deviceIdentity.addresses }
          : {}),
      },
      assertion: {
        payload,
        signature: signCloudEnrollment(identity.privateKeyPem, payload),
      },
      nonce,
      issuedAt,
    },
    sessionToken,
  );
  const accountId = typeof body.accountId === "string" ? body.accountId : "";
  const deviceId = typeof body.deviceId === "string" ? body.deviceId : "";
  const deviceToken = typeof body.deviceToken === "string" ? body.deviceToken : "";
  const relayToken = typeof body.relayToken === "string" ? body.relayToken : "";
  if (!accountId || !deviceId || !deviceToken || !relayToken) {
    throw new Error("设备注册响应缺少账号、设备或中继令牌");
  }
  await options.credentials.save(CLOUD_ACCOUNT_ID_CREDENTIAL_KEY, accountId);
  await options.credentials.save(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY, deviceToken);
  await options.credentials.save(CLOUD_RELAY_TOKEN_CREDENTIAL_KEY, relayToken);
  await options.ensureRelayTunnel().catch(() => undefined);
  options.log(`cloud_device_enroll device=${deviceId.slice(0, 8)}…`);
}

/** 登录成功后的共同收尾：存会话 → enroll → 通知状态。 */
export async function finishCloudLogin(
  options: CloudAuthFlowOptions,
  sessionToken: string,
  deviceIdentity: {
    accessToken: string;
    addresses?: string[];
  },
): Promise<CloudAccountState> {
  await options.credentials.save(CLOUD_SESSION_TOKEN_CREDENTIAL_KEY, sessionToken);
  await enrollDeviceWithSession(options, sessionToken, deviceIdentity);
  const state = await options.readState();
  options.onState(state);
  return state;
}

const GITHUB_DEVICE_CODE_URL = "https://github.com/login/device/code";
const GITHUB_TOKEN_URL = "https://github.com/login/oauth/access_token";
const GITHUB_TIMEOUT_MS = 15_000;
const DEFAULT_POLL_INTERVAL_MS = 5_000;
/** slow_down 响应要求的额外间隔。 */
const SLOW_DOWN_PENALTY_MS = 5_000;

interface DeviceCodeResponse {
  deviceCode: string;
  userCode: string;
  verificationUri: string;
  expiresAt: number;
  intervalMs: number;
}

interface PendingGithubLogin extends DeviceCodeResponse {
  /** slow_down 后递增的轮询间隔。 */
  currentIntervalMs: number;
}

export interface CloudGithubLoginOptions extends CloudAuthFlowOptions {
  /** GitHub OAuth App 的 public client_id（需在 GitHub 上启用 Device Flow）。 */
  githubClientId: string;
  /** 默认轮询间隔（毫秒）；GitHub interval=0 时使用，测试可注入更短间隔。 */
  loginPollIntervalMs?: number;
}

export function createCloudGithubLogin(options: CloudGithubLoginOptions) {
  let pending: PendingGithubLogin | null = null;
  const defaultIntervalMs = options.loginPollIntervalMs ?? DEFAULT_POLL_INTERVAL_MS;

  return {
    /** 当前应使用的轮询间隔（GitHub interval / slow_down 退避后）。 */
    currentIntervalMs(): number {
      return pending?.currentIntervalMs ?? defaultIntervalMs;
    },

    async begin(): Promise<{ userCode: string; verificationUri: string; expiresAt: number }> {
      if (!options.githubClientId) {
        throw new Error(
          "未配置 GitHub OAuth Client ID（MODE_CLOUD_GITHUB_CLIENT_ID 或部署配置 githubClientId）",
        );
      }
      const response = await fetchWithTimeout(
        GITHUB_DEVICE_CODE_URL,
        {
          method: "POST",
          headers: {
            "content-type": "application/x-www-form-urlencoded",
            accept: "application/json",
          },
          body: new URLSearchParams({
            client_id: options.githubClientId,
            scope: "read:user",
          }).toString(),
        },
        GITHUB_TIMEOUT_MS,
      );
      const body = (await response.json().catch(() => null)) as Record<string, unknown> | null;
      if (
        !body ||
        typeof body.device_code !== "string" ||
        !body.device_code ||
        typeof body.user_code !== "string" ||
        !body.user_code ||
        typeof body.verification_uri !== "string" ||
        !body.verification_uri
      ) {
        throw new Error(
          typeof body?.error_description === "string" && body.error_description
            ? `GitHub Device Flow 失败：${body.error_description}`
            : "GitHub Device Flow 响应格式不正确",
        );
      }
      const intervalRaw = typeof body.interval === "number" ? body.interval : 0;
      const expiresRaw = typeof body.expires_in === "number" ? body.expires_in : 600;
      pending = {
        deviceCode: body.device_code,
        userCode: body.user_code,
        verificationUri: body.verification_uri,
        expiresAt: Date.now() + expiresRaw * 1000,
        intervalMs: intervalRaw > 0 ? intervalRaw * 1000 : defaultIntervalMs,
        currentIntervalMs: intervalRaw > 0 ? intervalRaw * 1000 : defaultIntervalMs,
      };
      return {
        userCode: pending.userCode,
        verificationUri: pending.verificationUri,
        expiresAt: pending.expiresAt,
      };
    },

    /**
     * 单次轮询：pending 时返回未绑定状态（登录端继续等）；成功时完成会话交换 + enroll，
     * 返回 bound 状态；硬错误抛出可读消息。
     */
    async poll(deviceIdentity: () => Promise<{
      accessToken: string;
      addresses?: string[];
    }>): Promise<CloudAccountState> {
      if (!pending) {
        throw new Error("没有等待中的 GitHub 登录，请重新发起");
      }
      if (pending.expiresAt <= Date.now()) {
        pending = null;
        throw new Error("GitHub 授权已过期，请重新发起登录");
      }
      const response = await fetchWithTimeout(
        GITHUB_TOKEN_URL,
        {
          method: "POST",
          headers: {
            "content-type": "application/x-www-form-urlencoded",
            accept: "application/json",
          },
          body: new URLSearchParams({
            client_id: options.githubClientId,
            device_code: pending.deviceCode,
            grant_type: "urn:ietf:params:oauth:grant-type:device_code",
          }).toString(),
        },
        GITHUB_TIMEOUT_MS,
      );
      const body = (await response.json().catch(() => null)) as Record<string, unknown> | null;
      if (!body) {
        throw new Error(`GitHub 请求失败（HTTP ${response.status}）`);
      }
      if (typeof body.access_token === "string" && body.access_token) {
        const accessToken = body.access_token;
        pending = null;
        const exchange = await fetchWithTimeout(
          `${options.baseUrl}${CLOUD_AUTH_GITHUB_PATH}`,
          {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({
              accessToken,
              clientLabel: clientLabel(),
            }),
          },
          GITHUB_TIMEOUT_MS,
        );
        if (!exchange.ok) {
          throw new Error(`GitHub 登录交换失败（HTTP ${exchange.status}）`);
        }
        const session = (await exchange.json().catch(() => null)) as {
          sessionToken?: unknown;
        } | null;
        if (!session || typeof session.sessionToken !== "string" || !session.sessionToken) {
          throw new Error("GitHub 登录响应缺少会话令牌");
        }
        options.log("cloud_github_login ok");
        return finishCloudLogin(options, session.sessionToken, await deviceIdentity());
      }
      const error = typeof body.error === "string" ? body.error : "";
      if (error === "authorization_pending") {
        // 用户还没在浏览器里授权：返回未绑定状态让登录端继续等待。
        return { status: "unbound" };
      }
      if (error === "slow_down") {
        pending.currentIntervalMs += SLOW_DOWN_PENALTY_MS;
        return { status: "unbound" };
      }
      if (error === "expired_token") {
        pending = null;
        throw new Error("GitHub 授权已过期，请重新发起登录");
      }
      throw new Error(
        typeof body.error_description === "string" && body.error_description
          ? `GitHub 登录失败：${body.error_description}`
          : `GitHub 登录失败：${error || "未知错误"}`,
      );
    },

    /** 注销云端会话（best-effort）并清除本机会话令牌。 */
    async logout(): Promise<CloudAccountState> {
      const sessionToken = (
        await options.credentials.load(CLOUD_SESSION_TOKEN_CREDENTIAL_KEY)
      )?.trim();
      if (sessionToken) {
        try {
          await postCloudJson(options, CLOUD_AUTH_LOGOUT_PATH, {}, sessionToken);
        } catch {
          // 云端不可达时也要完成本地清理；服务端会话按 TTL 过期兜底。
        }
        await options.credentials.delete(CLOUD_SESSION_TOKEN_CREDENTIAL_KEY);
      }
      const state = await options.readState();
      options.onState(state);
      return state;
    },

    cancel(): void {
      pending = null;
    },
  };
}
