/**
 * 个人账号路由：/v1/auth/register|login|refresh|logout|sessions|credentials
 * （docs/specs/cloud-personal-accounts.md）。
 *
 * 会话令牌只存 SHA-256；登录失败统一 401 invalid_credentials，不区分用户名/密码；
 * refresh 轮转旧会话；注册即登录。目录/revoke/credentials 复用 resolveControlActor
 * 同时接受会话令牌与设备令牌。
 */
import type { IncomingMessage, ServerResponse } from "node:http";

import { validatePassword, validateUsername } from "./cloudAuthStore.js";
import {
  handleGithubDevicePoll,
  handleGithubDeviceStart,
  handleGithubLogin,
} from "./controlAuthGithub.js";
import { resolveControlActor } from "./controlActor.js";
import { issueCloudSession, rotateCloudSession, sendSessionResponse } from "./controlSession.js";
import { hashPassword, verifyPassword } from "./passwordCrypto.js";
import {
  allowAttempt,
  asTrimmedString,
  clientIpAddress,
  hashDeviceToken,
  hashProviderUser,
  pruneAttempts,
  readBearer,
  readJsonBody,
  sendJson,
  sessionCookieHeader,
  type ControlApiOptions,
} from "./controlHttp.js";
import { safeEqual } from "./internal.js";

const LOGIN_ATTEMPTS_PER_MINUTE = 10;
const LOGIN_ATTEMPTS_PER_USERNAME = 5;
const REGISTER_ATTEMPTS_PER_MINUTE = 10;

interface CredentialsBody {
  username: string;
  password: string;
  displayName?: string;
  inviteCode?: string;
}

function parseCredentialsBody(value: unknown): CredentialsBody | null {
  if (!value || typeof value !== "object") {
    return null;
  }
  const record = value as Record<string, unknown>;
  const username = asTrimmedString(record.username, 64);
  const password = typeof record.password === "string" ? record.password : "";
  if (!username || !password) {
    return null;
  }
  const displayName = asTrimmedString(record.displayName, 64);
  const inviteCode = asTrimmedString(record.inviteCode, 128);
  return {
    username,
    password,
    ...(displayName ? { displayName } : {}),
    ...(inviteCode ? { inviteCode } : {}),
  };
}

export async function handleRegister(
  req: IncomingMessage,
  res: ServerResponse,
  options: ControlApiOptions,
): Promise<void> {
  const now = options.now?.() ?? Date.now();
  pruneAttempts(now);
  const body = parseCredentialsBody(await readJsonBody(req));
  if (!body) {
    sendJson(res, 400, { error: "invalid_request" });
    return;
  }
  if (!allowAttempt(`register:${clientIpAddress(req)}`, REGISTER_ATTEMPTS_PER_MINUTE, now)) {
    sendJson(res, 429, { error: "rate_limited" });
    return;
  }
  if (options.inviteCode) {
    if (!body.inviteCode || !safeEqual(options.inviteCode, body.inviteCode)) {
      sendJson(res, 403, { error: "invite_required" });
      return;
    }
  }
  const username = validateUsername(body.username);
  if (!username || !validatePassword(body.password)) {
    sendJson(res, 400, { error: "invalid_request" });
    return;
  }
  const passwordHash = await hashPassword(body.password);
  const created = options.store.auth.createAccount({
    provider: "password",
    providerUserHash: hashProviderUser(options.accountPepper, "password", username),
    username,
    passwordHash,
    ...(body.displayName ? { displayName: body.displayName } : {}),
    now,
  });
  if (created === "conflict") {
    sendJson(res, 409, { error: "username_taken" });
    return;
  }
  const session = issueCloudSession(options, created.id, now, "register");
  options.log(`cloud_register account=${created.id.slice(0, 8)}…`);
  sendSessionResponse(res, options, session, created);
}

