import { decodeBase64Url, encodeBase64Url } from "@mode/remote-core";
import type { RemoteV3HttpRequestParams, RemoteV3HttpResponse } from "@mode/shared";

import type { LocalApiTarget } from "./wsTunnel.js";

const MAX_BODY_BYTES = 2 * 1024 * 1024;
const ALLOWED_METHODS = new Set(["GET", "POST", "PUT", "PATCH", "DELETE"]);
const ALLOWED_HEADERS = new Set(["content-type", "accept", "x-mode-rpc-host-capability"]);
const BLOCKED_PATHS = [/^\/api\/lan(?:\/|$)/, /^\/api\/remote-agent(?:\/|$)/];

/** 通过 v3 调用 Host 本机 `/api/*`；Host 本地令牌只在本进程内注入，不回传。 */
export async function requestLocalHttp(
  target: LocalApiTarget,
  params: Omit<RemoteV3HttpRequestParams, "hostDeviceId">,
): Promise<RemoteV3HttpResponse> {
  if (!ALLOWED_METHODS.has(params.method)) throw new Error("unsupported http method");
  if (!params.path.startsWith("/api/")) throw new Error("unsupported http path");
  if (BLOCKED_PATHS.some((pattern) => pattern.test(params.path))) {
    throw new Error("blocked http path");
  }
  const url = new URL(params.path, target.baseUrl);
  // LAN API 只从 query/Cookie 读取本机令牌；v3 侧用 Cookie，避免把令牌放进 URL。
  const headers: Record<string, string> = {
    cookie: `mode_lite_token=${encodeURIComponent(target.token)}`,
  };
  for (const [rawName, value] of Object.entries(params.headers ?? {})) {
    const name = rawName.toLowerCase();
    if (!ALLOWED_HEADERS.has(name) || typeof value !== "string") continue;
    if (value.length > 1024 || /[\r\n]/.test(value)) continue;
    headers[name] = value;
  }
  const body = params.bodyBase64
    ? Buffer.from(decodeBase64Url(params.bodyBase64, "body"))
    : undefined;
  if (body && body.length > MAX_BODY_BYTES) throw new Error("request body too large");
  const response = await fetch(url, { method: params.method, headers, body });
  const buffer = Buffer.from(await response.arrayBuffer());
  if (buffer.length > MAX_BODY_BYTES) throw new Error("response body too large");
  const responseHeaders: Record<string, string> = {};
  for (const name of ["content-type", "cache-control"]) {
    const value = response.headers.get(name);
    if (value) responseHeaders[name] = value;
  }
  return {
    status: response.status,
    headers: responseHeaders,
    bodyBase64: encodeBase64Url(buffer),
  };
}
