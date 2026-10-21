/**
 * 控制面共用的 HTTP/校验/限速辅助（controlApi / controlAuth）。
 * 抽出来是为了让两个路由文件都低于行数上限，并保持令牌哈希语义只有一份。
 */
import { createHash, createHmac } from "node:crypto";
import type { IncomingMessage, ServerResponse } from "node:http";

import type { CloudStore } from "./cloudStore.js";

const MAX_BODY_BYTES = 64 * 1024;
const ENROLL_ATTEMPTS_PER_MINUTE = 10;
export const RELAY_TUNNEL_ID_PATTERN = /^[A-Za-z0-9_-]{16,128}$/u;

export interface ControlApiOptions {
  store: CloudStore;
  enrollToken: string;
  accountPepper: string;
  publicBaseUrl: string;
  /** 登录成功后下发给 Desktop 的共享中继令牌；后续可替换为每设备令牌。 */
  relayToken: string;
  /** 云端微信 Bot 当前是否在跑（可能由管理员接口动态启动）。 */
  isWechatLoginEnabled: () => boolean;
  /** 桌面自动上传云端登录 Bot token；保存并重启 Bot。 */
  setWechatBot: (input: { token: string; allowedUser?: string }) => Promise<void>;
  getTunnelIds: () => string[];
  log: (line: string) => void;
  now?: () => number;
}

const attempts = new Map<string, { count: number; resetAt: number }>();

function clientKey(req: IncomingMessage): string {
  const forwarded = req.headers["cf-connecting-ip"] ?? req.headers["x-forwarded-for"];
  if (typeof forwarded === "string" && forwarded.trim()) {
    return forwarded.split(",")[0]!.trim();
  }
  return req.socket.remoteAddress ?? "unknown";
}

export function allowEnrollAttempt(req: IncomingMessage, now: number): boolean {
  const key = clientKey(req);
  const current = attempts.get(key);
  if (!current || current.resetAt <= now) {
    attempts.set(key, { count: 1, resetAt: now + 60_000 });
    return true;
  }
  if (current.count >= ENROLL_ATTEMPTS_PER_MINUTE) {
    return false;
  }
  current.count += 1;
  return true;
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
