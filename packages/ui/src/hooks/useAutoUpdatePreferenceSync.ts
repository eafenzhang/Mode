import { useEffect } from "react";

import { usePlatform } from "./usePlatform.js";
import { useSettings } from "./useSettingService.js";

/**
 * 把 Host 持久化的「自动下载并安装更新」偏好同步到 Tauri updater 的运行期缓存。
 *
 * `setting.json.autoDownloadAndInstallUpdates` 是唯一持久化真源；Rust 的
 * `UpdateManager.auto_download` 只是进程内缓存，重启后为 false。应用启动与设置变化时
 * 都必须把 Host 的值推过去，否则 `check_for_update` 永远看到 false，仍要求手动点下载。
 *
 * 该 hook 只在主应用（有 ServiceProvider）挂载；独立更新窗口没有 setting service，
 * 继续走 `platform.getAutoUpdatePreferences` 的本地缓存。
 */
export function useAutoUpdatePreferenceSync(): void {
  const platform = usePlatform();
  const { settings } = useSettings();
  const enabled = settings?.autoDownloadAndInstallUpdates;

  useEffect(() => {
    if (typeof enabled !== "boolean") {
      return;
    }
    void platform.setAutoDownloadAndInstallUpdates?.(enabled);
  }, [enabled, platform]);
}
