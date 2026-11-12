import { createHash, randomBytes, timingSafeEqual } from "node:crypto";
import type { IncomingMessage, ServerResponse } from "node:http";

import {
  ApiError,
  createGithubSession,
  readJson,
  requireString,
  sendJson,
  type ApiDeps,
} from "./apiSupport.js";

const FLOW_TTL_MS = 10 * 60_000;
const POLL_INTERVAL_SECONDS = 2;

interface OAuthSession {
  sessionToken: string;
  expiresAt: number;
  account: { id: string; displayName: string | null };
}

interface OAuthAttempt {
  pollHash: Buffer;
  createdAt: number;
  expiresAt: number;
  status: "pending" | "authorized" | "denied" | "error";
  returnUrl?: string;
  session?: OAuthSession;
  error?: string;
}

/**
 * GitHub OAuth App 回调流的短期状态（10 分钟）。
 *
 * 只保存 state/pollToken 的哈希与会话结果，不落盘：服务重启会丢弃未完成登录，
 * 客户端重新发起即可；已完成的 Web 会话仍由 SQLite 的 `web_sessions` 持有。
 */
export class GithubOAuthStore {
  private readonly attempts = new Map<string, OAuthAttempt>();

  create(input: { returnUrl?: string }): { state: string; pollToken: string; expiresAt: number } {
    this.sweep();
    const state = randomBytes(32).toString("base64url");
    const pollToken = randomBytes(32).toString("base64url");
    const now = Date.now();
    this.attempts.set(hash(state), {
      pollHash: Buffer.from(hash(pollToken), "hex"),
      createdAt: now,
      expiresAt: now + FLOW_TTL_MS,
      ...(input.returnUrl ? { returnUrl: input.returnUrl } : {}),
      status: "pending",
    });
    return { state, pollToken, expiresAt: now + FLOW_TTL_MS };
  }

  find(state: string): { key: string; attempt: OAuthAttempt } | null {
    const key = hash(state);
    const attempt = this.attempts.get(key);
    return attempt ? { key, attempt } : null;
  }

  verifyPoll(attempt: OAuthAttempt, pollToken: string): boolean {
    const candidate = Buffer.from(hash(pollToken), "hex");
    return (
      candidate.length === attempt.pollHash.length && timingSafeEqual(candidate, attempt.pollHash)
    );
  }

  sweep(): void {
    const now = Date.now();
    for (const [key, attempt] of this.attempts) {
      if (attempt.expiresAt <= now) this.attempts.delete(key);
    }
    if (this.attempts.size === 0) return;
    // 上限防御：异常流量下也只保留最近的 512 条短期登录。
    while (this.attempts.size > 512) {
      const oldest = this.attempts.keys().next().value;
      if (oldest === undefined) break;
      this.attempts.delete(oldest);
    }
  }
}

