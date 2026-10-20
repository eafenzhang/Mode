# Tauri 更新检查与自动下载

状态：已实现（2026-10-20）。适用范围：Tauri 桌面壳的 `checkForUpdates`、`downloadUpdate`、
`setAutoDownloadAndInstallUpdates` 与主窗口更新弹窗。

## 目标

对齐原 Electron 线的更新体验：用户在设置里开启「自动下载并安装更新」后，检查发现新版本
就直接进入下载，不再停在 `update-available` 等用户点「下载更新」。

## 所有权

| 事实 | 所有者 |
| --- | --- |
| `autoDownloadAndInstallUpdates` 持久化值 | Host `setting.json`（设置页与更新弹窗都经同一 API 写入） |
| Rust 进程内的 `auto_download` | `UpdateManager` 运行期缓存，不是独立真源；启动/设置变化时由 UI 同步 |
| 检查互斥 | `UpdateManager.checking` |
| 下载互斥 | `UpdateManager.downloading`（新增，单飞） |
| 跳过版本 | `UpdateManager.skipped_version`（仅自动检查消费） |

## 事件顺序

```text
应用启动 / 设置页切换
  └─ useAutoUpdatePreferenceSync：Host setting → platform.setAutoDownloadAndInstallUpdates
       └─ Rust auto_download 更新

手动检查（help → 检查更新）
  └─ check_for_update：
       1. 清掉 skipped_version（原版语义：手动检查重新展示被跳过的版本）
       2. checking 互斥 → updater.check()
       3. 命中新版本 → update-available（renderer 弹窗）
       4. auto_download=true → spawn download_update（downloading 互斥）
            → download-progress → update-downloaded
       5. auto_download=false → 等用户点击「下载更新」

自动检查（startup / 后续轮询，含 check_and_install）
  └─ 命中 skipped_version 时直接返回 skipped，不广播 update-available、不下载
```

## 失败与边界

- `download_update` 并发调用返回 `{ status: "busy" }`；取消/失败/成功都会复位 `downloading`。
- 自动下载失败时保持 `update-available`，用户仍可手动重试。
- 独立更新窗口没有 Host setting service，继续使用 `platform` 的运行期偏好（旧行为）。
- 手动检查清 skip 是刻意的原版语义；自动检查才尊重 skip。不要把手动入口改成永久跳过，
  否则用户无法再看到该版本。

## 验收

- 开启偏好后检查发现新版本：无需点击，状态自然进入 `download-progress` 并最终
  `update-downloaded`。
- 关闭偏好：保持 `update-available`，手动按钮仍可下载。
- 偏好写入 Host 后重启应用：启动同步 hook 把值推给 updater，不需要重新打开设置页。
- 手动检查跳过过的版本：仍会重新出现；自动检查不会。
- 并发触发下载（自动 + 手动按钮）只跑一轮。
