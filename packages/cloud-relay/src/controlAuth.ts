/**
 * 云端微信 Bot 登录路由：/v1/auth/start 与 /v1/auth/poll。
 *
 * Bot 只负责把微信用户发来的 6 位码标记为 approved；令牌交换在 poll 完成。
 * 成功响应包含共享中继令牌，Desktop 不再需要部署 secret 或 relay env。
 */
import { randomBytes, randomInt } from "node:crypto";
import type { IncomingMessage, ServerResponse } from "node:http";

import {
  RELAY_TUNNEL_ID_PATTERN,
  allowEnrollAttempt,
  asTrimmedString,
  hashDeviceToken,
  readJsonBody,
  sendJson,
  type ControlApiOptions,
} from "./controlHttp.js";
import { safeEqual } from "./internal.js";

interface AuthDevice {
  id: string;
  name: string;
  platform: string;
  version?: string;
  publicKey: string;
  relayTunnelId: string;
}

function parseAuthPollBody(value: unknown): { code: string; device: AuthDevice } | null {
  if (!value || typeof value !== "object") {
    return null;
  }
  const body = value as Record<string, unknown>;
  const device = body.device as Record<string, unknown> | undefined;
  const code = asTrimmedString(body.code, 8);
  if (!code || !/^\d{6}$/u.test(code) || !device) {
    return null;
  }
  const deviceId = asTrimmedString(device.id, 128);
  const name = asTrimmedString(device.name, 64);
  const platform = asTrimmedString(device.platform, 32);
  const publicKey = asTrimmedString(device.publicKey, 4096);
  const relayTunnelId = asTrimmedString(device.relayTunnelId, 128);
  if (
    !deviceId ||
    !name ||
    !platform ||
    !publicKey ||
    !relayTunnelId ||
    !RELAY_TUNNEL_ID_PATTERN.test(relayTunnelId)
  ) {
    return null;
  }
  const version = asTrimmedString(device.version, 64);
  return {
    code,
    device: {
      id: deviceId,
      name,
      platform,
      ...(version ? { version } : {}),
      publicKey,
      relayTunnelId,
    },
  };
}

export async function handleSetWechatBot(
  req: IncomingMessage,
  res: ServerResponse,
  options: ControlApiOptions,
): Promise<void> {
  const presented = req.headers["x-mode-enroll-token"];
  if (typeof presented !== "string" || !safeEqual(options.enrollToken, presented.trim())) {
    sendJson(res, 401, { error: "unauthorized" });
    return;
  }
  const now = options.now?.() ?? Date.now();
  if (!allowEnrollAttempt(req, now)) {
    sendJson(res, 429, { error: "rate_limited" });
    return;
  }
  const body = (await readJsonBody(req)) as Record<string, unknown> | null;
  const token = asTrimmedString(body?.token, 4096);
  const allowedUser = asTrimmedString(body?.allowedUser, 256);
  if (!token || token.length < 8 || (body?.allowedUser !== undefined && !allowedUser)) {
    sendJson(res, 400, { error: "invalid_request" });
    return;
  }
  await options.setWechatBot({
    token,
    ...(allowedUser ? { allowedUser } : {}),
  });
  sendJson(res, 200, { ok: true, wechatLogin: options.isWechatLoginEnabled() });
}

export function handleAuthStart(
  req: IncomingMessage,
  res: ServerResponse,
  options: ControlApiOptions,
): void {
  if (!options.isWechatLoginEnabled()) {
    sendJson(res, 503, { error: "wechat_login_disabled" });
    return;
  }
  const now = options.now?.() ?? Date.now();
  if (!allowEnrollAttempt(req, now)) {
    sendJson(res, 429, { error: "rate_limited" });
    return;
  }
  const code = String(randomInt(0, 1_000_000)).padStart(6, "0");
  const expiresAt = now + 5 * 60_000;
  options.store.createLoginCode({ codeHash: hashDeviceToken(code), createdAt: now, expiresAt });
  sendJson(res, 200, { code, expiresAt });
}

export async function handleAuthPoll(
  req: IncomingMessage,
  res: ServerResponse,
  options: ControlApiOptions,
): Promise<void> {
  if (!options.isWechatLoginEnabled()) {
    sendJson(res, 503, { error: "wechat_login_disabled" });
    return;
  }
  const body = parseAuthPollBody(await readJsonBody(req));
  if (!body) {
    sendJson(res, 400, { error: "invalid_request" });
    return;
  }
  const now = options.now?.() ?? Date.now();
  const codeHash = hashDeviceToken(body.code);
  const code = options.store.getLoginCode(codeHash);
  if (!code || code.expiresAt <= now) {
    sendJson(res, 401, { error: "invalid_code" });
    return;
  }
  if (code.status === "pending") {
    sendJson(res, 202, { status: "pending" });
    return;
  }
  if (code.status !== "approved") {
    sendJson(res, 401, { error: "code_used" });
    return;
  }
  const providerUserHash = options.store.consumeLoginCode(codeHash, now);
  if (!providerUserHash) {
    sendJson(res, 401, { error: "code_used" });
    return;
  }
  const deviceToken = randomBytes(32).toString("base64url");
  const result = options.store.enroll({
    provider: "weixin",
    providerUserHash,
    device: body.device,
    tokenHash: hashDeviceToken(deviceToken),
    now,
  });
  if (!result.ok) {
    sendJson(res, result.error === "tunnel_conflict" ? 409 : 403, { error: result.error });
    return;
  }
  options.log(
    `cloud_login device=${result.deviceId.slice(0, 8)}… account=${result.accountId.slice(0, 8)}…`,
  );
  sendJson(res, 200, {
    accountId: result.accountId,
    deviceId: result.deviceId,
    deviceToken,
    relayToken: options.relayToken,
  });
}
