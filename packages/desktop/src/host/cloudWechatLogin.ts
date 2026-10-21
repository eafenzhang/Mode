/**
 * Mode Cloud 微信 Bot 登录（docs/specs/mode-cloud-account-directory.md）。
 *
 * 从 cloudAccountService 抽出：开始登录 → 显示 6 位码 → 轮询云端直到 approved。
 * 成功后保存账号/设备/中继令牌，并触发隧道启动（由调用方注入）。
 */
import { hostname } from "node:os";

import {
  CLOUD_ACCOUNT_ID_CREDENTIAL_KEY,
  CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY,
  CLOUD_RELAY_TOKEN_CREDENTIAL_KEY,
  MODE_VERSION,
  type CloudAccountState,
  type CloudLoginChallenge,
} from "@mode/shared";
import type { ICredentialService } from "@mode/services";

import { loadOrCreateCloudDeviceIdentity } from "./cloudDeviceIdentity.js";
import { fetchWithTimeout } from "./lanPeerTransport.js";

const LOGIN_TIMEOUT_MS = 15_000;

interface BoundBotSummary {
  displayName?: string;
  providerUserId?: string;
}

export interface CloudWechatLoginOptions {
  credentials: ICredentialService;
  baseUrl: string;
  enrollmentConfigured: boolean;
  readBoundBot: () => Promise<BoundBotSummary | null>;
  ensureRelayTunnel: () => Promise<void>;
  readState: () => Promise<CloudAccountState>;
  onState: (state: CloudAccountState) => void;
  log: (line: string) => void;
}

export interface CloudWechatLogin {
  begin(): Promise<CloudLoginChallenge>;
  poll(): Promise<CloudAccountState>;
  cancel(): void;
}

export function createCloudWechatLogin(
  options: CloudWechatLoginOptions,
): CloudWechatLogin {
  let pending: CloudLoginChallenge | null = null;

  return {
    async begin() {
      const response = await fetchWithTimeout(
        `${options.baseUrl}/v1/auth/start`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: "{}",
        },
        LOGIN_TIMEOUT_MS,
      );
      if (response.status === 503) {
        throw new Error("云端微信登录未启用（需要 MODE_CLOUD_WECHAT_TOKEN）");
      }
      if (!response.ok) {
        throw new Error(`发起微信登录失败（HTTP ${response.status}）`);
      }
      const body = (await response.json()) as { code?: unknown; expiresAt?: unknown };
      if (
        typeof body.code !== "string" ||
        !/^\d{6}$/u.test(body.code) ||
        typeof body.expiresAt !== "number"
      ) {
        throw new Error("登录响应格式不正确");
      }
      pending = { code: body.code, expiresAt: body.expiresAt };
      return pending;
    },

    async poll() {
      if (!pending) {
        throw new Error("没有等待中的微信登录，请重新获取登录码");
      }
      if (pending.expiresAt <= Date.now()) {
        pending = null;
        throw new Error("登录码已过期，请重新获取");
      }
      const identity = await loadOrCreateCloudDeviceIdentity(options.credentials);
      const response = await fetchWithTimeout(
        `${options.baseUrl}/v1/auth/poll`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            code: pending.code,
            device: {
              id: identity.deviceId,
              name: hostname() || "Mode",
              platform: process.platform,
              version: MODE_VERSION,
              publicKey: identity.publicKeyPem,
              relayTunnelId: identity.relayTunnelId,
            },
          }),
        },
        LOGIN_TIMEOUT_MS,
      );
      if (response.status === 202) {
        const bot = await options.readBoundBot();
        const boundWeixinName =
          bot?.displayName?.trim() || bot?.providerUserId?.trim() || undefined;
        return {
          status: "pending",
          enrollmentConfigured: options.enrollmentConfigured,
          ...(boundWeixinName ? { boundWeixinName } : {}),
        };
      }
      if (!response.ok) {
        if (response.status === 401) {
          pending = null;
          throw new Error("登录码无效或已被使用，请重新获取");
        }
        if (response.status === 503) {
          throw new Error("云端微信登录未启用");
        }
        let message = `微信登录失败（HTTP ${response.status}）`;
        try {
          const body = (await response.json()) as { error?: unknown };
          if (typeof body.error === "string" && body.error) {
            message = `微信登录失败：${body.error}`;
          }
        } catch {
          // 保留 HTTP 状态错误。
        }
        pending = null;
        throw new Error(message);
      }
      const body = (await response.json()) as {
        accountId?: unknown;
        deviceToken?: unknown;
        relayToken?: unknown;
      };
      if (
        typeof body.accountId !== "string" ||
        !body.accountId ||
        typeof body.deviceToken !== "string" ||
        !body.deviceToken
      ) {
        throw new Error("登录响应缺少账号或设备令牌");
      }
      await options.credentials.save(CLOUD_ACCOUNT_ID_CREDENTIAL_KEY, body.accountId);
      await options.credentials.save(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY, body.deviceToken);
      if (typeof body.relayToken === "string" && body.relayToken) {
        await options.credentials.save(CLOUD_RELAY_TOKEN_CREDENTIAL_KEY, body.relayToken);
      }
      pending = null;
      await options.ensureRelayTunnel().catch(() => undefined);
      options.log(`cloud_wechat_login account=${body.accountId.slice(0, 8)}…`);
      const state = await options.readState();
      options.onState(state);
      return state;
    },

    cancel() {
      pending = null;
    },
  };
}
