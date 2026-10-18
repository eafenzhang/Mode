# M4 UI clean-room 替换：范围、顺序与验收（2026-10-18）

> 本文件是 M4「UI 替换」的开工前盘点。**只使用路径、导出面、`DESIGN.md` 与既有交互 spec**；
> `packages/ui` 是上游衍生实现，按 `docs/specs/clean-room-policy.md` **禁止读实现体**。
> 具体落地前还需你对两个口径拍板（见文末「待裁决」）。

## 目标

把存量的上游衍生 renderer 组件替换为按 `DESIGN.md` 重写的 clean-room 实现，
使 `packages/ui` 在重合度指标（`docs/specs/upstream-overlap-metric.md`）里归零，
同时保持功能等价（允许视觉差异，不允许功能差异）。

## D8 裁决（2026-10-19，用户拍板）

| 项 | 决定 | 含义 |
| --- | --- | --- |
| 落点 | **(a) 原地重建 `packages/ui`** | `ui-foundation` 留在 `packages/ui/src/ui-foundation/`；不新建 `ui-next` |
| 切换 | **(a) 逐屏替换 + adapter** | 保留旧导出名，一屏一屏验收后再删除旧实现 |
| 状态层 | 推荐口径 | `store/`、`hooks/` 留在 `packages/ui`，新 UI 只替换渲染层 |
| 旧代码 | **验收通过后删除** | 对应屏验收通过即删除旧组件；不长期双轨 |

**影响**：`@mode/ui` 包名与 `App` 导出保持不变，调用方零改动；`ui-foundation/adapters/`
承接旧 props 形状；每屏的删除动作以该屏的验收记录为前提。

## 当前读数（2026-10-18）

`packages/ui/src` 在重合度指标中占 **32,626 行**（identical 19,312 + normalized 13,314），
占 M4 总残余的约 42%。按子目录：

| 子目录 | 重叠文件 | 重叠行数 | 主要职责 |
| --- | --- | --- | --- |
| `lib/` | 70 | 5,778 | hooks/工具/平台适配，部分不是纯渲染 |
| `components/` | 41 | 4,703 | 基础组件（按钮/输入/卡片/菜单） |
| `settings/` | 48 | 3,402 | 设置页：provider、插件、MCP、hooks、skills |
| `v4/` | 42 | 2,979 | v4 协议投影/任务视图 |
| `hooks/` | 32 | 2,825 | 服务访问与状态订阅 |
| `workspace-file-tree/` | 17 | 1,931 | 工作区文件树 |
| 根目录文件 | 28 | 1,917 | `App` / `Root` / 输入框 / 轨迹视图等 |
| `app-shell/` | 21 | 1,694 | 布局、窗口框架、顶栏 |
| `store/` | 6 | 1,361 | Zustand store（状态所有者） |
| `mentions/` | 13 | 871 | @ 文件/命令提及 |
| `ToolCallBlocks/` | 12 | 789 | 工具调用卡片 |
| `root/` | 11 | 777 | 根路由/面板编排 |
| `browser-use/` | 5 | 549 | 内嵌浏览器面板 |
| `terminal/` | 3 | 441 | 终端面板 |
| `onboarding/` | 8 | 376 | 引导流程 |
| 其余 | — | ~2,000 | preview / git / chat toolbar / 远程连接等 |

`packages/ui/test` 现有 32 个纯逻辑测试（输入映射、远程桌面状态机、插件投影等），
替换期间可继续作为行为基线；**没有端到端 UI 测试**，交互验收仍以人工/截图为主。

**接线映射（2026-10-19）**：旧组件 → `ui-foundation` 的逐项对应、缺口与 adapter 策略见
`docs/plans/2026-10-19-m4-ui-adapter-map.md`。D8 拍板后按该表切换。

## 替换顺序（建议）

按「依赖方向 + 重合行数 + 回归风险」排序，每步都可独立验收：

### P1.1 设计基础（`components/` + 主题 token）

