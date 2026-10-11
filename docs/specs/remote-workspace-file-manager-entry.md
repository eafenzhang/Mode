# 远程工作区右上角文件管理器入口

## 背景

工作区头部右上角（`WorkspaceHeaderActionSection`）的「打开方式」胶囊就是本地工作区的文件管理器入口：`sortInstalledEditorsForOpenWith` 把 `finder` / `qspace` / `explorer` 排在最前，所以本地默认主按钮即系统文件管理器。远端工作区拿不到这个入口：

- `resolveWorkspaceEditorSelection`（`packages/ui/src/lib/workspaceEditorSelection.ts`）对 SSH 只留 `vscode`、WSL 留 `vscode` + `explorer`、Docker/LAN 直接 `availableEditors = []`；
- `availableEditors` 为空时 `WorkspaceEditorButtonGroup` 返回 `null`，于是 SSH/Docker/LAN 的右上角完全没有文件管理器入口。

本 spec 定义远端工作区的对应入口。

## 产品规则

### 范围

- 覆盖全部远端工作区：SSH / WSL / Docker / LAN（判定见「入口可见性」）。
- 本地工作区不显示本入口——本地已由「打开方式」胶囊提供系统文件管理器，重复入口没有价值。
- 不改「打开方式」胶囊自身的编辑器过滤语义（SSH/Docker 不能用本机 App 打开远端路径，这条失败关闭维持现状）。

### 入口可见性

显示条件 = 工作区是远端的，即 `remoteTarget || workspaceIdentity || remoteSessionId` 任一为真；三者全空则不显示（与 `SessionPane` 既有 `remoteWorkspace` 判定同口径）。

### 行为

- 点击打开**内置目录浏览窗口**（`DirectoryBrowser`，只浏览形态），起点 = 当前工作区路径 `workspaceAbsPath`。
- 服务访问器由 `useWorkspaceServicesResolution(workspaceAbsPath, remoteSessionId, workspaceIdentity)` 解析：远端走该远端连接的 `fileService.readdir`，断连时回落到断连代理并给出既有错误文案——不在本组件里新建第二条服务解析路径。
- 只浏览：列目录 + 列文件、可进入子目录与上级目录，**不提供选择与回填**。文件行不参与选中，footer 只有「关闭」。多选回填仍只属于 composer 的「从工作区选择文件」，两条入口不合并。
- 独立于「打开方式」胶囊：SSH 上两者并存（胶囊开 VS Code，本入口开内置浏览）；Docker/LAN 上胶囊为空时本入口单独出现。

### 状态所有者

| 状态 | 所有者 | 说明 |
| --- | --- | --- |
| 浏览窗口开合 | `WorkspaceFileManagerButton` 本地 `useState` | 纯 UI 态，关闭即丢弃 |
| 目录列表 / 当前路径 / 选中集 | `DirectoryBrowser` 内部 state | 既有实现，不外提 |
| 远端服务句柄 | `useWorkspaceServicesResolution` + `remoteWorkspaceSessionStore` | 唯一解析点，复用既有 hook |

### 不变量

1. **不新开服务通道**：入口复用 workspace services 解析，不直连 `window.mode`，不绕过 Host。
2. **不改授权面**：本入口只做目录/文件名浏览，不读文件字节；字节读取仍走附件读路径的权威行授权。
3. **本地行为零改动**：本地工作区看不到本入口，「打开方式」胶囊与 `DirectoryBrowser` 既有选择形态保持不变。

## 事件顺序

```text
Renderer(WorkspaceHeaderActionSection)
 │ 点击「文件管理器」
 ▼
WorkspaceFileManagerButton ── set open ──▶ DirectoryBrowser(browseOnly)
                                              │ useWorkspaceServicesResolution 解析 services
                                              ▼
                                     fileService.readdir(path)  ──▶ 远端 host
                                              │ 列目录/文件，可导航
                                              ▼
 关闭 ── set closed ──▶ 卸载 DirectoryBrowser（不发额外请求）
```

## 验收场景

1. LAN 远端工作区：右上角出现「文件管理器」图标；点击打开浏览窗口，起点为该远端工作区路径；可进入子目录、返回上级；关闭后窗口消失。
2. SSH / Docker 远端工作区：同样出现且可用；SSH 上与 VS Code 胶囊并存，互不影响。
3. 本地工作区：右上角不出现本入口，「打开方式」胶囊行为不变。
4. 只浏览语义：文件行点击不产生选中态，footer 无「选择 N 个文件」，只有「关闭」。
5. 远端断连：窗口仍可打开并显示既有断连错误文案，不白屏、不新建连接。
6. 窄屏远控（`simplifyForNarrowRemote`）：入口不渲染，维持既有头部省宽度的约定。

## 测试

- 单测（入口可见性）：远端三种标识任一为真即显示；三者全空不显示；`simplifyForNarrowRemote` 抑制。
- 单测（只浏览形态）：browseOnly 下 footer 只有关闭、文件行不可选；非 browseOnly 的 file / directory 形态不变。
- 验证：`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`，目标包测试按 `package.json` 实际入口执行并如实报告。

## 不做

- 不给远端提供「在本机系统文件管理器中打开」——SSH/Docker/LAN 的路径在本机不存在；WSL 的 UNC 资源管理器仍由既有 `resolveWorkspaceFileManagerEditor` 覆盖。
- 不把入口塞进帮助菜单 `⋯`，不与 side pane 文件树合并。
- 不改「打开方式」胶囊的编辑器过滤与失败关闭语义。
- 不在浏览窗口里做文件打开 / 下载 / 预览；字节类操作走既有附件读与 artifact 通路。
