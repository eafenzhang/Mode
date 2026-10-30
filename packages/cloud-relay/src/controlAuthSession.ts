/**
 * 个人账号路由：/v1/auth/github|github/device/*|refresh|logout|sessions
 * （docs/specs/cloud-personal-accounts.md，v2：仅 GitHub 登录）。
 *
 * 会话令牌只存 SHA-256；refresh 轮转旧会话。目录/revoke 复用 resolveControlActor
 * 同时接受会话令牌与设备令牌。密码注册/登录/补设已删除（404）。
 */
import type { IncomingMessage, ServerResponse } from "node:http";

import {
  handleGithubDevicePoll,
  handleGithubDeviceStart,
  handleGithubLogin,
} from "./controlAuthGithub.js";
import { resolveControlActor } from "./controlActor.js";
import { rotateCloudSession } from "./controlSession.js";
import {
  hashDeviceToken,
  readBearer,
  sendJson,
  sessionCookieHeader,
  type ControlApiOptions,
} from "./controlHttp.js";

export async function handleRefresh(
  req: IncomingMessage,
  res: ServerResponse,
  options: ControlApiOptions,
): Promise<void> {
  const now = options.now?.() ?? Date.now();
  const bearer = readBearer(req);
  const session = bearer
    ? options.store.auth.findSessionByTokenHash(hashDeviceToken(bearer), now)
    : null;
  if (!bearer || !session) {
    sendJson(res, 401, { error: "session_expired" });
    return;
  }
  const nextToken = rotateCloudSession(options, session, now);
  if (!nextToken) {
    sendJson(res, 401, { error: "session_expired" });
    return;
  }
  sendJson(res, 200, { sessionToken: nextToken.sessionToken, expiresAt: nextToken.expiresAt });
}

export function handleLogout(
  req: IncomingMessage,
  res: ServerResponse,
  options: ControlApiOptions,
): void {
  const bearer = readBearer(req);
  if (!bearer) {
    sendJson(res, 401, { error: "unauthorized" });
    return;
  }
  options.store.auth.revokeSessionByTokenHash(hashDeviceToken(bearer));
  // 门户退出同时清 Cookie；Desktop 忽略该响应头。
  res.setHeader("set-cookie", sessionCookieHeader(options.publicBaseUrl, "", 0));
  sendJson(res, 200, { ok: true });
}

export function handleListSessions(
  req: IncomingMessage,
  res: ServerResponse,
  options: ControlApiOptions,
): void {
  const now = options.now?.() ?? Date.now();
  const actor = resolveControlActor(options, req);
  if (!actor || actor.source !== "session") {
    sendJson(res, 401, { error: "unauthorized" });
    return;
  }
  const sessions = options.store.auth.listSessions(actor.accountId, now).map((session) => ({
    id: session.id,
    ...(session.clientLabel ? { clientLabel: session.clientLabel } : {}),
    createdAt: session.createdAt,
    lastUsedAt: session.lastUsedAt,
    expiresAt: session.expiresAt,
  }));
  sendJson(res, 200, { sessions });
}

export function handleRevokeSession(
  req: IncomingMessage,
  res: ServerResponse,
  sessionId: string,
  options: ControlApiOptions,
): void {
  const actor = resolveControlActor(options, req);
  if (!actor || actor.source !== "session") {
    sendJson(res, 401, { error: "unauthorized" });
    return;
  }
  const revoked = options.store.auth.revokeSessionById(sessionId, actor.accountId);
  sendJson(res, revoked ? 200 : 404, revoked ? { ok: true } : { error: "not_found" });
}

/** `/v1/auth/*` 个人账号路由统一入口（docs/specs/cloud-personal-accounts.md）。 */
export async function handleSessionAuthRequest(
  req: IncomingMessage,
  res: ServerResponse,
  url: URL,
  options: ControlApiOptions,
): Promise<boolean> {
  const method = req.method;
  if (url.pathname === "/v1/auth/github/device/start") {
    if (method !== "POST") {
      sendJson(res, 405, { error: "method_not_allowed" });
      return true;
    }
    await handleGithubDeviceStart(req, res, options);
    return true;
  }
  if (url.pathname === "/v1/auth/github/device/poll") {
    if (method !== "POST") {
      sendJson(res, 405, { error: "method_not_allowed" });
      return true;
    }
    await handleGithubDevicePoll(req, res, options);
    return true;
  }
  if (url.pathname === "/v1/auth/github") {
    if (method !== "POST") {
      sendJson(res, 405, { error: "method_not_allowed" });
      return true;
    }
    await handleGithubLogin(req, res, options);
    return true;
  }
  if (url.pathname === "/v1/auth/refresh") {
    if (method !== "POST") {
      sendJson(res, 405, { error: "method_not_allowed" });
      return true;
    }
    await handleRefresh(req, res, options);
    return true;
  }
  if (url.pathname === "/v1/auth/logout") {
    if (method !== "POST") {
      sendJson(res, 405, { error: "method_not_allowed" });
      return true;
    }
    handleLogout(req, res, options);
    return true;
  }
  if (url.pathname === "/v1/auth/sessions") {
    if (method !== "GET") {
      sendJson(res, 405, { error: "method_not_allowed" });
      return true;
    }
    handleListSessions(req, res, options);
    return true;
  }
  const revoke = /^\/v1\/auth\/sessions\/([A-Za-z0-9_-]{1,128})$/u.exec(url.pathname);
  if (revoke) {
    if (method !== "DELETE") {
      sendJson(res, 405, { error: "method_not_allowed" });
      return true;
    }
    handleRevokeSession(req, res, revoke[1]!, options);
    return true;
  }
  sendJson(res, 404, { error: "not_found" });
  return true;
}