export async function handleLogin(
  req: IncomingMessage,
  res: ServerResponse,
  options: ControlApiOptions,
): Promise<void> {
  const now = options.now?.() ?? Date.now();
  pruneAttempts(now);
  const body = parseCredentialsBody(await readJsonBody(req));
  if (!body) {
    sendJson(res, 400, { error: "invalid_request" });
    return;
  }
  const ip = clientIpAddress(req);
  if (
    !allowAttempt(`login-ip:${ip}`, LOGIN_ATTEMPTS_PER_MINUTE, now) ||
    !allowAttempt(`login-user:${body.username.toLowerCase()}`, LOGIN_ATTEMPTS_PER_USERNAME, now)
  ) {
    sendJson(res, 429, { error: "rate_limited" });
    return;
  }
  const username = validateUsername(body.username);
  // 密码登录按 username 列查找：GitHub/迁移账号补设密码后同样命中。
  const account = username ? options.store.auth.findAccountByUsername(username) : null;
  if (account && account.status !== "active") {
    sendJson(res, 403, { error: "account_disabled" });
    return;
  }
  const passwordHash = account?.passwordHash ?? null;
  // 用户名不存在与密码错误统一返回 invalid_credentials，不区分文案。
  const valid = passwordHash ? await verifyPassword(body.password, passwordHash) : false;
  if (!account || !passwordHash || !valid) {
    sendJson(res, 401, { error: "invalid_credentials" });
    return;
  }
  const rawLabel = req.headers["x-mode-client"];
  const clientLabel =
    (typeof rawLabel === "string" ? asTrimmedString(rawLabel, 64) : null) ?? undefined;
  const session = issueCloudSession(options, account.id, now, clientLabel);
  options.log(`cloud_login account=${account.id.slice(0, 8)}…`);
  sendSessionResponse(res, options, session, account);
}

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

/** 微信锚点账号补设用户名密码（Bearer 会话或设备令牌均可）。 */
export async function handleSetCredentials(
  req: IncomingMessage,
  res: ServerResponse,
  options: ControlApiOptions,
): Promise<void> {
  const now = options.now?.() ?? Date.now();
  pruneAttempts(now);
  const actor = resolveControlActor(options, req);
  if (!actor) {
    sendJson(res, 401, { error: "unauthorized" });
    return;
  }
  const body = parseCredentialsBody(await readJsonBody(req));
  if (!body) {
    sendJson(res, 400, { error: "invalid_request" });
    return;
  }
  const username = validateUsername(body.username);
  if (!username || !validatePassword(body.password)) {
    sendJson(res, 400, { error: "invalid_request" });
    return;
  }
  const passwordHash = await hashPassword(body.password);
  const result = options.store.auth.setAccountCredentials({
    accountId: actor.accountId,
    username,
    usernameHash: hashProviderUser(options.accountPepper, "password", username),
    passwordHash,
    now,
  });
  if (result === "conflict") {
    sendJson(res, 409, { error: "username_taken" });
    return;
  }
  if (result === "not_found") {
    sendJson(res, 404, { error: "not_found" });
    return;
  }
  options.log(`cloud_credentials account=${actor.accountId.slice(0, 8)}…`);
  sendJson(res, 200, { ok: true });
}

/** `/v1/auth/*` 个人账号路由统一入口（docs/specs/cloud-personal-accounts.md）。 */
export async function handlePasswordAuthRequest(
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
  if (url.pathname === "/v1/auth/register") {
    if (method !== "POST") {
      sendJson(res, 405, { error: "method_not_allowed" });
      return true;
    }
    await handleRegister(req, res, options);
    return true;
  }
  if (url.pathname === "/v1/auth/login") {
    if (method !== "POST") {
      sendJson(res, 405, { error: "method_not_allowed" });
      return true;
    }
    await handleLogin(req, res, options);
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
  if (url.pathname === "/v1/auth/credentials") {
    if (method !== "POST") {
      sendJson(res, 405, { error: "method_not_allowed" });
      return true;
    }
    await handleSetCredentials(req, res, options);
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
