import { useCallback, useEffect, type Dispatch, type SetStateAction } from "react";
import type { WebAccountClient, WebDeviceProfile, WebDeviceSummary } from "@mode/remote-core/web";

/**
 * 从 GitHub 授权页切回 `/v3` 时主动刷新一次登录态与设备目录。
 *
 * 设备流本身有轮询，但浏览器在后台可能节流定时器；focus 刷新保证「授权后返回」
 * 能立即看到登录结果。
 */
export function useV3FocusRefresh(
  client: WebAccountClient | null,
  setProfile: Dispatch<SetStateAction<WebDeviceProfile | null>>,
  setHosts: Dispatch<SetStateAction<WebDeviceSummary[]>>,
): void {
  const refresh = useCallback(async (): Promise<void> => {
    if (!client) return;
    const current = await client.getProfile();
    if (!current.accessToken) return;
    setProfile(current);
    setHosts(await client.listHosts());
  }, [client, setProfile, setHosts]);

  useEffect(() => {
    const onFocus = (): void => {
      void refresh().catch(() => undefined);
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh]);
}
