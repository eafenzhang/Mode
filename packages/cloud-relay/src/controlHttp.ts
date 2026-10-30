/**
 * 控制面共用的 HTTP/校验/限速辅助（controlApi / controlAuth）。
 * 抽出来是为了让两个路由文件都低于行数上限，并保持令牌哈希语义只有一份。
 */
import { createHash, createHmac } from "node:crypto";
import type { IncomingMessage, ServerResponse } from "node:http";

import type { CloudStore } from "./cloudStore.js";
import type { GithubDeviceFlow } from "./githubDeviceFlow.js";

const MAX_BODY_BYTES = 64 * 1024;
const ENROLL_ATTEMPTS_PER_MINUTE = 10;
export const RELAY_TUNNEL_ID_PATTERN = /^[A-Za-z0-9_-]{16,128}$/u;
/** 门户会话 Cookie（docs/specs/cloud-personal-accounts.md §手机/浏览器门户）。 */
export const CLOUD_SESSION_COOKIE = "mode_cloud_session";

export interface ControlApiOptions {
  store: CloudStore;
  enrollToken: string;
  accountPepper: string;
  publicBaseUrl: string;
  /** 登录成功后下发给 Desktop 的共享中继令牌；后续可替换为每设备令牌。 */
  relayToken: string;
  getTunnelIds: () => string[];
  /** revoke 设备后由控制面回调，主动断开该设备隧道（docs/specs/cloud-personal-accounts.md）。 */
  closeTunnel: (tunnelId: string) => void;
  /** 隧道按设备鉴权；server 在其返回 false 时回退共享 secret（纯中继部署兼容）。 */
  verifyTunnelToken: (tunnelId: string, presented: string) => boolean;
  /** 注册邀请码；未配置 = 开放注册（docs/specs/cloud-personal-accounts.md）。 */
  inviteCode?: string;
  /** 人类会话时长（毫秒），默认 30 天滑动过期。 */
  sessionTtlMs?: number;
  /** GitHub API 基址（验证 access token 用）；默认 https://api.github.com。 */
  githubApiBaseUrl?: string;
  /** 门户 GitHub Device Flow 代理；未配置 client_id 时为 null（/device/* 返回 503）。 */
  githubDeviceFlow: GithubDeviceFlow | null;
  log: (line: string) => void;
  now?: () => number;
}

const attempts = new Map<string, { count: number; resetAt: number }>();

export function clientIpAddress(req: IncomingMessage): string {
  const forwarded = req.headers["cf-connecting-ip"] ?? req.headers["x-forwarded-for"];
  if (typeof forwarded === "string" && forwarded.trim()) {
    return forwarded.split(",")[0]!.trim();
  }
  return req.socket.remoteAddress ?? "unknown";
}

/** 通用滑动窗口限速（1 分钟）；bucket 需自带命名空间避免互串。 */
export function allowAttempt(bucket: string, limit: number, now: number): boolean {
  const current = attempts.get(bucket);
  if (!current || current.resetAt <= now) {
    attempts.set(bucket, { count: 1, resetAt: now + 60_000 });
    return true;
  }
  if (current.count >= limit) {
    return false;
  }
  current.count += 1;
  return true;
}

export function allowEnrollAttempt(req: IncomingMessage, now: number): boolean {
  return allowAttempt(`enroll:${clientIpAddress(req)}`, ENROLL_ATTEMPTS_PER_MINUTE, now);
}

export function pruneAttempts(now: number): void {
  if (attempts.size < 10_000) {
    return;
  }
  for (const [key, value] of attempts) {
    if (value.resetAt <= now) {
      attempts.delete(key);
    }
  }
}

export function sendJson(res: ServerResponse, status: number, body: unknown): void {
  const payload = JSON.stringify(body);
  res.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
    "content-length": Buffer.byteLength(payload),
  });
  res.end(payload);
}

export async function readJsonBody(req: IncomingMessage): Promise<unknown | null> {
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const chunk of req) {
    const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    size += buffer.length;
    if (size > MAX_BODY_BYTES) {
      return null;
    }
    chunks.push(buffer);
  }
  if (size === 0) {
    return null;
  }
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    return null;
  }
}

export function asTrimmedString(value: unknown, max: number): string | null {
  if (typeof value !== "string") {
    return null;
  }
  const trimmed = value.trim();
  return trimmed.length > 0 && trimmed.length <= max ? trimmed : null;
}

export function hashProviderUser(
  pepper: string,
  provider: string,
  providerUserId: string,
): string {
  return createHmac("sha256", pepper).update(`${provider}:${providerUserId}`).digest("hex");
}

export function hashDeviceToken(token: string): string {
  return createHash("sha256").update(token).digest("hex");
}

export function readBearer(req: IncomingMessage): string | null {
  const header = req.headers.authorization;
  if (typeof header !== "string" || !header.startsWith("Bearer ")) {
    return null;
  }
  const token = header.slice("Bearer ".length).trim();
  return token || null;
}

/** 门户会话 Cookie 读取（Desktop 仍走 Bearer）。 */
export function readSessionCookie(req: IncomingMessage): string | null {
  const header = req.headers.cookie;
  if (typeof header !== "string") {
    return null;
  }
  for (const part of header.split(";")) {
    const separator = part.indexOf("=");
    if (separator <= 0) {
      continue;
    }
    if (part.slice(0, separator).trim() !== CLOUD_SESSION_COOKIE) {
      continue;
    }
    const value = part.slice(separator + 1).trim();
    return value || null;
  }
  return null;
}

/** 会话 Cookie：HttpOnly + SameSite=Lax；HTTPS 部署再加 Secure。 */
export function sessionCookieHeader(
  publicBaseUrl: string,
  token: string,
  maxAgeSeconds: number,
): string {
  const parts = [
    `${CLOUD_SESSION_COOKIE}=${token}`,
    "Path=/",
    "HttpOnly",
    "SameSite=Lax",
    `Max-Age=${Math.max(0, Math.floor(maxAgeSeconds))}`,
  ];
  if (publicBaseUrl.startsWith("https:")) {
    parts.push("Secure");
  }
  return parts.join("; ");
}
