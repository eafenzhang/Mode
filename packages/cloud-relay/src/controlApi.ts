/**
 * Mode Cloud 控制面（docs/specs/mode-cloud-account-directory.md）。
 *
 * 与中继同进程、同容器；只处理 `/v1/*`。请求校验手写在本地完成，避免把 workspace
 * 包（@mode/shared/zod）带进 relay 的独立 Docker 构建。
 */
import { randomBytes, verify } from "node:crypto";
import type { IncomingMessage, ServerResponse } from "node:http";

import { handleAuthPoll, handleAuthStart, handleSetWechatBot } from "./controlAuth.js";
import {
  RELAY_TUNNEL_ID_PATTERN,
  allowEnrollAttempt,
  asTrimmedString,
  hashDeviceToken,
  hashProviderUser,
  pruneAttempts,
  readBearer,
  readJsonBody,
  sendJson,
  type ControlApiOptions,
} from "./controlHttp.js";
import { safeEqual } from "./internal.js";

export type { ControlApiOptions } from "./controlHttp.js";

const ASSERTION_CLOCK_SKEW_MS = 5 * 60_000;

interface EnrollRequestBody {
  account: { provider: string; providerUserId: string; displayName?: string };
  device: {
    id: string;
    name: string;
    platform: string;
    version?: string;
    publicKey: string;
    relayTunnelId: string;
  };
  assertion: { payload: string; signature: string };
  nonce: string;
  issuedAt: number;
}

function parseEnrollBody(value: unknown): EnrollRequestBody | null {
  if (!value || typeof value !== "object") {
    return null;
  }
  const body = value as Record<string, unknown>;
  const account = body.account as Record<string, unknown> | undefined;
  const device = body.device as Record<string, unknown> | undefined;
  const assertion = body.assertion as Record<string, unknown> | undefined;
  if (!account || !device || !assertion) {
    return null;
  }
  const provider = asTrimmedString(account.provider, 32);
  const providerUserId = asTrimmedString(account.providerUserId, 256);
  const deviceId = asTrimmedString(device.id, 128);
  const name = asTrimmedString(device.name, 64);
  const platform = asTrimmedString(device.platform, 32);
  const publicKey = asTrimmedString(device.publicKey, 4096);
  const relayTunnelId = asTrimmedString(device.relayTunnelId, 128);
  const payload = asTrimmedString(assertion.payload, 8192);
  const signature = asTrimmedString(assertion.signature, 4096);
  const nonce = asTrimmedString(body.nonce, 128);
  if (
    provider !== "weixin" ||
    !providerUserId ||
    !deviceId ||
    !name ||
    !platform ||
    !publicKey ||
    !relayTunnelId ||
    !RELAY_TUNNEL_ID_PATTERN.test(relayTunnelId) ||
    !payload ||
    !signature ||
    !nonce ||
    nonce.length < 8 ||
    typeof body.issuedAt !== "number" ||
    !Number.isFinite(body.issuedAt)
  ) {
    return null;
  }
  const displayName = asTrimmedString(account.displayName, 64);
  const version = asTrimmedString(device.version, 64);
  return {
    account: { provider, providerUserId, ...(displayName ? { displayName } : {}) },
    device: {
      id: deviceId,
      name,
      platform,
      ...(version ? { version } : {}),
      publicKey,
      relayTunnelId,
    },
    assertion: { payload, signature },
    nonce,
    issuedAt: body.issuedAt,
  };
}

function verifyAssertion(
  body: EnrollRequestBody,
  now: number,
): "ok" | "invalid_signature" | "stale" | "mismatch" {
  if (Math.abs(now - body.issuedAt) > ASSERTION_CLOCK_SKEW_MS) {
    return "stale";
  }
  let parsed: Record<string, unknown>;
  try {
    parsed = JSON.parse(body.assertion.payload) as Record<string, unknown>;
  } catch {
    return "invalid_signature";
  }
  const account = parsed.account as Record<string, unknown> | undefined;
  const device = parsed.device as Record<string, unknown> | undefined;
  if (
    account?.provider !== body.account.provider ||
    account?.providerUserId !== body.account.providerUserId ||
    device?.id !== body.device.id ||
    device?.relayTunnelId !== body.device.relayTunnelId ||
    parsed.nonce !== body.nonce ||
    parsed.issuedAt !== body.issuedAt
  ) {
    return "mismatch";
  }
  try {
    const valid = verify(
      null,
      Buffer.from(body.assertion.payload, "utf8"),
      body.device.publicKey,
      Buffer.from(body.assertion.signature, "base64"),
    );
    return valid ? "ok" : "invalid_signature";
  } catch {
    return "invalid_signature";
  }
}

function buildRelayUrl(publicBaseUrl: string, tunnelId: string): string {
  return `${publicBaseUrl.replace(/\/+$/u, "")}/d/${tunnelId}`;
}

