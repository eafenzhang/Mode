import type { WebCredentialStore, WebDeviceProfile } from "./accountTypes.js";
import { exportWebIdentity, generateWebIdentity } from "./identity.js";

export const PROFILE_KEY = "mode.remote.v3.web-profile";

interface WebProfileShape {
  serverUrl: string;
  deviceId: string;
  deviceName: string;
  privateKey: string;
  accessToken: string | null;
  refreshToken: string | null;
}

export function parseWebProfile(raw: string): WebProfileShape | null {
  try {
    const value = JSON.parse(raw) as Partial<WebProfileShape>;
    if (
      typeof value.serverUrl !== "string" ||
      typeof value.deviceId !== "string" ||
      typeof value.deviceName !== "string" ||
      typeof value.privateKey !== "string"
    ) {
      return null;
    }
    return {
      serverUrl: value.serverUrl,
      deviceId: value.deviceId,
      deviceName: value.deviceName,
      privateKey: value.privateKey,
      accessToken: typeof value.accessToken === "string" ? value.accessToken : null,
      refreshToken: typeof value.refreshToken === "string" ? value.refreshToken : null,
    };
  } catch {
    return null;
  }
}

export async function readWebJson(response: Response): Promise<Record<string, unknown>> {
  try {
    const value = (await response.json()) as unknown;
    return value && typeof value === "object" ? (value as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

export function readWebError(body: Record<string, unknown>, fallback: string): string {
  const error = body.error as { message?: unknown } | undefined;
  return typeof error?.message === "string" && error.message ? error.message : fallback;
}

export function webRandomUUID(): string {
  const cryptoApi = globalThis.crypto;
  if (cryptoApi && typeof cryptoApi.randomUUID === "function") return cryptoApi.randomUUID();
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
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