/** GitHub OAuth App 回调：授权页 → 服务端换 token → 客户端轮询拿 Web 会话。 */
export async function handleGithubOAuthRoute(
  deps: ApiDeps,
  store: GithubOAuthStore,
  req: IncomingMessage,
  res: ServerResponse,
  method: string,
  path: string,
  url: URL,
): Promise<boolean> {
  if (method === "POST" && path === "/api/v1/auth/github/oauth/start") {
    const oauth = requireOAuth(deps);
    const body = await readJson(req);
    const returnUrl = readReturnUrl(body.returnUrl, deps);
    const created = store.create({ ...(returnUrl ? { returnUrl } : {}) });
    const authorizationUrl = new URL(
      "/login/oauth/authorize",
      oauth.authorizeBaseUrl ?? "https://github.com",
    );
    authorizationUrl.searchParams.set("client_id", oauth.clientId);
    authorizationUrl.searchParams.set("redirect_uri", oauth.callbackUrl);
    authorizationUrl.searchParams.set("scope", "read:user");
    authorizationUrl.searchParams.set("state", created.state);
    authorizationUrl.searchParams.set("allow_signup", "false");
    sendJson(res, 200, {
      authorizationUrl: authorizationUrl.toString(),
      state: created.state,
      pollToken: created.pollToken,
      expiresIn: Math.floor((created.expiresAt - Date.now()) / 1000),
      interval: POLL_INTERVAL_SECONDS,
    });
    return true;
  }

  if (method === "GET" && path === "/api/v1/auth/github/oauth/callback") {
    const oauth = requireOAuth(deps);
    const state = url.searchParams.get("state") ?? "";
    const found = state ? store.find(state) : null;
    if (!found || found.attempt.expiresAt <= Date.now()) {
      sendHtml(res, 400, resultPage(false, "登录请求已过期，请回到应用重新发起。"));
      return true;
    }
    const attempt = found.attempt;
    // state 只接受一次回调：重放或并发回调直接拒绝。
    if (attempt.status !== "pending") {
      sendHtml(res, 409, resultPage(false, "该登录请求已经处理过，请回到应用查看状态。"));
      return true;
    }
    const denied = url.searchParams.get("error");
    if (denied) {
      attempt.status = "denied";
      attempt.error = url.searchParams.get("error_description") ?? denied;
      oauth.log?.(
        `github_oauth_callback kind=github_error error=${denied} state=${state.slice(0, 6)}…`,
      );
      finish(res, attempt, false, "你取消了 GitHub 授权。");
      return true;
    }
    const code = url.searchParams.get("code");
    if (!code) {
      attempt.status = "error";
      attempt.error = "missing_code";
      finish(res, attempt, false, "GitHub 未返回授权码，请重试。");
      return true;
    }
    try {
      const fetchImpl = oauth.fetchImpl ?? fetch;
      const response = await fetchImpl(
        `${oauth.tokenBaseUrl ?? "https://github.com"}/login/oauth/access_token`,
        {
          method: "POST",
          headers: {
            accept: "application/json",
            "content-type": "application/x-www-form-urlencoded",
          },
          body: new URLSearchParams({
            client_id: oauth.clientId,
            client_secret: oauth.clientSecret,
            code,
            redirect_uri: oauth.callbackUrl,
          }),
        },
      );
      const payload = (await response.json()) as Record<string, unknown>;
      if (!response.ok || typeof payload.access_token !== "string") {
        throw new Error(
          typeof payload.error === "string" ? payload.error : `GitHub 返回 ${response.status}`,
        );
      }
      attempt.session = await createGithubSession(deps, payload.access_token, "web-oauth");
      attempt.status = "authorized";
      oauth.log?.(`github_oauth_callback kind=ok state=${state.slice(0, 6)}…`);
      finish(res, attempt, true, "GitHub 授权成功，已可返回应用。");
    } catch (error) {
      attempt.status = "error";
      attempt.error = error instanceof Error ? error.message : String(error);
      oauth.log?.(
        `github_oauth_callback kind=exchange_failed error=${attempt.error} state=${state.slice(0, 6)}…`,
      );
      finish(res, attempt, false, "GitHub 授权失败，请回到应用重试。");
    }
    return true;
  }

  if (method === "POST" && path === "/api/v1/auth/github/oauth/poll") {
    const body = await readJson(req);
    const state = requireString(body, "state");
    const pollToken = requireString(body, "pollToken");
    const found = store.find(state);
    if (!found || !store.verifyPoll(found.attempt, pollToken)) {
      throw new ApiError(404, "github_oauth_not_found", "登录请求不存在或已过期");
    }
    const attempt = found.attempt;
    if (attempt.expiresAt <= Date.now()) {
      sendJson(res, 200, { status: "expired" });
      return true;
    }
    if (attempt.status === "pending") {
      sendJson(res, 200, { status: "pending" });
      return true;
    }
    if (attempt.status === "denied") {
      sendJson(res, 200, { status: "denied" });
      return true;
    }
    if (attempt.status === "error") {
      sendJson(res, 200, { status: "error", message: attempt.error ?? "GitHub 授权失败" });
      return true;
    }
    sendJson(res, 200, {
      status: "ok",
      sessionToken: attempt.session!.sessionToken,
      expiresAt: attempt.session!.expiresAt,
      account: attempt.session!.account,
    });
    return true;
  }

  return false;
}

function requireOAuth(deps: ApiDeps): NonNullable<ApiDeps["githubOAuth"]> {
  if (!deps.githubOAuth) {
    throw new ApiError(503, "github_oauth_unavailable", "服务端未配置 GitHub OAuth 回调登录");
  }
  return deps.githubOAuth;
}

function hash(value: string): string {
  return createHash("sha256").update(value).digest("hex");
}

function readReturnUrl(value: unknown, deps: ApiDeps): string | undefined {
  if (value === undefined) return undefined;
  if (typeof value !== "string" || value.length > 2048) {
    throw new ApiError(422, "invalid_return_url", "returnUrl 格式非法");
  }
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new ApiError(422, "invalid_return_url", "returnUrl 必须是绝对地址");
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    throw new ApiError(422, "invalid_return_url", "returnUrl 只支持 http/https");
  }
  const allowed = new Set(
    (deps.allowedOrigins ?? []).flatMap((origin) => {
      try {
        return [new URL(origin).origin];
      } catch {
        return [];
      }
    }),
  );
  if (deps.publicUrl) {
    try {
      allowed.add(new URL(deps.publicUrl).origin);
    } catch {
      // 忽略非法 publicUrl，继续用来源白名单判定。
    }
  }
  if (!allowed.has(url.origin)) {
    throw new ApiError(422, "invalid_return_url", "returnUrl 不在允许的来源内");
  }
  return url.toString();
}

function finish(res: ServerResponse, attempt: OAuthAttempt, ok: boolean, message: string): void {
  if (attempt.returnUrl) {
    const target = new URL(attempt.returnUrl);
    target.searchParams.set("oauth", ok ? "1" : "error");
    res.writeHead(303, { location: target.toString(), "cache-control": "no-store" });
    res.end();
    return;
  }
  sendHtml(res, ok ? 200 : 400, resultPage(ok, message));
}

function sendHtml(res: ServerResponse, status: number, html: string): void {
  res.writeHead(status, {
    "content-type": "text/html; charset=utf-8",
    "cache-control": "no-store",
    "content-security-policy": "default-src 'none'; style-src 'unsafe-inline'",
  });
  res.end(html);
}

function resultPage(ok: boolean, message: string): string {
  return `<!doctype html><html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>${ok ? "授权成功" : "授权失败"}</title></head>
<body style="margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;background:#0b0b0d;color:#eee;font:15px/1.6 system-ui,sans-serif">
<main style="max-width:28rem;padding:1.5rem;text-align:center">
<h1 style="font-size:1.1rem;font-weight:600">${ok ? "GitHub 授权成功" : "GitHub 授权失败"}</h1>
<p style="color:#aaa">${escapeHtml(message)}</p>
</main></body></html>`;
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}
