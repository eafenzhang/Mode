import type { WebCredentialStore, WebDeviceProfile, WebDeviceSummary } from "./accountTypes.js";
import { generateWebIdentity, importWebIdentity, exportWebIdentity } from "./identity.js";
import {
  clearWebAuth,
  deleteWebProfile,
  loadOrCreateWebProfile,
  normalizeWebServerUrl,
  readWebError,
  readWebJson,
  saveWebProfile,
  webRandomUUID,
} from "./accountSupport.js";
import {
  pollGithubDeviceLogin as pollDeviceLogin,
  pollGithubOAuthLogin as pollOAuthLogin,
  startGithubDeviceLogin as startDeviceLogin,
  startGithubOAuthLogin as startOAuthLogin,
  WebApiError,
  type WebGithubChallenge,
  type WebGithubPoll,
} from "./githubLogin.js";

export type { WebGithubChallenge, WebGithubPoll } from "./githubLogin.js";
export { WebApiError } from "./githubLogin.js";

export type { WebCredentialStore, WebDeviceProfile, WebDeviceSummary } from "./accountTypes.js";

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
    const profile = await this.ensureProfile();
    return await startDeviceLogin(this.loginContext(profile.serverUrl));
  }

  /** GitHub OAuth App 回调登录；服务端未配置时返回 null，调用方回落 Device Flow。 */
  async beginGithubOAuthLogin(returnUrl?: string): Promise<WebGithubChallenge | null> {
    const profile = await this.ensureProfile();
    return await startOAuthLogin({
      ...this.loginContext(profile.serverUrl),
      ...(returnUrl ? { returnUrl } : {}),
    });
  }

  async pollGithubOAuthLogin(state: string, pollToken: string): Promise<WebGithubPoll> {
    const profile = await this.ensureProfile();
    return await pollOAuthLogin({
      ...this.loginContext(profile.serverUrl),
      state,
      pollToken,
      complete: (sessionToken) => this.loginWithSessionToken(sessionToken),
    });
  }

  async pollGithubDeviceLogin(deviceCode: string): Promise<WebGithubPoll> {
    const profile = await this.ensureProfile();
    return await pollDeviceLogin({
      ...this.loginContext(profile.serverUrl),
      deviceCode,
      complete: (sessionToken) => this.loginWithSessionToken(sessionToken),
    });
  }

  private loginContext(serverUrl: string): { serverUrl: string; fetchImpl: typeof fetch } {
    return { serverUrl, fetchImpl: this.options.fetchImpl ?? fetch };
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
    const filterSelf = (devices: WebDeviceSummary[]): WebDeviceSummary[] =>
      devices.filter((device) => device.role === "host" && device.deviceId !== profile.deviceId);
    try {
      return filterSelf(await this.listDevices(profile.serverUrl, profile.accessToken, fetchImpl));
    } catch (error) {
      if (!(error instanceof WebApiError) || error.status !== 401) throw error;
      profile = await this.refresh(profile, fetchImpl);
      // 刷新后要写回内存：/v3 随后用 getProfile() 取 token 建链，否则会拿旧 token。
      this.profile = profile;
      await saveWebProfile(this.options.store, profile);
      return filterSelf(await this.listDevices(profile.serverUrl, profile.accessToken!, fetchImpl));
    }
  }

  async refreshTokens(): Promise<WebDeviceProfile> {
    const fetchImpl = this.options.fetchImpl ?? fetch;
    const next = await this.refresh(await this.ensureProfile(), fetchImpl);
    await saveWebProfile(this.options.store, next);
    this.profile = next;
    return next;
  }

  /** 退出登录但保留设备身份与账号记录，便于在 `/v3` 重新选择该账号。 */
  async signOut(): Promise<void> {
    await clearWebAuth(this.options.store, this.options.serverUrl);
    this.profile = null;
  }

  async logout(): Promise<void> {
    const current =
      this.profile ??
      (this.options.serverUrl
        ? await loadOrCreateWebProfile(this.options.store, {
            serverUrl: this.options.serverUrl,
          })
        : null);
    if (current) await deleteWebProfile(this.options.store, current.serverUrl);
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

  /** device token 请求：401 刷新一次再重试；其它非 2xx 抛 `WebApiError`。 */
  private async deviceRequest(
    path: string,
    init: { method?: string; body?: unknown } = {},
  ): Promise<Record<string, unknown>> {
    const fetchImpl = this.options.fetchImpl ?? fetch;
    let profile = await this.ensureProfile();
    if (!profile.accessToken) throw new Error("Web 设备尚未登录");
    const send = async (accessToken: string) => {
      const response = await fetchImpl(`${profile.serverUrl}${path}`, {
        method: init.method ?? "GET",
        headers: {
          "content-type": "application/json",
          authorization: `Bearer ${accessToken}`,
        },
        ...(init.body === undefined ? {} : { body: JSON.stringify(init.body) }),
      });
      return { response, body: await readWebJson(response) };
    };
    let result = await send(profile.accessToken);
    if (result.response.status === 401) {
      profile = await this.refresh(profile, fetchImpl);
      await saveWebProfile(this.options.store, profile);
      this.profile = profile;
      result = await send(profile.accessToken!);
    }
    if (!result.response.ok) {
      throw new WebApiError(result.response.status, readWebError(result.body, "配对请求失败"));
    }
    return result.body;
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
    if (!response.ok)
      throw new WebApiError(response.status, readWebError(body, "读取设备目录失败"));
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
          identityKey: typeof device.identityKey === "string" ? device.identityKey : "",
          online: item.online === true,
          addresses: Array.isArray(item.addresses)
            ? item.addresses.filter((value): value is string => typeof value === "string")
            : [],
        },
      ];
    });
  }
}
