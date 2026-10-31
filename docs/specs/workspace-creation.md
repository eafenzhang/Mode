# Spec：空态工作区菜单「创建工作区」

## 1. 背景

0.0.93 安装版实测：空态输入框的工作区胶囊菜单只有「打开文件夹 / 远程连接 /
不在项目中工作」，没有原版（ZCode）固定组第一项的「创建项目」入口。仓库里已有
`fileService.createScratchWorkspace`、名称校验与位置提示
（`ChatEmptyScratchWorkspaceDialog.tsx` 的 helper）和完整 i18n 文案，但没有任何
UI 调用方——属于「服务与文案就绪、入口缺失」的断链。

本 spec 定义入口、对话窗口、所有者与失败语义。术语沿用本仓库的「工作区」，不改成
原版的「项目」。

## 2. 对齐口径

- **入口位置**：空态工作区菜单固定组的第一项，`FolderPlus` 图标，位于「打开文件夹」
  之前；与 `DropdownMenuItem` 的普通动作行同高、同内边距。
- **选中项指示**：仅这个菜单把勾选标记放到行尾（原版格式），其它菜单仍用默认的前置
  标记。通过 `DropdownMenuCheckboxItem` 的可选 `indicatorPosition` 实现，不改全局默认。
- **「打开文件夹」保留**：该入口继续选择已有目录，与「创建工作区」并存。原版把
  「选择源文件夹」合进创建对话；本仓库没有「命名已有目录」的产品模型，因此不伪造
  「源文件夹」，已有目录继续走既有打开路径。
- **设备选择不在本 spec**：远程工作区由「远程连接」创建与管理；本地创建固定落在
  本机 `~/ModeProject`，不提供无实际分支的「设备」下拉框。
- **侧边栏「项目 +」菜单同步**：同一入口追加到侧边栏项目分区的「+」菜单，仍位于
  「打开文件夹」之前、`FolderPlus` 图标，与空态菜单共用 `ChatEmptyScratchWorkspaceDialog`
  和 `handleCreateScratchWorkspace`（2026-10-28 补齐；此前 spec 明确暂不覆盖）。

## 3. 所有者与写入路径

- 唯一写入者：Root 的 `handleCreateScratchWorkspace`（`useRootWorkspaceActions`），
  经 `IFileService.createScratchWorkspace` 调 Host 侧 `fileService`。
- 渲染层只提交名称，**不直接** `mkdir`，也不拼绝对路径。
- 工作区根 `~/ModeProject` 由 `fileService` 的 `SCRATCH_WORKSPACE_ROOT_NAME` 定义。
  UI 的位置提示 `getScratchWorkspaceLocationHint(name)` 只负责展示同一路径，不参与写入。
- 创建成功后的打开动作复用 `handleSelectProject(path)`：加 tab、清空该 workspace 的
  `activeTaskId`、更新 `recentProjects`。与「打开文件夹」共享同一条路径，不新增第二套
  选择逻辑。

## 4. 事件顺序

```
菜单选择「创建工作区」
  → 菜单关闭（DropdownMenu 默认关闭流程）
  → 创建对话框打开（名称清空、错误清空）
  → 提交
      → 名称校验：空 / 含 "/" 或 "\" 就地报错，不创建
      → createScratchWorkspace({ name })（mkdir -p，已存在目录幂等成功）
      → handleSelectProject(path)
      → 对话框关闭
```

取消、按 Esc、点遮罩：不创建目录、不切换工作区。

## 5. 失败语义

| 情况 | 表现 |
| --- | --- |
| 名称为空 | 对话框内显示「工作区名称不能为空。」，不关闭 |
| 名称含 `/` 或 `\` | 显示「工作区名称不能包含 / 或 \。」，不关闭 |
| service 抛错 | 显示「创建工作区失败。」，保留输入；错误写 `logger.error` |
| 当前模式不允许打开工作区（返回 `null`） | 同「创建失败」，不伪装成功 |
| 同名再次创建 | 幂等：目录已存在也成功，切回同一 workspace，不产生重复 tab |

提交期间禁用输入、取消与创建按钮，避免重复提交；关闭对话框的请求在提交完成前被忽略。

## 6. 可见性

- 只有调用方提供 `onCreateScratchWorkspace` 时才渲染入口。设置页复用同一个工作区菜单
  但没有创建能力（当前传空回调/不传），因此不显示该入口。
- 侧边栏「项目 +」菜单由 `WorkspaceShellLayout` 注入同一 `onCreateScratchWorkspace`；
  空态与侧边栏共享同一个对话框组件与提交路径，不新增第二套创建逻辑。
- 本地与远程工作区都显示入口：创建目标固定为本机 `~/ModeProject`，与 service 语义一致。

## 7. 验收

1. 空态菜单出现「创建工作区」，在「打开文件夹」之上，图标为 `FolderPlus`。
2. 侧边栏项目分区「+」菜单出现同一入口，顺序一致；创建成功后打开新工作区并关闭对话框。
2. 输入 `demo` 创建后：`~/ModeProject/demo` 存在、tab 与胶囊切到该 workspace、对话框关闭。
3. 名称为空或含斜杠：就地报错且不产生目录；取消不产生目录。
4. 同名再次创建：幂等成功并切回该 workspace。
5. zh-CN / en-US 两套文案齐全；使用逻辑属性，不写死左右。
6. 本菜单选中项勾选标记在行尾；其它菜单的选中态不变。
