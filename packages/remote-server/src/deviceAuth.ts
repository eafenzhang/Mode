import type { IncomingMessage, ServerResponse } from "node:http";

import {
  ApiError,
  createGithubSession,
  readJson,
  requireString,
  sendJson,
  type ApiDeps,
} from "./apiSupport.js";

/** 浏览器 GitHub Device Flow 代理：GitHub token 只在服务端换 Web 会话。 */
export async function handleGithubDeviceRoute(
  deps: ApiDeps,
  req: IncomingMessage,
  res: ServerResponse,
  method: string,
  path: string,
): Promise<boolean> {
  if (method !== "POST") return false;
  if (path === "/api/v1/auth/github/device/start") {
    const device = requireDevice(deps);
    const fetchImpl = device.fetchImpl ?? fetch;
    const response = await fetchImpl(`${device.baseUrl ?? "https://github.com"}/login/device/code`, {
      method: "POST",
      headers: {
        accept: "application/json",
        "content-type": "application/x-www-form-urlencoded",
      },
      body: new URLSearchParams({ client_id: device.clientId, scope: "read:user" }),
    });
    const body = (await response.json()) as Record<string, unknown>;
    if (
      !response.ok ||
      typeof body.device_code !== "string" ||
      typeof body.user_code !== "string"
    ) {
      throw new ApiError(502, "github_device_failed", "GitHub Device Flow 启动失败");
    }
    sendJson(res, 200, {
      deviceCode: body.device_code,
      userCode: body.user_code,
      verificationUri:
        typeof body.verification_uri === "string"
          ? body.verification_uri
          : "https://github.com/login/device",
      expiresIn: typeof body.expires_in === "number" ? body.expires_in : 900,
      interval: typeof body.interval === "number" ? body.interval : 5,
    });
    return true;
  }

  if (path === "/api/v1/auth/github/device/poll") {
    const device = requireDevice(deps);
    const body = await readJson(req);
    const deviceCode = requireString(body, "deviceCode");
    const fetchImpl = device.fetchImpl ?? fetch;
    const response = await fetchImpl(
      `${device.baseUrl ?? "https://github.com"}/login/oauth/access_token`,
      {
        method: "POST",
        headers: {
          accept: "application/json",
          "content-type": "application/x-www-form-urlencoded",
        },
        body: new URLSearchParams({
          client_id: device.clientId,
          device_code: deviceCode,
          grant_type: "urn:ietf:params:oauth:grant-type:device_code",
        }),
      },
    );
    const payload = (await response.json()) as Record<string, unknown>;
    if (typeof payload.access_token === "string") {
      sendJson(res, 200, {
        status: "ok",
        ...(await createGithubSession(deps, payload.access_token, "web-device")),
      });
      return true;
    }
    if (payload.error === "authorization_pending") {
      sendJson(res, 200, { status: "pending" });
      return true;
    }
    if (payload.error === "slow_down") {
      sendJson(res, 200, {
        status: "slow_down",
        interval: typeof payload.interval === "number" ? payload.interval : undefined,
      });
      return true;
    }
    if (payload.error === "expired_token") {
      sendJson(res, 200, { status: "expired" });
      return true;
    }
    if (payload.error === "access_denied") {
      sendJson(res, 200, { status: "denied" });
      return true;
    }
    throw new ApiError(400, "github_device_failed", "GitHub Device Flow 轮询失败");
  }
  return false;
}

function requireDevice(deps: ApiDeps): NonNullable<ApiDeps["githubDevice"]> {
  if (!deps.githubDevice) {
    throw new ApiError(503, "github_device_unavailable", "服务端未配置 GitHub Device Flow");
  }
  return deps.githubDevice;
}
