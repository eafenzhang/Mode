# 运行态验收修复执行记录（2026-10-20）

> 对应 spec：`docs/specs/runtime-ui-acceptance-fixes.md`。设备名、IP 与一次性配对码不在本文记录。

## 根因与修复

1. **本地工作区右上角无资源管理器入口**：Tauri 注入层把 `getInstalledEditors` / `openInEditor`
   放在 `REJECTING_REQUIRED`，`WorkspaceEditorButtonGroup` 拿不到任何编辑器，整颗「打开方式」
   胶囊返回 null。修复：注入层与 `@mode/ui/tauri-platform` shim 实现最小文件管理器面
   （Windows `explorer`、macOS `finder`、其它 `xdg-open`），`openInEditor` 委托
   `openInFileManager`，未知 id / 远端目标失败关闭。
2. **菜单二级菜单悬停不切换**：每个子菜单各自维护 open，指针进入另一个子菜单时旧子菜单不关。
   修复：根菜单维护唯一 `activeSubmenuId`，子菜单指针进入时互斥关闭兄弟子菜单；补纯函数
   判据 `isSiblingSubmenuActive` 与单测。
3. **机器人页面 UI 错位**：`SettingsRow` 控制列固定 192/280px 且外层 `flex-nowrap`，宽控制
   向右溢出，`BotsDialog` 右栏出现横向滚动。修复：控制列 `min-w-0 + flex-wrap`，右栏
   `overflow-x-hidden`。
4. **Web / 手机远控入口缺失**：弹层原先只有 IM Bot Channel；footer 入口还只认
   `workspacePath`。修复：弹层新增「浏览器 / 手机」卡片（LAN 地址、二维码、一次性配对码、
   去设置开启），footer 条件改为 `workspacePath || workspaceIdentity`。
5. **远端工作区无法选择模型**：首次 `modelSelectionService.getView()` 加载期间工具栏仍渲染
   禁用态「选择模型」。真机观察该状态最终恢复为可用模型（`MiDeepSeek/deepseek-flash`）。
   修复：`loading` 时显示「正在加载模型…」+ spinner；不改变远端模型事实所有者，不回退本地
   模型列表。
6. **远程桌面「对端不支持」**：设备能力探测通过，但会话 POST 响应缺少 `mediaTicket`；
   控制器把它固定映射成 `unsupported`。修复：失败分支读取对端 `server-info.version` 并写入
   诊断错误；UI 的 `unsupported` 文案明确提示检查对端「允许被远程控制」、版本与重启。
   不改协议、不做旧版媒体面回退。

## 改动文件

- `crates/sidecar-host/src/window_mode.ts`
- `packages/ui/src/platform/tauri-invoke-shim.ts`
- `packages/ui/src/ui-foundation/adapters/{dropdown-menu,dropdown-submenu,dropdownMenuContext,context-menu}.tsx|ts`
- `packages/ui/src/settings/SettingsPageParts.tsx`
- `packages/ui/src/BotsDialog.tsx`
- `packages/ui/src/WebRemoteControlDialog.tsx`
- `packages/ui/src/WorkspaceSidebarFooter.tsx`
- `packages/ui/src/v4/composer/V4ComposerToolbar.tsx`
- `packages/ui/src/lib/webRemoteEntry.ts`（新增）
- `packages/ui/src/i18n/locales/{zh-CN,en-US,fa}.ts`
- `packages/desktop/src/host/remoteDesktopPeerClient.ts`
- `apps/desktop-tauri/test/windowModeInjection.test.mts`
- 新增测试：`packages/ui/test/{webRemoteEntry,botsDialogLayout}.test.ts`，
  扩展 `uiFoundationDropdownMenuAdapter.test.ts`、`tauriInvokeShim.test.ts`
- 新增 spec：`docs/specs/runtime-ui-acceptance-fixes.md`

## 验证

