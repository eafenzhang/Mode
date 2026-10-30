/**
 * Mode Cloud 用户名密码登录/注册 + 会话路径设备 enroll
 * （docs/specs/cloud-personal-accounts.md）。
 *
 * 注册/登录成功 → 保存 sessionToken → 用会话 enroll 本机设备（Ed25519 断言，
 * 无 account 块）→ 保存设备与中继令牌 → 启动隧道（由调用方注入）。
 */
import { hostname } from "node:os";

import {
  CLOUD_ACCOUNT_ID_CREDENTIAL_KEY,
  CLOUD_AUTH_CREDENTIALS_PATH,
  CLOUD_AUTH_LOGIN_PATH,
  CLOUD_AUTH_LOGOUT_PATH,
  CLOUD_AUTH_REGISTER_PATH,
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
 * 成功保存 accountId / deviceToken / relayToken 并启动隧道。
 */
export async function enrollDeviceWithSession(
  options: CloudAuthFlowOptions,
  sessionToken: string,
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

/** 登录/注册/GitHub 换取会话后的共同收尾：存会话 → enroll → 通知状态。 */
export async function finishCloudLogin(
  options: CloudAuthFlowOptions,
  sessionToken: string,
): Promise<CloudAccountState> {
  await options.credentials.save(CLOUD_SESSION_TOKEN_CREDENTIAL_KEY, sessionToken);
  await enrollDeviceWithSession(options, sessionToken);
  const state = await options.readState();
  options.onState(state);
  return state;
}

export function createCloudPasswordAuth(options: CloudAuthFlowOptions) {
  return {
    async register(params: {
      username: string;
      password: string;
      displayName?: string;
      inviteCode?: string;
    }): Promise<CloudAccountState> {
      const body = await postCloudJson(options, CLOUD_AUTH_REGISTER_PATH, {
        username: params.username,
        password: params.password,
        ...(params.displayName ? { displayName: params.displayName } : {}),
        ...(params.inviteCode ? { inviteCode: params.inviteCode } : {}),
      });
      if (typeof body.sessionToken !== "string" || !body.sessionToken) {
        throw new Error("注册响应缺少会话令牌");
      }
      options.log("cloud_register ok");
      return finishCloudLogin(options, body.sessionToken);
    },

    async login(params: { username: string; password: string }): Promise<CloudAccountState> {
      const body = await postCloudJson(options, CLOUD_AUTH_LOGIN_PATH, {
        username: params.username,
        password: params.password,
      });
      if (typeof body.sessionToken !== "string" || !body.sessionToken) {
        throw new Error("登录响应缺少会话令牌");
      }
      options.log("cloud_login ok");
      return finishCloudLogin(options, body.sessionToken);
    },

    /** GitHub/迁移账号补设用户名密码；优先用会话，其次设备令牌。 */
    async setCredentials(params: {
      username: string;
      password: string;
    }): Promise<CloudAccountState> {
      const sessionToken = (await options.credentials.load(CLOUD_SESSION_TOKEN_CREDENTIAL_KEY))?.trim();
      const deviceToken = (await options.credentials.load(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY))?.trim();
      const token = sessionToken || deviceToken;
      if (!token) {
        throw new Error("尚未登录 Mode Cloud，无法设置用户名密码");
      }
      await postCloudJson(
        options,
        CLOUD_AUTH_CREDENTIALS_PATH,
        { username: params.username, password: params.password },
        token,
      );
      options.log("cloud_credentials_set ok");
      const state = await options.readState();
      options.onState(state);
      return state;
    },

    /** 撤销云端会话（best-effort）并清除本机会话令牌。 */
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
  };
}
