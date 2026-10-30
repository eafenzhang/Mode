/**
 * Mode Cloud GitHub Device Flow 登录（docs/specs/cloud-personal-accounts.md）。
 *
 * begin：向 GitHub 申请 device code，返回 user_code + verificationUri 给 UI 展示；
 * poll：轮询 GitHub token 端点（authorization_pending 继续、slow_down 退避），
 * 拿到 access token 后调 /v1/auth/github 换云会话，再走会话路径 enroll 本机设备。
 */
import {
  CLOUD_AUTH_GITHUB_PATH,
  type CloudAccountState,
  type CloudGithubLoginChallenge,
} from "@mode/shared";

import { finishCloudLogin, type CloudAuthFlowOptions } from "./cloudPasswordLogin.js";
import { fetchWithTimeout } from "./lanPeerTransport.js";

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
}

async function postGithubForm(
  url: string,
  params: Record<string, string>,
): Promise<Record<string, unknown>> {
  const response = await fetchWithTimeout(
    url,
    {
      method: "POST",
      headers: {
        "content-type": "application/x-www-form-urlencoded",
        accept: "application/json",
      },
      body: new URLSearchParams(params).toString(),
    },
    GITHUB_TIMEOUT_MS,
  );
  const payload = (await response.json().catch(() => null)) as Record<string, unknown> | null;
  if (!payload) {
    throw new Error(`GitHub 请求失败（HTTP ${response.status}）`);
  }
  return payload;
}

export function createCloudGithubLogin(options: CloudGithubLoginOptions) {
  let pending: PendingGithubLogin | null = null;

  return {
    async begin(): Promise<CloudGithubLoginChallenge> {
      if (!options.githubClientId) {
        throw new Error(
          "未配置 GitHub OAuth Client ID（MODE_CLOUD_GITHUB_CLIENT_ID 或部署配置 githubClientId）",
        );
      }
      const body = await postGithubForm(GITHUB_DEVICE_CODE_URL, {
        client_id: options.githubClientId,
        scope: "read:user",
      });
      if (
        typeof body.device_code !== "string" ||
        !body.device_code ||
        typeof body.user_code !== "string" ||
        !body.user_code ||
        typeof body.verification_uri !== "string" ||
        !body.verification_uri
      ) {
        throw new Error(
          typeof body.error_description === "string" && body.error_description
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
        intervalMs: intervalRaw > 0 ? intervalRaw * 1000 : DEFAULT_POLL_INTERVAL_MS,
        currentIntervalMs: intervalRaw > 0 ? intervalRaw * 1000 : DEFAULT_POLL_INTERVAL_MS,
      };
      return {
        userCode: pending.userCode,
        verificationUri: pending.verificationUri,
        expiresAt: pending.expiresAt,
      };
    },

    /**
     * 单次轮询：pending 时返回未绑定状态（UI 继续等）；成功时完成会话交换 + enroll，
     * 返回 bound 状态；硬错误抛出可读消息。
     */
    async poll(): Promise<CloudAccountState> {
      if (!pending) {
        throw new Error("没有等待中的 GitHub 登录，请重新发起");
      }
      if (pending.expiresAt <= Date.now()) {
        pending = null;
        throw new Error("GitHub 授权已过期，请重新发起登录");
      }
      const body = await postGithubForm(GITHUB_TOKEN_URL, {
        client_id: options.githubClientId,
        device_code: pending.deviceCode,
        grant_type: "urn:ietf:params:oauth:grant-type:device_code",
      });
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
              clientLabel: `desktop-${process.platform}`,
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
        return finishCloudLogin(options, session.sessionToken);
      }
      const error = typeof body.error === "string" ? body.error : "";
      if (error === "authorization_pending") {
        // 用户还没在浏览器里授权：返回未绑定状态让 UI 继续等待。
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

    cancel(): void {
      pending = null;
    },
  };
}