- `pnpm typecheck`：通过。
- `pnpm --filter @mode/ui test`：**415 passed / 0 failed**。
- `pnpm --filter @mode/desktop-tauri test`：**38 passed / 7 skipped / 0 failed**。
- `pnpm exec oxlint <changed files>`：**0 error**。
- `pnpm architecture:check --changed`：**0 violations / 0 new**。
- `pnpm --filter @mode/desktop-tauri run build:package-inputs`：通过（产品 UI + Host bundle）。
- 本地 NSIS 打包：首次误走 MSVC（本机无 VS Build Tools）失败；改用 gnu 工具链装配重试成功，
  产物 `dist/tauri/Mode-0.0.62-audit.20261020-win-x64.exe`（51.79 MiB）与
  `.exe.sig`。这是本机 gnu 构建，未执行 Authenticode；两端手工安装同一文件即可复验。
- `pnpm fmt:check` 的根命令仍会被未跟踪的 `.plan/job.html` 阻塞；本轮改动文件已用
  `oxfmt` 单独格式化。

### 新构建真机复验（0.0.62-audit.20261020）

已在本机静默安装并启动新构建，逐项看屏：

- 本地工作区右上角出现「打开方式」胶囊；可访问到
  `button 在 资源管理器 中打开` 与 `image 资源管理器`（问题 5）。
- 「移动端远程控制」弹层出现「浏览器 / 手机」卡片：访问地址
  `http://<LAN 地址>:45880/`、二维码、配对码与「生成配对码」按钮（问题 4）。
- 机器人管理弹层右栏不再出现横向滚动条，控制列在 192px 列内换行，内容不再被截断（问题 3）。
- 远端任务在上一构建中观察到的模型选择器最终恢复为 `MiDeepSeek/deepseek-flash`；
  新构建在加载期显示「正在加载模型…」，不再用禁用态「选择模型」误导（问题 2）。

### 2026-10-20 追加修复（0.0.63）

- **资源管理器图标**：Tauri 不再注入自造黄色 SVG，`getInstalledEditors` 返回空
  `iconDataUrl`；本地「打开方式」胶囊与远端 `WorkspaceFileManagerButton` 统一回退到既有
  `FolderOpenIcon`，两端同一枚原版图标。
- **已有远端对话切模型**：`SessionPane.handleSelectModel` 在已有会话上除更新 draft 外，
  额外经同一 `modeSessionService` 发 `switchModelConfig` 配置 CAS；远端 workspace 因此
  打到对端 Host。CAS 被拒只告警，draft 保留，下一条消息仍携带用户选择。
- **移动端远控原版双栏**：恢复左栏「手机扫码连接」（状态/二维码/地址与配对码/刷新/复制/
  停止）与右栏「使用 Bot Channel」；保留上一轮的 LAN 地址、配对码、二维码和“去设置开启”。
- 追加修复的验证：`pnpm typecheck` 通过；`@mode/ui` 415 passed；desktop-tauri 38 passed /
  7 skipped；改动文件 `oxlint` 0 error；`architecture:check --changed` 0 违规。
- 0.0.63 安装包：`dist/tauri/Mode-0.0.63-audit.20261020-win-x64.exe`，已在本机静默安装并启动。
  真机看屏确认：本地右上角恢复原版 `FolderOpenIcon` 资源管理器胶囊；移动端弹层恢复
  原版双栏（左「手机扫码连接」二维码/状态/刷新/复制，右「使用 Bot Channel」）。
- 远端已有对话切模型的端到端复验受当前环境限制：本机 `gas-hub-xp` 对端处于
  「重新连接」状态，无法起真实远端 session；CAS 代码路径经 typecheck/架构门禁，
  需两端装 0.0.63 并连上后复验。

## 远端问题的实际结论

- 远端模型选择器在加载完成后可用；问题在于首次加载窗口缺少状态提示，不是永久不可选。
  已有会话现在还会立即发 `switchModelConfig` CAS，不再只等下一次提交。
- 远程桌面失败指向对端缺少媒体票据（版本/进程不一致），不是本机能力位探测错误。
  本机同时装有旧 Electron `Mode 0.0.52` 与新 Tauri 线（已升级到
  `0.0.63-audit.20261020`）；验收前需确认对端启动的是 Tauri 线、安装同一新构建并重启。