async function handleEnroll(
  req: IncomingMessage,
  res: ServerResponse,
  options: ControlApiOptions,
): Promise<void> {
  const now = options.now?.() ?? Date.now();
  pruneAttempts(now);
  if (!allowEnrollAttempt(req, now)) {
    sendJson(res, 429, { error: "rate_limited" });
    return;
  }
  const presented = req.headers["x-mode-enroll-token"];
  if (typeof presented !== "string" || !safeEqual(options.enrollToken, presented.trim())) {
    sendJson(res, 401, { error: "unauthorized" });
    return;
  }
  const body = parseEnrollBody(await readJsonBody(req));
  if (!body) {
    sendJson(res, 400, { error: "invalid_request" });
    return;
  }
  const assertion = verifyAssertion(body, now);
  if (assertion !== "ok") {
    sendJson(res, assertion === "stale" ? 400 : 401, { error: assertion });
    return;
  }
  const deviceToken = randomBytes(32).toString("base64url");
  const result = options.store.enroll({
    provider: body.account.provider,
    providerUserHash: hashProviderUser(
      options.accountPepper,
      body.account.provider,
      body.account.providerUserId,
    ),
    ...(body.account.displayName ? { displayName: body.account.displayName } : {}),
    device: body.device,
    tokenHash: hashDeviceToken(deviceToken),
    now,
  });
  if (!result.ok) {
    sendJson(res, result.error === "tunnel_conflict" ? 409 : 403, { error: result.error });
    return;
  }
  options.log(`cloud_enroll device=${result.deviceId.slice(0, 8)}… account=${result.accountId.slice(0, 8)}…`);
  sendJson(res, 200, {
    accountId: result.accountId,
    deviceId: result.deviceId,
    deviceToken,
  });
}

function handleListDevices(
  req: IncomingMessage,
  res: ServerResponse,
  options: ControlApiOptions,
): void {
  const token = readBearer(req);
  const found = token ? options.store.findDeviceByTokenHash(hashDeviceToken(token)) : null;
  if (!found) {
    sendJson(res, 401, { error: "unauthorized" });
    return;
  }
  if (found.account.status !== "active") {
    sendJson(res, 403, { error: "account_disabled" });
    return;
  }
  const onlineTunnels = new Set(options.getTunnelIds());
  const devices = options.store.listDevices(found.account.id).map((device) => ({
    id: device.id,
    name: device.name,
    platform: device.platform,
    ...(device.version ? { version: device.version } : {}),
    online: onlineTunnels.has(device.relayTunnelId),
    relayUrl: buildRelayUrl(options.publicBaseUrl, device.relayTunnelId),
    ...(device.lastSeenAt !== null ? { lastSeenAt: device.lastSeenAt } : {}),
  }));
  sendJson(res, 200, {
    account: {
      id: found.account.id,
      ...(found.account.displayName ? { displayName: found.account.displayName } : {}),
    },
    devices,
  });
}

function handleRevoke(
  req: IncomingMessage,
  res: ServerResponse,
  deviceId: string,
  options: ControlApiOptions,
): void {
  const token = readBearer(req);
  const found = token ? options.store.findDeviceByTokenHash(hashDeviceToken(token)) : null;
  if (!found) {
    sendJson(res, 401, { error: "unauthorized" });
    return;
  }
  const target = options.store
    .listDevices(found.account.id)
    .find((device) => device.id === deviceId);
  if (!target) {
    // 不区分“不存在”和“别的账号”，避免枚举设备。
    sendJson(res, 403, { error: "forbidden" });
    return;
  }
  options.store.revokeDevice(found.account.id, deviceId);
  options.log(`cloud_revoke device=${deviceId.slice(0, 8)}…`);
  sendJson(res, 200, { ok: true });
}

/** 处理 `/v1/*`；返回 true 表示已写响应。 */
export async function handleControlRequest(
  req: IncomingMessage,
  res: ServerResponse,
  url: URL,
  options: ControlApiOptions,
): Promise<boolean> {
  if (!url.pathname.startsWith("/v1/")) {
    return false;
  }
  if (url.pathname === "/v1/health") {
    if (req.method !== "GET") {
      sendJson(res, 405, { error: "method_not_allowed" });
      return true;
    }
    sendJson(res, 200, {
      ok: true,
      control: true,
      wechatLogin: options.isWechatLoginEnabled(),
      ...options.store.stats(),
    });
    return true;
  }
  if (url.pathname === "/v1/admin/wechat-bot") {
    if (req.method !== "POST") {
      sendJson(res, 405, { error: "method_not_allowed" });
      return true;
    }
    await handleSetWechatBot(req, res, options);
    return true;
  }
  if (url.pathname === "/v1/auth/start") {
    if (req.method !== "POST") {
      sendJson(res, 405, { error: "method_not_allowed" });
      return true;
    }
    handleAuthStart(req, res, options);
    return true;
  }
  if (url.pathname === "/v1/auth/poll") {
    if (req.method !== "POST") {
      sendJson(res, 405, { error: "method_not_allowed" });
      return true;
    }
    await handleAuthPoll(req, res, options);
    return true;
  }
  if (url.pathname === "/v1/devices/enroll") {
    if (req.method !== "POST") {
      sendJson(res, 405, { error: "method_not_allowed" });
      return true;
    }
    await handleEnroll(req, res, options);
    return true;
  }
  if (url.pathname === "/v1/devices") {
    if (req.method !== "GET") {
      sendJson(res, 405, { error: "method_not_allowed" });
      return true;
    }
    handleListDevices(req, res, options);
    return true;
  }
  const revoke = /^\/v1\/devices\/([A-Za-z0-9_-]{1,128})\/revoke$/u.exec(url.pathname);
  if (revoke) {
    if (req.method !== "POST") {
      sendJson(res, 405, { error: "method_not_allowed" });
      return true;
    }
    handleRevoke(req, res, revoke[1]!, options);
    return true;
  }
  sendJson(res, 404, { error: "not_found" });
  return true;
}
