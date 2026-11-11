import { loadOrCreateWebProfile, type WebDeviceProfile } from "@mode/remote-core/web";

import { createWebCredentialStore } from "./credentialStore.js";

/** `/v3` 登录页记录的服务地址；主 Web 用它静默恢复同一账号会话。 */
export const V3_SERVER_URL_KEY = "mode.remote.v3.web-server-url";

/**
 * 读取 `/v3` 已保存的账号会话（不发起网络请求）。
 *
 * 没有服务地址或尚未登录时返回 null——主页据此提示先到 `/v3` 登录，而不是在这里
 * 再长一套登录 UI。缺失设备身份时由 `loadOrCreateWebProfile` 生成（与 `/v3` 同一存储键）。
 */
export async function loadStoredV3Profile(): Promise<WebDeviceProfile | null> {
  const serverUrl = localStorage.getItem(V3_SERVER_URL_KEY)?.trim();
  if (!serverUrl) return null;
  const profile = await loadOrCreateWebProfile(createWebCredentialStore(), {
    serverUrl,
    deviceName: "Mode Web",
  });
  return profile.accessToken ? profile : null;
}
