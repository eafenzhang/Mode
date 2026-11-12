import type { WebDeviceProfile } from "./accountTypes.js";
import { readWebError, readWebJson } from "./accountSupport.js";

export type WebGithubChallenge =
  | {
      kind: "device";
      deviceCode: string;
      userCode: string;
      verificationUri: string;
      expiresIn: number;
      interval: number;
    }
  | {
      kind: "oauth";
      authorizationUrl: string;
      state: string;
      pollToken: string;
      expiresIn: number;
      interval: number;
    };

export type WebGithubPoll =
  | { status: "pending" | "expired" | "denied" }
  | { status: "slow_down"; interval?: number }
  | { status: "error"; message: string }
  | { status: "ok"; profile: WebDeviceProfile };

export class WebApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = "WebApiError";
  }
}

interface LoginContext {
  serverUrl: string;
  fetchImpl: typeof fetch;
}

export async function startGithubDeviceLogin(input: LoginContext): Promise<WebGithubChallenge> {
  // fetch 不能作为对象方法调用：浏览器里 `this` 会变成普通对象触发 Illegal invocation。
  const { serverUrl, fetchImpl } = input;
  const response = await fetchImpl(`${serverUrl}/api/v1/auth/github/device/start`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: "{}",
  });
  const body = await readWebJson(response);
  if (!response.ok) {
    throw new WebApiError(response.status, readWebError(body, "GitHub Device Flow 启动失败"));
  }
  if (
    typeof body.deviceCode !== "string" ||
    typeof body.userCode !== "string" ||
    typeof body.verificationUri !== "string"
  ) {
    throw new Error("GitHub Device Flow 响应缺少字段");
  }
  return {
    kind: "device",
    deviceCode: body.deviceCode,
    userCode: body.userCode,
    verificationUri: body.verificationUri,
    expiresIn: typeof body.expiresIn === "number" ? body.expiresIn : 900,
    interval: typeof body.interval === "number" ? body.interval : 5,
  };
}

/** GitHub OAuth App 回调登录；服务端未配置时返回 null，调用方回落 Device Flow。 */
export async function startGithubOAuthLogin(
  input: LoginContext & { returnUrl?: string },
): Promise<WebGithubChallenge | null> {
  const { serverUrl, fetchImpl } = input;
  const response = await fetchImpl(`${input.serverUrl}/api/v1/auth/github/oauth/start`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input.returnUrl ? { returnUrl: input.returnUrl } : {}),
  });
  if (response.status === 503) return null;
  const body = await readWebJson(response);
  if (!response.ok) {
    throw new WebApiError(response.status, readWebError(body, "GitHub OAuth 登录启动失败"));
  }
  if (
    typeof body.authorizationUrl !== "string" ||
    typeof body.state !== "string" ||
    typeof body.pollToken !== "string"
  ) {
    throw new Error("GitHub OAuth 登录响应缺少字段");
  }
  return {
    kind: "oauth",
    authorizationUrl: body.authorizationUrl,
    state: body.state,
    pollToken: body.pollToken,
    expiresIn: typeof body.expiresIn === "number" ? body.expiresIn : 600,
    interval: typeof body.interval === "number" ? body.interval : 2,
  };
}

export async function pollGithubOAuthLogin(
  input: LoginContext & {
    state: string;
    pollToken: string;
    complete: (sessionToken: string) => Promise<WebDeviceProfile>;
  },
): Promise<WebGithubPoll> {
  const { fetchImpl } = input;
  const response = await fetchImpl(`${input.serverUrl}/api/v1/auth/github/oauth/poll`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ state: input.state, pollToken: input.pollToken }),
  });
  const body = await readWebJson(response);
  if (!response.ok) {
    throw new WebApiError(response.status, readWebError(body, "GitHub OAuth 轮询失败"));
  }
  const status = body.status;
  if (status === "ok" && typeof body.sessionToken === "string") {
    return { status: "ok", profile: await input.complete(body.sessionToken) };
  }
  if (status === "error") {
    return {
      status: "error",
      message: typeof body.message === "string" ? body.message : "GitHub 授权失败",
    };
  }
  if (status === "pending" || status === "expired" || status === "denied") {
    return { status };
  }
  throw new Error("GitHub OAuth 轮询返回未知状态");
}

export async function pollGithubDeviceLogin(
  input: LoginContext & {
    deviceCode: string;
    complete: (sessionToken: string) => Promise<WebDeviceProfile>;
  },
): Promise<WebGithubPoll> {
  const { fetchImpl } = input;
  const response = await fetchImpl(`${input.serverUrl}/api/v1/auth/github/device/poll`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ deviceCode: input.deviceCode }),
  });
  const body = await readWebJson(response);
  if (!response.ok) {
    throw new WebApiError(response.status, readWebError(body, "GitHub Device Flow 轮询失败"));
  }
  const status = body.status;
  if (status === "ok" && typeof body.sessionToken === "string") {
    return { status: "ok", profile: await input.complete(body.sessionToken) };
  }
  if (status === "slow_down") {
    return {
      status: "slow_down",
      ...(typeof body.interval === "number" ? { interval: body.interval } : {}),
    };
  }
  if (status === "expired" || status === "denied") return { status };
  return { status: "pending" };
}
