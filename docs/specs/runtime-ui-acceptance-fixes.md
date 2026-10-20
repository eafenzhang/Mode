# 运行态验收修复（2026-10-20）

本 spec 记录 v0.0.61 真机验收发现的六项问题、根因与修复口径。实现必须遵守既有
`docs/specs/remote-workspace-file-manager-entry.md`、`docs/specs/lan-remote-desktop.md`、
`docs/specs/in-app-remote-desktop.md` 与 `docs/specs/lan-paired-devices.md` 的所有者与边界，
不新增第二套状态。

## 1. 本地工作区右上角资源管理器入口

**根因**：本地入口由「打开方式」胶囊承担；胶囊调用 `getInstalledEditors()`。Tauri 注入层把
`getInstalledEditors` / `openInEditor` 列为拒绝桩，胶囊拿不到任何编辑器（含 `explorer`），
于是整颗胶囊不渲染，右上角没有任何资源管理器入口。

**修复口径**：

- Tauri 注入层实现 `getInstalledEditors()`：返回当前平台的系统文件管理器条目
  （Windows `explorer`、macOS `finder`、其它 POSIX `xdg-open`）。`iconDataUrl` **留空**：
  Tauri 拿不到系统图标位图，由 UI 回退到既有 `FolderOpenIcon`，本地与远端工作区使用同一枚
  原版图标，不注入自造 SVG。这是最小可用面，不伪造未安装的编辑器。
- `openInEditor(editorId, path)`：文件管理器 id 委托壳命令 `openInFileManager`，返回
  `{ success, error? }`；未知 id 失败关闭，不静默成功。
- 本地行为仍由「打开方式」胶囊承担；`WorkspaceFileManagerButton` 的本地隐藏规则不变。
- 远端工作区继续走既有文件管理器入口，不因本修复改变。

## 2. 菜单二级菜单悬停内容不更新

**根因**：`DropdownMenuSub` 的每个子菜单各自维护 open 状态，`DropdownMenuSubTrigger` 的
`onPointerEnter` 只打开自己，不关闭兄弟子菜单。鼠标从一个 provider 移到另一个时，旧子菜单
仍然打开并覆盖新内容，表现为「悬停不切换」。

**修复口径**：

- `DropdownMenu` 根上下文维护唯一的 `activeSubmenuId`。
- 子菜单指针进入时把自己设为 active；其它子菜单在 active 变化时关闭。
- 根菜单关闭时清空 active；受控 `open` 语义保持。
- 只影响子菜单互斥，不改变点击选择、键盘导航与 portal 外部点击判定。

## 3. 机器人页面 UI 错位

**根因**：`BotsDialog` 右栏是 `flex-1` 可滚动区，内部 `SettingsRow` 的控制列固定 192px 且
外层 `flex-nowrap`；部分控制（渠道状态徽标 + 移除密钥、绑定码等）宽于该列时向右溢出，
形成右栏横向滚动与内容截断。

**修复口径**：

- `SettingsRow` 控制列允许 `min-w-0` + `flex-wrap`，控制内容在固定列内换行，不再撑破网格。
- `BotsDialog` 右栏显式 `overflow-x-hidden`，纵向滚动保持。
- 不改变各卡片的字段、顺序和设置写入路径。

## 4. Web / 手机远控入口

**根因**：侧边栏 footer 只有「移动端远程控制」弹层入口，弹层内只有 IM Bot Channel，
没有浏览器/手机 Web 远控的地址与配对入口；远端工作区只有 `workspaceIdentity` 时，
footer 的 `workspacePath` 条件还可能把入口隐藏。

**修复口径**：

- footer 入口条件改为 `isDesktop && (workspacePath || workspaceIdentity)`；
  `workspacePath` 缺失时传空串，不改变弹层契约。
- `WebRemoteControlDialog` 增加「浏览器 / 手机」卡片：
  - 读取 `getLanAccessState()`，展示局域网监听地址与端口；
  - 生成并展示一次性配对码（复用 `createLanAccessPairCode()`）；
  - 为 Web 入口地址生成二维码（`qrcode`，纯展示，不新增网络请求）；
  - LAN 访问未开启时给出「去设置开启」入口，跳转既有远程控制设置分区。
- 地址、配对码的唯一所有者仍是 Host 的 LAN 服务；弹层只读快照与调用既有动作，不持久化。

## 5. 远程工作区模型选择加载态

**根因**：远端 `modelSelectionService.getView()` 首次调用可能持续数秒到一分钟；在此期间
`modelSelectionState.status === "loading"`，工具栏仍渲染禁用态的「选择模型」按钮，
用户无法判断是在加载还是不可用。服务就绪后选择器会恢复（真机已观察到恢复为可用模型）。

**修复口径**：

- `V4ComposerToolbar` 在 `loading` 时渲染明确的「正在加载模型…」禁用态（带 spinner），
  不显示可误解的「选择模型」占位；`unavailable` 继续保持「等待远程模型」。
- 不改变远端模型事实的所有者与 `remote-waiting` 门禁，不在加载期回退本地模型列表。
- 加载完成后按既有路径启用选择器；不新增超时掩盖真实同步问题。

## 6. 远程桌面「对端不支持」诊断

**根因**：设备能力探测通过（`remoteDesktop` + `remoteDesktopNative` 为真），但会话 POST
返回的响应缺少 `mediaTicket`；控制器把这条固定映射为 `unsupported`，UI 只显示
「对端不支持远程桌面」，丢掉了「缺少媒体票据（版本过旧）」的具体原因。

**修复口径**：

- `openRemoteDesktopPeerSession` 在缺少 `mediaTicket` 时保留稳定错误码 `unsupported`，
  但同时返回可读的版本诊断（在失败分支读取对端 `server-info.version`，不改成功路径）。
- UI 的 `unsupported` 文案明确提示检查对端「允许被远程控制」开关、版本与重启；
  Host 返回的版本诊断保留在错误字符串里供日志/后续展示，用户据此能区分
  「对端未开启」与「对端版本/进程过旧」。
- 能力位探测与媒体票据的判定口径不变；不改协议，不做旧版媒体面回退。

## 7. 已有远端对话切换模型

**根因**：`handleSelectModel` 只更新 composer draft（renderer intent），不向已有 session 发
配置命令；对远端已有对话，用户看到菜单可选但会话选择不生效。草稿值还会被
`modelSelectionView.effectiveSelection` 覆盖，表现为“切了没反应”。

**修复口径**：

- 已有会话（`sessionId` 非空）：更新 draft intent 后，额外经同一 `modeSessionService`
  发 `switchModelConfig` 配置 CAS（`provider/model/thought`）。远端 workspace 因此打到对端
  Host，不使用本地回退。
- 草稿/新任务没有可切会话，仍只保存 draft，由首条 Submission 携带。
- CAS 失败只告警并保留 draft；下一条消息仍携带用户选择，不把配置写失败伪装成成功。

## 8. 移动端远控原版双栏布局

**根因**：上一轮为补 Web/手机入口把弹层改成了单栏卡片堆叠，与用户认可的原版布局不一致。

**修复口径**：

- 恢复原版双栏：左栏「手机扫码连接」（状态、二维码、地址/配对码、刷新二维码、复制链接、
  停止），右栏「使用 Bot Channel」（渠道列表 + 机器人管理）。
- 保留上一轮新增的 LAN 地址、一次性配对码、二维码和“去设置开启”能力；数据仍只读
  Host 的 LAN 快照，不新建状态。
- 弹层宽度按双栏放大；窄屏仍按单列回退。
