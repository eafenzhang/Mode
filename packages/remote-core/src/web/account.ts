import { generateWebIdentity, importWebIdentity, exportWebIdentity } from "./identity.js";
import { parseWebProfile, readWebError, readWebJson, webRandomUUID } from "./accountSupport.js";

const PROFILE_KEY = "mode.remote.v3.web-profile";

export interface WebCredentialStore {
  get(key: string): Promise<string | null>;
  set(key: string, value: string): Promise<void>;
  delete(key: string): Promise<void>;
}

export interface WebDeviceProfile {
  serverUrl: string;
  deviceId: string;
  deviceName: string;
  /** X25519 私钥，原始 32 字节 base64url；不写日志/URL。 */
  privateKey: string;
  accessToken: string | null;
  refreshToken: string | null;
}

export interface WebDeviceSummary {
  deviceId: string;
  name: string;
  role: "host" | "client";
  platform: string;
  online: boolean;
  addresses: string[];
}

export interface WebGithubChallenge {
  deviceCode: string;
  userCode: string;
  verificationUri: string;
  expiresIn: number;
  interval: number;
}

export type WebGithubPoll =
  | { status: "pending" | "expired" | "denied" }
  | { status: "slow_down"; interval?: number }
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

export function normalizeWebServerUrl(raw: string): string {
  const url = new URL(raw.trim());
  const local =
    url.hostname === "localhost" || url.hostname === "127.0.0.1" || url.hostname === "::1";
  if (url.protocol !== "https:" && !(url.protocol === "http:" && local)) {
    throw new Error("远程控制 v3 服务必须使用 HTTPS（仅本机开发允许 HTTP）");
  }
  url.pathname = "";
  url.search = "";
  url.hash = "";
  return url.toString().replace(/\/$/u, "");
}

export async function loadOrCreateWebProfile(
  store: WebCredentialStore,
  options: { serverUrl?: string; deviceName?: string } = {},
): Promise<WebDeviceProfile> {
  const raw = await store.get(PROFILE_KEY);
  const parsed = raw ? parseWebProfile(raw) : null;
  const serverUrl = options.serverUrl ? normalizeWebServerUrl(options.serverUrl) : parsed?.serverUrl;
  if (!serverUrl) throw new Error("远程控制 v3 服务地址未配置");
  if (parsed && parsed.serverUrl === serverUrl) {
    const profile = {
      ...parsed,
      ...(options.deviceName ? { deviceName: options.deviceName } : {}),
    };
    if (options.deviceName) await saveWebProfile(store, profile);
    return profile;
  }
  // 首次使用或切换服务地址：生成独立设备身份，旧 token 全部失效。
  const identity = generateWebIdentity();
  const profile: WebDeviceProfile = {
    serverUrl,
    deviceId: webRandomUUID(),
    deviceName: options.deviceName?.trim() || "Mode Web",
    privateKey: exportWebIdentity(identity),
    accessToken: null,
    refreshToken: null,
  };
  await saveWebProfile(store, profile);
  return profile;
}

export async function saveWebProfile(
  store: WebCredentialStore,
  profile: WebDeviceProfile,
): Promise<void> {
  await store.set(PROFILE_KEY, JSON.stringify(profile));
}

export async function clearWebAuth(store: WebCredentialStore): Promise<WebDeviceProfile | null> {
  const raw = await store.get(PROFILE_KEY);
  const parsed = raw ? parseWebProfile(raw) : null;
  if (!parsed) return null;
  const next = { ...parsed, accessToken: null, refreshToken: null };
  await saveWebProfile(store, next);
  return next;
}

export async function exchangeGithubAccessToken(
  serverUrl: string,
  accessToken: string,
  fetchImpl: typeof fetch,
): Promise<string> {
  const response = await fetchImpl(`${normalizeWebServerUrl(serverUrl)}/api/v1/auth/github`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ accessToken, clientLabel: "web" }),
  });
  const body = await readWebJson(response);
  if (!response.ok) throw new WebApiError(response.status, readWebError(body, "GitHub 登录失败"));
  if (typeof body.sessionToken !== "string") throw new Error("登录响应缺少 sessionToken");
  return body.sessionToken;
}

