import type { IncomingMessage, ServerResponse } from "node:http";

import { accountAnchor, type GitHubVerifier } from "./github.js";
import type { RemoteGateway } from "./gateway.js";
import { CloudStore, type DeviceRow } from "./store.js";

export const MAX_BODY_BYTES = 64 * 1024;

export interface ApiDeps {
  store: CloudStore;
  gateway: RemoteGateway;
  pepper: string;
  github: GitHubVerifier;
  /** 浏览器 GitHub Device Flow 代理；未配置时不暴露对应端点。 */
  githubDevice?: {
    clientId: string;
    baseUrl?: string;
    fetchImpl?: typeof fetch;
  };
  /** GitHub OAuth App 回调登录；client secret 只保存在服务端。 */
  githubOAuth?: {
    clientId: string;
    clientSecret: string;
    callbackUrl: string;
    authorizeBaseUrl?: string;
    tokenBaseUrl?: string;
    fetchImpl?: typeof fetch;
  };
  /** 服务端公开地址，用于校验 OAuth returnUrl 来源。 */
  publicUrl?: string;
  /** 跨账号配对会话 TTL（缺省 180s；测试可缩短）。 */
  /**
   * 浏览器跨源调用 `/api/v1/*` 的白名单（与 Control/Relay WS 同一份配置）。
   * 未配置时不发 CORS 头：仅同源页面可用。
   */
  allowedOrigins?: string[];
}

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  constructor(status: number, code: string, message: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

export async function createGithubSession(
  deps: ApiDeps,
  accessToken: string,
  clientLabel?: string,
): Promise<{
  sessionToken: string;
  expiresAt: number;
  account: { id: string; displayName: string | null };
}> {
  const identity = await deps.github(accessToken);
  const anchor = accountAnchor(deps.pepper, identity.providerUserId);
  const account = deps.store.createOrFindAccount("github", anchor, identity.name ?? identity.login);
  const session = deps.store.createWebSession(account.id, clientLabel);
  return {
    sessionToken: session.token,
    expiresAt: session.expiresAt,
    account: { id: account.id, displayName: account.display_name },
  };
}

export async function readJson(req: IncomingMessage): Promise<Record<string, unknown>> {
  const chunks: Buffer[] = [];
  let total = 0;
  for await (const chunk of req) {
    const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    total += buffer.length;
    if (total > MAX_BODY_BYTES) throw new ApiError(413, "body_too_large", "请求体超过 64 KiB");
    chunks.push(buffer);
  }
  if (total === 0) return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    throw new ApiError(400, "invalid_message", "请求体不是合法 JSON");
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw new ApiError(400, "invalid_message", "请求体必须是 JSON 对象");
  }
  return parsed as Record<string, unknown>;
}

export function requireString(body: Record<string, unknown>, key: string): string {
  const value = body[key];
  if (typeof value !== "string" || value.length < 1 || value.length > 8192) {
    throw new ApiError(422, "invalid_message", `${key} 必须是非空字符串`);
  }
  return value;
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

export type { DeviceRow };