- 依据：`DESIGN.md` 与实现契约 `docs/specs/m4-ui-foundation.md`（token/字号/圆角算法/
  primitive 状态矩阵/验收守卫）；
- 产出：Button/Input/Card/Dialog/Menu/Tabs 等基础件的 clean-room 版本与 Story/示例页；
- 验收：与 `DESIGN.md` 逐条对照；圆角纯函数与 token 守卫用例；现有页面在新基础件上视觉无阻断；
- 风险：`components/ui/*` 可能被大量页面直接引用，替换要保留导出签名。

### P1.2 应用外壳（`app-shell/` + `root/` + 窗口框架）

- 依据：`DESIGN.md`「Workspace layout / Responsive Behavior」与窗口命令面；
- 验收：桌面与手机 Web 的布局断点、窗口控制、侧栏/面板开合；
- 依赖：`packages/ui/src/store/` 的布局状态（不能另起一套状态）。

### P1.3 会话核心（根目录 `App/Root`、输入框、轨迹、工具卡片）

- 范围：`App.tsx`、`Root.tsx`、`LexicalChatInput`、`ModelTrajectory*`、
  `ToolCallBlocks/`、`chat-input-toolbar/`、`mentions/`；
- 依据：`docs/specs/tauri-agent-runtime.md` 的事件流、`bot-*` 交互 spec 的展示口径；
- 验收：流式/工具/审批/排队输入四类场景；现有 `packages/ui/test` 的纯逻辑用例全绿；
- 风险：输入法与 IME、粘贴图片、@ 提及是回归高发区。
- **静态结构已起步（2026-10-19）**：`packages/ui/src/ui-foundation/chatStyles.ts` +
  `Chat.tsx`——消息列表（role=log/aria-live）、四种角色的行对齐与气泡（user 卡片、
  assistant 无底、system 弱化、tool surface+mono）、空状态、流式指示、主输入 shell
  （`rounded-2xl` 例外）与工具栏；用例 `packages/ui/test/uiFoundationChat.test.ts`。
  **工具/审批块（2026-10-19）**：`toolStyles.ts` / `ToolBlock.tsx` / `ApprovalBlock.tsx`——
  scheduled/started/progress/result/error 五态、工具名与正文 mono、等待 confirmation 绿、
  允许/拒绝语义色、操作槽尾部对齐；用例 `packages/ui/test/uiFoundationTool.test.ts`。
  **diff 与排队输入（2026-10-19）**：`diffStyles.ts` / `DiffBlock.tsx` / `QueuedInput.tsx`——
  diff 专用 `diff-added`/`diff-removed`（不借用 success/destructive）、行首标记、排队输入
  紧凑行与尾部操作槽；用例 `packages/ui/test/uiFoundationDiff.test.ts`。
  真实事件接线与输入法/提及仍待后续。

### P1.4 工作区与任务（`workspace-file-tree/`、`workspace-grouped-tasks/`、git 面板）

- 依据：`remote-workspace-file-manager-entry.md`、`remote-workspace-identity.md` 与
  `DESIGN.md` 的列表/树规范；
- 验收：本地/远程工作区、文件树刷新与搜索、任务列表分组与归档动作；
- 注意：`workspaceIdentity` 与路径展示规则不得改动（AGENTS.md）。
- **静态结构已起步（2026-10-19）**：`workspaceStyles.ts` / `Workspace.tsx`——文件树
  （role=tree/treeitem、aria-level/selected/expanded、8+12px 缩进、选中/焦点态）、任务列表
  （行高、选中态、标题截断、元数据弱化）、任务状态语义色与状态点；用例
  `packages/ui/test/uiFoundationWorkspace.test.ts`。真实文件/任务数据与搜索仍待接线。

### P1.5 设置与插件（`settings/`、`v4/`、插件商店）

- 依据：`plugin-store-availability.md`、`plugin-host.md`、`first-party-plugin-attribution.md`、
  `flatten-provider-zones.md`；