/** 浏览器账号客户端：设备身份来自本地 store，token 刷新失败即要求重新登录。 */
export class WebAccountClient {
  private profile: WebDeviceProfile | null = null;

  constructor(
    private readonly options: {
      store: WebCredentialStore;
      fetchImpl?: typeof fetch;
      serverUrl?: string;
      deviceName?: string;
    },
  ) {}

  async loginWithGithubToken(githubAccessToken: string): Promise<WebDeviceProfile> {
    const profile = await this.ensureProfile();
    const sessionToken = await exchangeGithubAccessToken(
      profile.serverUrl,
      githubAccessToken,
      this.options.fetchImpl ?? fetch,
    );
    return await this.loginWithSessionToken(sessionToken);
  }

  async beginGithubDeviceLogin(): Promise<WebGithubChallenge> {
    const fetchImpl = this.options.fetchImpl ?? fetch;
    const profile = await this.ensureProfile();
    const response = await fetchImpl(
      `${profile.serverUrl}/api/v1/auth/github/device/start`,
      { method: "POST", headers: { "content-type": "application/json" }, body: "{}" },
    );
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
      deviceCode: body.deviceCode,
      userCode: body.userCode,
      verificationUri: body.verificationUri,
      expiresIn: typeof body.expiresIn === "number" ? body.expiresIn : 900,
      interval: typeof body.interval === "number" ? body.interval : 5,
    };
  }

  async pollGithubDeviceLogin(deviceCode: string): Promise<WebGithubPoll> {
    const fetchImpl = this.options.fetchImpl ?? fetch;
    const profile = await this.ensureProfile();
    const response = await fetchImpl(
      `${profile.serverUrl}/api/v1/auth/github/device/poll`,
      {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ deviceCode }),
      },
    );
    const body = await readWebJson(response);
    if (!response.ok) {
      throw new WebApiError(response.status, readWebError(body, "GitHub Device Flow 轮询失败"));
    }
    const status = body.status;
    if (status === "ok" && typeof body.sessionToken === "string") {
      return { status: "ok", profile: await this.loginWithSessionToken(body.sessionToken) };
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

  async loginWithSessionToken(sessionToken: string): Promise<WebDeviceProfile> {
    const fetchImpl = this.options.fetchImpl ?? fetch;
    let profile = await this.ensureProfile();
    try {
      profile = await this.registerDevice(profile, sessionToken, fetchImpl);
    } catch (error) {
      if (!(error instanceof WebApiError) || error.status !== 409) throw error;
      // deviceId/身份已被其他账号占用或撤销：轮换后重试一次。
      profile = await this.rotateProfile(profile);
      profile = await this.registerDevice(profile, sessionToken, fetchImpl);
    }
    await saveWebProfile(this.options.store, profile);
    this.profile = profile;
    return profile;
  }

  async getProfile(): Promise<WebDeviceProfile> {
    return await this.ensureProfile();
  }

  async listHosts(): Promise<WebDeviceSummary[]> {
    const fetchImpl = this.options.fetchImpl ?? fetch;
    let profile = await this.ensureProfile();
    if (!profile.accessToken) throw new Error("Web 设备尚未登录");
    try {
      return await this.listDevices(profile.serverUrl, profile.accessToken, fetchImpl);
    } catch (error) {
      if (!(error instanceof WebApiError) || error.status !== 401) throw error;
      profile = await this.refresh(profile, fetchImpl);
      await saveWebProfile(this.options.store, profile);
      return await this.listDevices(profile.serverUrl, profile.accessToken!, fetchImpl);
    }
  }

  async refreshTokens(): Promise<WebDeviceProfile> {
    const fetchImpl = this.options.fetchImpl ?? fetch;
    const next = await this.refresh(await this.ensureProfile(), fetchImpl);
    await saveWebProfile(this.options.store, next);
    this.profile = next;
    return next;
  }

  async logout(): Promise<void> {
    await this.options.store.delete(PROFILE_KEY);
    this.profile = null;
  }

  get identity() {
    return importWebIdentity(this.profile?.privateKey ?? "");
  }

  private async ensureProfile(): Promise<WebDeviceProfile> {
    if (!this.profile) {
      this.profile = await loadOrCreateWebProfile(this.options.store, {
        ...(this.options.serverUrl ? { serverUrl: this.options.serverUrl } : {}),
        ...(this.options.deviceName ? { deviceName: this.options.deviceName } : {}),
      });
    }
    return this.profile;
  }

  private async rotateProfile(profile: WebDeviceProfile): Promise<WebDeviceProfile> {
    const identity = generateWebIdentity();
    const next: WebDeviceProfile = {
      ...profile,
      deviceId: webRandomUUID(),
      privateKey: exportWebIdentity(identity),
      accessToken: null,
      refreshToken: null,
    };
    await saveWebProfile(this.options.store, next);
    this.profile = next;
    return next;
  }

  private async registerDevice(
    profile: WebDeviceProfile,
    sessionToken: string,
    fetchImpl: typeof fetch,
  ): Promise<WebDeviceProfile> {
    const response = await fetchImpl(`${profile.serverUrl}/api/v1/devices`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization: `Bearer ${sessionToken}`,
      },
      body: JSON.stringify({
        device: {
          deviceId: profile.deviceId,
          name: profile.deviceName,
          role: "client",
          platform: "web",
          identityKey: this.identity.identityKey,
        },
      }),
    });
    const body = await readWebJson(response);
    if (!response.ok) throw new WebApiError(response.status, readWebError(body, "设备注册失败"));
    if (typeof body.accessToken !== "string" || typeof body.refreshToken !== "string") {
      throw new Error("设备注册响应缺少 token");
    }
    return { ...profile, accessToken: body.accessToken, refreshToken: body.refreshToken };
  }

  private async refresh(
    profile: WebDeviceProfile,
    fetchImpl: typeof fetch,
  ): Promise<WebDeviceProfile> {
    if (!profile.refreshToken) throw new Error("refresh token 缺失，请重新登录");
    const response = await fetchImpl(`${profile.serverUrl}/api/v1/devices/refresh`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ deviceId: profile.deviceId, refreshToken: profile.refreshToken }),
    });
    const body = await readWebJson(response);
    if (!response.ok) throw new WebApiError(response.status, readWebError(body, "token 刷新失败"));
    if (typeof body.accessToken !== "string" || typeof body.refreshToken !== "string") {
      throw new Error("token 刷新响应缺少 token");
    }
    return { ...profile, accessToken: body.accessToken, refreshToken: body.refreshToken };
  }

  private async listDevices(
    serverUrl: string,
    accessToken: string,
    fetchImpl: typeof fetch,
  ): Promise<WebDeviceSummary[]> {
    const response = await fetchImpl(`${serverUrl}/api/v1/devices`, {
      headers: { authorization: `Bearer ${accessToken}` },
    });
    const body = await readWebJson(response);
    if (!response.ok) throw new WebApiError(response.status, readWebError(body, "读取设备目录失败"));
    const items = Array.isArray(body.items) ? body.items : [];
    return items.flatMap((entry) => {
      if (!entry || typeof entry !== "object") return [];
      const item = entry as {
        device?: Record<string, unknown>;
        online?: unknown;
        addresses?: unknown;
      };
      const device = item.device;
      if (
        !device ||
        typeof device.deviceId !== "string" ||
        typeof device.name !== "string" ||
        (device.role !== "host" && device.role !== "client") ||
        typeof device.platform !== "string"
      ) {
        return [];
      }
      return [
        {
          deviceId: device.deviceId,
          name: device.name,
          role: device.role,
          platform: device.platform,
          online: item.online === true,
          addresses: Array.isArray(item.addresses)
            ? item.addresses.filter((value): value is string => typeof value === "string")
            : [],
        },
      ];
    });
  }
}

