import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

// Tauri 更新偏好与自动下载（docs/specs/tauri-updates.md）：
// Host setting 是持久化真源，启动/设置变化同步到 updater；检查命中后由 Rust 自动下载。
// packages/ui 没有 DOM 环境，沿用源码契约测试。

const appSource = await readFile(new URL("../src/App.tsx", import.meta.url), "utf8");
const buttonSource = await readFile(new URL("../src/UpdateStatusButton.tsx", import.meta.url), "utf8");
const controllerSource = await readFile(
  new URL("../src/UpdateStatusDialogController.tsx", import.meta.url),
  "utf8",
);
const syncHookSource = await readFile(
  new URL("../src/hooks/useAutoUpdatePreferenceSync.ts", import.meta.url),
  "utf8",
);

test("启动/设置变化：Host 偏好同步到 updater 运行期缓存", () => {
  assert.ok(
    syncHookSource.includes("platform.setAutoDownloadAndInstallUpdates?.(enabled)"),
    "同步 hook 必须调用 platform.setAutoDownloadAndInstallUpdates",
  );
  assert.ok(
    syncHookSource.includes("settings?.autoDownloadAndInstallUpdates"),
    "同步值必须来自 Host 设置",
  );
  assert.ok(
    appSource.includes("useAutoUpdatePreferenceSync()"),
    "主应用启动时必须挂载同步 hook",
  );
});

test("更新弹窗的自动下载勾选写回 Host 设置，而不是只写运行期缓存", () => {
  assert.ok(
    buttonSource.includes("updateAppSettings({ autoDownloadAndInstallUpdates: enabled })"),
    "弹窗勾选必须持久化到 Host setting.json",
  );
  assert.ok(
    buttonSource.includes("autoDownloadAndInstallUpdates={appSettings?.autoDownloadAndInstallUpdates === true}"),
    "弹窗显示值必须来自 Host 设置",
  );
  assert.ok(
    controllerSource.includes("if (onAutoDownloadAndInstallUpdatesChange)"),
    "controller 必须优先走宿主提供的持久化回调",
  );
  assert.ok(
    controllerSource.includes("await platform.setAutoDownloadAndInstallUpdates?.(enabled)"),
    "独立更新窗口仍保留 platform 运行期缓存兜底",
  );
});
