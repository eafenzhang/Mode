import type { WebCredentialStore, WebDeviceProfile } from "./accountTypes.js";
import { exportWebIdentity, generateWebIdentity } from "./identity.js";

/** 旧版单账号 key：仅用于迁移，读取后立即搬到按服务地址分键的槽位。 */
export const PROFILE_KEY = "mode.remote.v3.web-profile";
const PROFILE_KEY_PREFIX = "mode.remote.v3.web-profile:";
const ACCOUNT_INDEX_KEY = "mode.remote.v3.web-accounts";

/** 已保存账号的最小投影（设备私钥/token 不进索引）。 */
export interface WebAccountSummary {
  serverUrl: string;
  deviceId: string;
  deviceName: string;
}

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

/** 账号槽位 key：`profile.serverUrl` 在写入前都已规范化。 */
export function webProfileKey(serverUrl: string): string {
  return `${PROFILE_KEY_PREFIX}${serverUrl}`;
}

async function readAccountIndex(store: WebCredentialStore): Promise<WebAccountSummary[]> {
  const raw = await store.get(ACCOUNT_INDEX_KEY);
  if (!raw) return [];
  try {
    const value = JSON.parse(raw) as unknown;
    if (!Array.isArray(value)) return [];
    return value.flatMap((entry) => {
      if (!entry || typeof entry !== "object") return [];
      const item = entry as Partial<WebAccountSummary>;
      return typeof item.serverUrl === "string" &&
        typeof item.deviceId === "string" &&
        typeof item.deviceName === "string"
        ? [{ serverUrl: item.serverUrl, deviceId: item.deviceId, deviceName: item.deviceName }]
        : [];
    });
  } catch {
    return [];
  }
}

async function writeAccountIndex(
  store: WebCredentialStore,
  entries: WebAccountSummary[],
): Promise<void> {
  const sorted = [...entries].sort((left, right) => left.serverUrl.localeCompare(right.serverUrl));
  await store.set(ACCOUNT_INDEX_KEY, JSON.stringify(sorted));
}

async function upsertAccountIndex(
  store: WebCredentialStore,
  profile: WebDeviceProfile,
): Promise<void> {
  const entries = await readAccountIndex(store);
  const next = entries.filter((entry) => entry.serverUrl !== profile.serverUrl);
  next.push({
    serverUrl: profile.serverUrl,
    deviceId: profile.deviceId,
    deviceName: profile.deviceName,
  });
  await writeAccountIndex(store, next);
}

/** 旧版单账号 key → 按 serverUrl 分键；只迁移一次，失败保持旧值不动。 */
async function migrateLegacyProfile(store: WebCredentialStore): Promise<void> {
  const legacyRaw = await store.get(PROFILE_KEY);
  if (!legacyRaw) return;
  const parsed = parseWebProfile(legacyRaw);
  if (!parsed) {
    await store.delete(PROFILE_KEY);
    return;
  }
  const key = webProfileKey(parsed.serverUrl);
  if (!(await store.get(key))) {
    await store.set(key, legacyRaw);
  }
  await upsertAccountIndex(store, parsed as WebDeviceProfile);
  await store.delete(PROFILE_KEY);
}

/** 已保存账号列表（按服务地址排序，不含任何凭据）。 */
export async function listWebProfiles(store: WebCredentialStore): Promise<WebAccountSummary[]> {
  await migrateLegacyProfile(store);
  return await readAccountIndex(store);
}

export async function saveWebProfile(
  store: WebCredentialStore,
  profile: WebDeviceProfile,
): Promise<void> {
  const serverUrl = normalizeWebServerUrl(profile.serverUrl);
  const next = { ...profile, serverUrl };
  await store.set(webProfileKey(serverUrl), JSON.stringify(next));
  await upsertAccountIndex(store, next);
}

/** 删除单个账号（退出即删除；重新登录会生成新身份）。 */
export async function deleteWebProfile(
  store: WebCredentialStore,
  serverUrl: string,
): Promise<void> {
  const normalized = normalizeWebServerUrl(serverUrl);
  await store.delete(webProfileKey(normalized));
  const entries = await readAccountIndex(store);
  await writeAccountIndex(
    store,
    entries.filter((entry) => entry.serverUrl !== normalized),
  );
}

export async function clearWebAuth(
  store: WebCredentialStore,
  serverUrl?: string,
): Promise<WebDeviceProfile | null> {
  await migrateLegacyProfile(store);
  const target =
    serverUrl !== undefined
      ? normalizeWebServerUrl(serverUrl)
      : (await readAccountIndex(store))[0]?.serverUrl;
  if (!target) return null;
  const raw = await store.get(webProfileKey(target));
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
  await migrateLegacyProfile(store);
  const requested = options.serverUrl ? normalizeWebServerUrl(options.serverUrl) : undefined;
  // 未指定服务地址时取索引里的第一个（按 URL 排序）作为当前账号。
  const serverUrl = requested ?? (await readAccountIndex(store))[0]?.serverUrl;
  if (!serverUrl) throw new Error("远程控制 v3 服务地址未配置");
  const raw = await store.get(webProfileKey(serverUrl));
  const parsed = raw ? parseWebProfile(raw) : null;
  if (parsed && parsed.serverUrl === serverUrl) {
    const profile = {
      ...parsed,
      ...(options.deviceName ? { deviceName: options.deviceName } : {}),
    };
    if (options.deviceName) await saveWebProfile(store, profile);
    return profile;
  }
  // 新服务地址：只生成该账号的设备身份，不影响其他已保存账号。
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