- 验收：`plugin-store-availability` 的 6 个场景（卡片禁用、详情固定说明、已安装不受影响）；
- 现状：卡片投影已有 node 侧测试，UI E2E 仍需人工。
- **静态结构已起步（2026-10-19）**：`settingsStyles.ts` / `Settings.tsx`——设置布局/导航/
  分区、插件卡片（brand 背板 `rounded-2xl` 例外、说明两行截断）、不可安装 pill、
  详情固定说明（无重试入口）；用例 `packages/ui/test/uiFoundationSettings.test.ts`。
  数据接线与 6 个 E2E 场景仍待后续。

### P1.6 远程与开发者面板（`remote-desktop/`、`remote-connection/`、`terminal/`、`browser-use/`、`preview*`）

- 依据：`in-app-remote-desktop.md`、`lan-remote-desktop*.md`、`remote-workspace-conversation-mirror.md`；
- 验收：远程桌面输入映射/重连/剪贴板（已有 86 条 UI 纯逻辑测试可复用）、终端与预览面板；
- 风险：这些面板与 Host/relay 的实时链路耦合，替换不得改变消息契约。
- **静态结构已起步（2026-10-19）**：`remoteStyles.ts` / `Remote.tsx`——toolbar/画布/状态条
  三段、图标按钮选中与焦点态、断流覆盖（不透明背景）、终端 mono 面板、预览 header；
  用例 `packages/ui/test/uiFoundationRemote.test.ts`。媒体流与重连接线仍待后续。

### P1.7 零散面与资产

- `onboarding/`、`settings-sync`、`quickpick`、`shortcuts`、`assets/`；
- `assets/`（324 个文件）的来源与等待决策见 `2026-10-18-m4-ui-assets-inventory.md`：
  284 个插件图标（278 个第三方品牌图标台账 `reviewRequired`）、25 个 provider 图标
  （21 个与上游一致、台账无条目）、42 个与上游 identical 的资产需重画/替换。
- **静态结构已起步（2026-10-19）**：`onboardingStyles.ts` / `Onboarding.tsx`（步骤点状态、
  卡片、操作槽）、`quickpickStyles.ts` / `Quickpick.tsx`（popover 面板、listbox/option、
  空态）、`shortcutStyles.ts` / `Shortcuts.tsx`（dl/dt/dd + kbd mono）；用例
  `packages/ui/test/uiFoundationMisc.test.ts`。settings-sync 与资产重画仍待后续。

## 验收

1. **功能等价**：每个 P1.x 完成后跑相关 `packages/ui/test` 用例与对应 spec 场景；
   交互场景由人在桌面 + 手机 Web 各过一遍；
2. **视觉**：按 `DESIGN.md` 检查 token/间距/圆角/主题（允许与旧版视觉差异）；
3. **重合度**：每完成一个子目录，跑
   `pnpm check:upstream-overlap -- --json` 确认该目录 identical/normalized 归零；
4. **不回归契约**：事件流、`workspaceIdentity`、插件协议 schema 不改；
5. 全部完成后 `pnpm verify:self-developed` 才可能接近 0（还取决于 sidecar 口径）。

## 待裁决（阻塞正式开工）

1. **落点**：原地重建 `packages/ui` vs 新建 `packages/ui-next` 后切（见 M4 决策清单 D8）。
   建议 `ui-next` + 一次性 cutover，避免替换期间两套组件同时被引用。
2. **sidecar/传输层是否计入 0%**（D7）：决定 `packages/ui` 之外是否还有第二、三大块工作。
3. **验收人力**：UI 替换没有自动化 E2E，逐屏人工验收需要明确负责人与时间窗。

## 风险

| 风险 | 对冲 |
| --- | --- |
| 读旧实现照着写，违反 clean-room | 只读 `DESIGN.md`、spec、导出签名与测试断言；评审检查逐字雷同 |
| 组件替换导致 store/hook 双写 | 状态仍归 `packages/ui/src/store/`，替换只动渲染层 |
| 远程/流式面板回归 | 先复用现有纯逻辑测试，再逐面板替换 |
| `assets/` 来源不清 | 先出资产来源清单，第三方走台账，上游资产重画 |
