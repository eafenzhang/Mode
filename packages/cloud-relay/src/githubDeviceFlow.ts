/**
 * GitHub Device Flow 代理（docs/specs/cloud-personal-accounts.md §手机/浏览器门户）。
 *
 * GitHub 的 device/token 端点不返回 CORS 头，浏览器不能直连，因此由 relay 代理：
 * device_code 只留在 relay 内存（门户只拿 loginId），access token 轮询到后交给
 * 调用方验证，验证完即弃。
 */
import { randomBytes } from "node:crypto";

const DEFAULT_GITHUB_BASE = "https://github.com";
const DEVICE_CODE_PATH = "/login/device/code";
const TOKEN_PATH = "/login/oauth/access_token";
const GITHUB_TIMEOUT_MS = 10_000;
const MAX_PENDING = 1_000;
const PENDING_TTL_MS = 10 * 60_000;
/** 同一 loginId 的最小轮询间隔，防门户高频触发 GitHub 限速。 */
const MIN_POLL_INTERVAL_MS = 5_000;

interface PendingLogin {
  deviceCode: string;
  expiresAt: number;
  lastPollAt: number;
}

export interface GithubDeviceStart {
  loginId: string;
  userCode: string;
  verificationUri: string;
  expiresAt: number;
}

export type GithubDevicePoll =
  | { status: "pending" }
  | { status: "authorized"; accessToken: string }
  | { status: "expired" }
  | { status: "invalid" };

export interface GithubDeviceFlow {
  start(): Promise<GithubDeviceStart | null>;
  poll(loginId: string): Promise<GithubDevicePoll>;
}

async function postForm(
  url: string,
  params: Record<string, string>,
): Promise<Record<string, unknown> | null> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), GITHUB_TIMEOUT_MS);
  try {
    const response = await fetch(url, {
      method: "POST",
      headers: {
        "content-type": "application/x-www-form-urlencoded",
        accept: "application/json",
      },
      body: new URLSearchParams(params).toString(),
      signal: controller.signal,
    });
    const body = (await response.json().catch(() => null)) as Record<string, unknown> | null;
    return response.ok ? body : body;
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

export function createGithubDeviceFlow(options: {
  clientId: string;
  githubBaseUrl?: string;
  now?: () => number;
  /** 测试用：覆盖最小轮询间隔。 */
  minPollIntervalMs?: number;
}): GithubDeviceFlow {
  const base = (options.githubBaseUrl ?? DEFAULT_GITHUB_BASE).replace(/\/+$/u, "");
  const now = options.now ?? Date.now;
  const minPollIntervalMs =
    options.minPollIntervalMs && options.minPollIntervalMs >= 0
      ? options.minPollIntervalMs
      : MIN_POLL_INTERVAL_MS;
  const pending = new Map<string, PendingLogin>();

  const prune = (): void => {
    const current = now();
    for (const [key, entry] of pending) {
      if (entry.expiresAt <= current) {
        pending.delete(key);
      }
    }
    if (pending.size > MAX_PENDING) {
      const overflow = pending.size - MAX_PENDING;
      let removed = 0;
      for (const key of pending.keys()) {
        pending.delete(key);
        removed += 1;
        if (removed >= overflow) {
          break;
        }
      }
    }
  };

  return {
    async start(): Promise<GithubDeviceStart | null> {
      const body = await postForm(`${base}${DEVICE_CODE_PATH}`, {
        client_id: options.clientId,
        scope: "read:user",
      });
      if (
        !body ||
        typeof body.device_code !== "string" ||
        !body.device_code ||
        typeof body.user_code !== "string" ||
        !body.user_code ||
        typeof body.verification_uri !== "string" ||
        !body.verification_uri
      ) {
        return null;
      }
      prune();
      const expiresIn = typeof body.expires_in === "number" ? body.expires_in : 600;
      const loginId = randomBytes(16).toString("base64url");
      const expiresAt = now() + Math.min(expiresIn * 1000, PENDING_TTL_MS);
      pending.set(loginId, { deviceCode: body.device_code, expiresAt, lastPollAt: 0 });
      return {
        loginId,
        userCode: body.user_code,
        verificationUri: body.verification_uri,
        expiresAt,
      };
    },

    async poll(loginId: string): Promise<GithubDevicePoll> {
      const entry = pending.get(loginId);
      if (!entry) {
        return { status: "invalid" };
      }
      if (entry.expiresAt <= now()) {
        pending.delete(loginId);
        return { status: "expired" };
      }
      if (now() - entry.lastPollAt < minPollIntervalMs) {
        return { status: "pending" };
      }
      entry.lastPollAt = now();
      const body = await postForm(`${base}${TOKEN_PATH}`, {
        client_id: options.clientId,
        device_code: entry.deviceCode,
        grant_type: "urn:ietf:params:oauth:grant-type:device_code",
      });
      if (body && typeof body.access_token === "string" && body.access_token) {
        pending.delete(loginId);
        return { status: "authorized", accessToken: body.access_token };
      }
      const error = body && typeof body.error === "string" ? body.error : "";
      if (error === "authorization_pending" || error === "slow_down") {
        return { status: "pending" };
      }
      if (error === "expired_token") {
        pending.delete(loginId);
        return { status: "expired" };
      }
      pending.delete(loginId);
      return { status: "invalid" };
    },
  };
}
