/**
 * GitHub 登录路由（docs/specs/cloud-personal-accounts.md）。
 *
 * - `POST /v1/auth/github`：Desktop Device Flow 拿到 access token 后换云会话。
 * - `POST /v1/auth/github/device/start|poll`：门户代理链路 —— GitHub 的
 *   device/token 端点没有 CORS 头，浏览器不能直连，由 relay 代持 device_code。
 *
 * 云端持 access token 调 `GET /user` 验证并用 GitHub 数字 id 定位/创建账号（JIT）。
 * access token 用完即弃：不落库、不写日志。
 */
import type { IncomingMessage, ServerResponse } from "node:http";

import { issueCloudSession, sendSessionResponse } from "./controlSession.js";
import {
  allowAttempt,
  asTrimmedString,
  clientIpAddress,
  hashProviderUser,
  pruneAttempts,
  readJsonBody,
  sendJson,
  type ControlApiOptions,
} from "./controlHttp.js";

const GITHUB_API_BASE_URL = "https://api.github.com";
const GITHUB_VERIFY_TIMEOUT_MS = 10_000;
const GITHUB_LOGIN_ATTEMPTS_PER_MINUTE = 10;

export interface GithubUserProfile {
  /** GitHub 数字用户 id（账号锚点），以 GitHub 返回为准。 */
  id: string;
  login: string;
  name?: string;
}

/** 用 access token 调 GitHub API 取用户；网络失败与无效 token 都返回 null。 */
export async function verifyGithubToken(
  baseUrl: string,
  accessToken: string,
): Promise<GithubUserProfile | null> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), GITHUB_VERIFY_TIMEOUT_MS);
  try {
    const response = await fetch(`${baseUrl.replace(/\/+$/u, "")}/user`, {
      headers: {
        authorization: `Bearer ${accessToken}`,
        accept: "application/vnd.github+json",
        "user-agent": "mode-cloud-relay",
        "x-github-api-version": "2022-11-28",
      },
      signal: controller.signal,
    });
    if (!response.ok) {
      return null;
    }
    const body = (await response.json()) as { id?: unknown; login?: unknown; name?: unknown };
    if (
      typeof body.id !== "number" ||
      !Number.isFinite(body.id) ||
      typeof body.login !== "string" ||
      !body.login
    ) {
      return null;
    }
    return {
      id: String(body.id),
      login: body.login,
      ...(typeof body.name === "string" && body.name.trim() ? { name: body.name.trim() } : {}),
    };
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

type GithubLoginResult =
  | { ok: true; sessionToken: string; expiresAt: number; account: GithubAccount }
  | { ok: false; error: "invalid_github_token" | "account_disabled" | "internal" };

interface GithubAccount {
  id: string;
  displayName: string | null;
  status: string;
}

/** 验证 GitHub token → 定位/JIT 建号 → 签发会话（/v1/auth/github 与门户共用）。 */
async function completeGithubLogin(
  options: ControlApiOptions,
  accessToken: string,
  now: number,
  clientLabel?: string,
): Promise<GithubLoginResult> {
  const profile = await verifyGithubToken(
    options.githubApiBaseUrl ?? GITHUB_API_BASE_URL,
    accessToken,
  );
  if (!profile) {
    return { ok: false, error: "invalid_github_token" };
  }
  const providerUserHash = hashProviderUser(options.accountPepper, "github", profile.id);
  let account = options.store.auth.findAccountByAnchor("github", providerUserHash) as
    | GithubAccount
    | null;
  if (!account) {
    const created = options.store.auth.createAccount({
      providerUserHash,
      displayName: profile.name ?? profile.login,
      now,
    });
    if (created !== "conflict") {
      account = created;
    }
  }
  if (!account) {
    return { ok: false, error: "internal" };
  }
  if (account.status !== "active") {
    return { ok: false, error: "account_disabled" };
  }
  const session = issueCloudSession(options, account.id, now, clientLabel);
  options.log(`cloud_github_login account=${account.id.slice(0, 8)}…`);
  return { ok: true, ...session, account };
}

function loginErrorStatus(error: GithubLoginResult & { ok: false }): number {
  if (error.error === "account_disabled") {
    return 403;
  }
  if (error.error === "internal") {
    return 500;
  }
  return 401;
}

export async function handleGithubLogin(
  req: IncomingMessage,
  res: ServerResponse,
  options: ControlApiOptions,
): Promise<void> {
  const now = options.now?.() ?? Date.now();
  pruneAttempts(now);
  if (!allowAttempt(`github:${clientIpAddress(req)}`, GITHUB_LOGIN_ATTEMPTS_PER_MINUTE, now)) {
    sendJson(res, 429, { error: "rate_limited" });
    return;
  }
  const body = (await readJsonBody(req)) as Record<string, unknown> | null;
  const accessToken = asTrimmedString(body?.accessToken, 512);
  const clientLabel = asTrimmedString(body?.clientLabel, 64);
  if (!accessToken) {
    sendJson(res, 400, { error: "invalid_request" });
    return;
  }
  const result = await completeGithubLogin(
    options,
    accessToken,
    now,
    clientLabel ?? undefined,
  );
  if (!result.ok) {
    sendJson(res, loginErrorStatus(result), { error: result.error });
    return;
  }
  sendSessionResponse(res, options, result, result.account);
}

/** 门户 GitHub 登录第一步：relay 代持 device_code，只把 user code 交给浏览器。 */
export async function handleGithubDeviceStart(
  req: IncomingMessage,
  res: ServerResponse,
  options: ControlApiOptions,
): Promise<void> {
  const now = options.now?.() ?? Date.now();
  pruneAttempts(now);
  if (!allowAttempt(`github-device:${clientIpAddress(req)}`, GITHUB_LOGIN_ATTEMPTS_PER_MINUTE, now)) {
    sendJson(res, 429, { error: "rate_limited" });
    return;
  }
  if (!options.githubDeviceFlow) {
    sendJson(res, 503, { error: "github_login_disabled" });
    return;
  }
  const started = await options.githubDeviceFlow.start();
  if (!started) {
    sendJson(res, 502, { error: "github_unreachable" });
    return;
  }
  sendJson(res, 200, started);
}

/** 门户 GitHub 登录轮询：pending 202；授权后换会话并下发 Cookie。 */
export async function handleGithubDevicePoll(
  req: IncomingMessage,
  res: ServerResponse,
  options: ControlApiOptions,
): Promise<void> {
  const now = options.now?.() ?? Date.now();
  if (!options.githubDeviceFlow) {
    sendJson(res, 503, { error: "github_login_disabled" });
    return;
  }
  const body = (await readJsonBody(req)) as Record<string, unknown> | null;
  const loginId = asTrimmedString(body?.loginId, 128);
  if (!loginId) {
    sendJson(res, 400, { error: "invalid_request" });
    return;
  }
  const result = await options.githubDeviceFlow.poll(loginId);
  if (result.status === "pending") {
    sendJson(res, 202, { status: "pending" });
    return;
  }
  if (result.status === "expired") {
    sendJson(res, 401, { error: "expired_token" });
    return;
  }
  if (result.status === "invalid") {
    sendJson(res, 401, { error: "invalid_login" });
    return;
  }
  const completed = await completeGithubLogin(options, result.accessToken, now, "web-portal");
  if (!completed.ok) {
    sendJson(res, loginErrorStatus(completed), { error: completed.error });
    return;
  }
  sendSessionResponse(res, options, completed, completed.account);
}
