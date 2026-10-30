/**
 * 控制面会话签发与响应（github / device-flow 共用）。
 * 独立成文件避免 controlAuthSession 与 controlAuthGithub 互相 import。
 */
import { randomBytes } from "node:crypto";
import type { ServerResponse } from "node:http";

import { hashDeviceToken, sendJson, sessionCookieHeader, type ControlApiOptions } from "./controlHttp.js";

const SESSION_TOKEN_BYTES = 32;
const DEFAULT_SESSION_TTL_MS = 30 * 24 * 3_600_000;

function sessionTtl(options: ControlApiOptions): number {
  return options.sessionTtlMs && options.sessionTtlMs > 0
    ? options.sessionTtlMs
    : DEFAULT_SESSION_TTL_MS;
}

/** 签发人类会话；调用方负责响应体。 */
export function issueCloudSession(
  options: ControlApiOptions,
  accountId: string,
  now: number,
  clientLabel?: string,
): { sessionToken: string; expiresAt: number } {
  const sessionToken = randomBytes(SESSION_TOKEN_BYTES).toString("base64url");
  const expiresAt = now + sessionTtl(options);
  options.store.auth.createSession({
    accountId,
    tokenHash: hashDeviceToken(sessionToken),
    ...(clientLabel ? { clientLabel } : {}),
    now,
    expiresAt,
  });
  return { sessionToken, expiresAt };
}

export function accountResponse(account: {
  id: string;
  displayName: string | null;
}): { id: string; displayName?: string } {
  return {
    id: account.id,
    ...(account.displayName ? { displayName: account.displayName } : {}),
  };
}

/** refresh 轮转：新令牌 + 同事务 revoke 旧会话；旧会话已失效返回 null。 */
export function rotateCloudSession(
  options: ControlApiOptions,
  session: { id: string; accountId: string; clientLabel: string | null },
  now: number,
): { sessionToken: string; expiresAt: number } | null {
  const sessionToken = randomBytes(SESSION_TOKEN_BYTES).toString("base64url");
  const expiresAt = now + sessionTtl(options);
  const rotated = options.store.auth.rotateSession({
    oldSessionId: session.id,
    accountId: session.accountId,
    tokenHash: hashDeviceToken(sessionToken),
    ...(session.clientLabel ? { clientLabel: session.clientLabel } : {}),
    now,
    expiresAt,
  });
  return rotated ? { sessionToken, expiresAt } : null;
}

/**
 * 会话成功响应：同时下发 HttpOnly 门户 Cookie（Desktop 只看 body 里的 token）。
 * Cookie 让门户在同域页面免 Bearer；Max-Age 与会话过期时间一致。
 */
export function sendSessionResponse(
  res: ServerResponse,
  options: ControlApiOptions,
  session: { sessionToken: string; expiresAt: number },
  account: { id: string; displayName: string | null },
): void {
  const now = options.now?.() ?? Date.now();
  const maxAgeSeconds = Math.max(0, Math.floor((session.expiresAt - now) / 1000));
  res.setHeader(
    "set-cookie",
    sessionCookieHeader(options.publicBaseUrl, session.sessionToken, maxAgeSeconds),
  );
  sendJson(res, 200, {
    sessionToken: session.sessionToken,
    expiresAt: session.expiresAt,
    account: accountResponse(account),
  });
}
