# M4 UI 接线前适配层映射（2026-10-19）

> 目的：D8（`ui-next` vs 原地重建）落点确定后，能按一张表逐组件切换。
> **方法**：只列旧模块路径、公开导出名与新 `ui-foundation` 的对应关系——
> 导出签名属于允许读取的接口面，未打开任何旧组件实现体（clean-room）。

## 1. 基础件映射

| 旧模块（`packages/ui/src/components/ui/`） | 旧导出 | 新落点 | 状态 |
| --- | --- | --- | --- |
| `button.tsx` | `Button`、`buttonVariants` | `ui-foundation/Button` + `buttonClassName` | **可直接换**；variant/size 名称需对齐 |
| `input.tsx` | `Input`、`inputVariants` | `Input` + `inputClassName` | **可直接换**；variant 归一到 fieldSize |
| `textarea.tsx` | `Textarea` | `adapters/textarea` | **已切换 9 处调用** |
| `card.tsx` | `Card`、子件 | `adapters/card` | **已切换 4 处调用**；旧文件待看屏后删除 |
| `dialog.tsx` | 复合导出（Trigger/Content/Header/Title/Description/Footer…） | `adapters/dialog.tsx`（Dialog/Content/Trigger/Close/Header/Title/Description/Footer/Portal） | **adapter 已补**（2026-10-19） |
| `alert-dialog.tsx` | 复合导出 | `adapters/alert-dialog` | **已切换 5 处调用**（Action/Cancel 走 DialogClose；asChild/Overlay 登记差异） |
| `dropdown-menu.tsx` | 复合导出 | `Menu`/`MenuItem`/`MenuGroup`/`MenuSeparator`/`MenuCheckboxItem`/`MenuRadioItem` | **已补**（2026-10-19）；Trigger/Sub 待 adapter |
| `popover.tsx` | 复合导出 | `Popover` | **部分**：Trigger/Content/Anchor 待补 |
| `select.tsx` | 复合导出 | `SelectTrigger`/`SelectContent`/`SelectItem`/`SelectGroup`/`SelectValue` | **已补**（2026-10-19） |
| `tabs.tsx` | `Tabs`、`TabsList`、`TabsTrigger`、`TabsContent` | `Tabs` / `Tab` | **部分**：命名对齐 + Content 待补 |
| `tooltip.tsx` | `Tooltip`、`TooltipContent`、`TooltipProvider`、`TooltipTrigger` | `Tooltip`/`TooltipContent`/`TooltipProvider`/`TooltipTrigger` | **已补静态语义**（2026-10-19）；显示/隐藏与延迟策略待接线 |
| `toast.tsx` | `ToastPosition`、`upsertToastItem`、`resolveToastStackClassName` | `Toast` | **部分**：堆叠/定位策略待补 |
| `badge.tsx` | `Badge`、`badgeVariants` | `Badge` + `badgeClassName` | **已补**（2026-10-19） |
| `separator.tsx` | `Separator` | `adapters/separator` | **已切换并删除旧文件**（2026-10-19） |
| `scroll-area.tsx` | `ScrollArea`、`ScrollBar` | `adapters/scroll-area` | **已切换 2 处调用**；`type`/`scrollbarClassName` 静默忽略（待接线） |
| `checkbox.tsx` / `switch.tsx` / `radio-group.tsx` / `progress.tsx` | 对应控件 | `adapters/*` | **已切换**：checkbox 6、progress 4、switch 24；radio 待切换 |
| `avatar.tsx` | `Avatar*` | `adapters/avatar` | **已切换 1 处调用**；旧文件待看屏后删除 |
| `alert.tsx` | `Alert*` | `adapters/alert` | **已切换 9 处调用**（variant→tone） |

## 2. 业务面映射

| 旧模块 | 旧导出 | 新落点 | 缺口 |
| --- | --- | --- | --- |
| `App.tsx` / `Root.tsx` | `App`、`Root` | `WorkspaceFrame` + `AppShell` + `WorkspaceHeader` | 路由/面板编排待接 |
| `LexicalChatInput.tsx` | `LexicalChatInput`、`ChatComposerPasteEvent`、`LexicalChatInputHandle` | `ChatInputShell` | **编辑器本体（Lexical/IME/粘贴）需独立重写** |
| `ModelTrajectoryPane.tsx` | `ModelTrajectoryPane` + 辅助导出 | `MessageList` / `MessageRow` | 轨迹搜索/展开/高亮待做 |
| `PermissionDialog.tsx` | `PermissionDialog` | `ApprovalBlock` + `Dialog` + `Button` | 三态审批接线 |
| `ToolCallBlocks/` | 多种 renderer/summary | `ToolBlock` / `DiffBlock` | 每类工具的 renderer 映射待做 |
| `PreviewPane.tsx` | `PreviewPane` | `PreviewPane`（同名，语义不同） | 格式分派（docx/pdf/image…）待做 |
| `settings/PluginStorePage.tsx` | `PluginStorePage` | `SettingsLayout` + `PluginCard` | 数据接线与 6 场景 E2E |
| `settings/PluginStoreDetailView.tsx` | `PluginStoreDetailView`、`PluginStoreAdvancedSection` | `PluginUnavailableHint` + `SettingsSection` | 组件清单/describe 状态 |
| `remote-desktop/*` | `RemoteDesktopHome`、`RemoteDesktopSession`、`DeviceCard`、`AddDevice` 等 | `RemoteSurface` + `RemoteToolbarButton` + `RemoteStatusItem` | 会话/设备列表接线、媒体流 |
| `Terminal` / `browser-use` / `git-graph` / `mentions` / `workflow` | 目录级 | 终端/预览容器已有；其余暂无 | 各自专项 |

## 3. 接线策略

1. **先补 composable，再做 adapter**：Card 子件、Dialog/Menu/Select/Tooltip 的 Trigger/Content
   子件、Badge/Separator/Checkbox 等，都是「新组件复用旧导出名」的前提；
2. **adapter 层放在 `ui-foundation/adapters/`**：保留旧导出名与 props 形状，内部转到新 primitive，
   逐屏切换时调用方不用一次性改 import；
3. **先静态后行为**：先替换纯展示（消息、工具、diff、设置卡片），再替换带焦点/编辑器的
   （LexicalChatInput、Dialog/Menu 交互）；
4. **状态不迁移**：`packages/ui/src/store/` 仍是唯一状态所有者；adapter 只换渲染。

## 4. D8 裁决（2026-10-19）：原地重建 + 逐屏 adapter

- 落点：`packages/ui` 原地重建，`ui-foundation` 保持现路径；不新建 `ui-next`；
- 切换：adapter 直接覆盖 `components/ui/*` 的导出，旧实现按屏验收通过后删除；
- 状态层：`store/`、`hooks/` 不动，adapter 只换渲染；
- 入口：`@mode/ui` 包名与 `App` 导出不变，调用方零改动。

## 5. 下一步可执行项

1. 〔已完成 2026-10-19〕补 Card 子组件（Header/Title/Description/Content/Footer/Action）；
2. 〔已完成 2026-10-19〕补 Dialog 的 Header/Footer/Title/Description；Trigger 语义留 adapter；
3. 〔已完成 2026-10-19〕补 Menu 的 Group/Separator/Checkbox/Radio；Trigger/Sub 留 adapter；
4. 〔已完成 2026-10-19〕补 Select Content/Item/Group/Value 与 Tooltip Provider/Trigger 静态语义；
5. 〔已完成 2026-10-19〕补 Badge/Separator 两个高频缺口；
6. 之后按 P1.2→P1.7 顺序落 adapter。

## 6. adapter 第一批（2026-10-19 已落地）

`packages/ui/src/ui-foundation/adapters/` 已提供旧导出名包装：

- `button.tsx`：`Button`（`variant: default|primary|outline|secondary|ghost|destructive|link`，
  `default → primary`）、`buttonVariants`、`mapButtonVariant/Size`；
- `input.tsx`：`Input`、`inputVariants`（`fieldSize` 对应旧 size，未知值走默认）；
- `card.tsx`：`Card` 全家族原样转发；
- `badge.tsx`：`Badge`（旧 `variant` 归一到 tone）、`badgeVariants`；
- `separator.tsx`：`Separator`。

用例：`packages/ui/test/uiFoundationAdapters.test.ts`。**尚未覆盖** Dialog/Menu/Select/Tooltip
的旧 props 子集——它们的 Trigger/Content 语义需要在切换对应屏时按调用方用法补齐，
不在 adapter 里猜。

## 7. adapter 第二批（2026-10-19 已落地）

- `dropdown-menu.tsx`：`DropdownMenuContent/Group/Item/CheckboxItem/RadioItem/Separator/Label`
  （旧导出名）；根 DropdownMenu/Trigger/Sub 的开关语义留到切换时补；
- `select.tsx`：`SelectContent/Group/Label/Item/Trigger/Value`；根 Select 状态机待切换；
- `tooltip.tsx`：`TooltipProvider/Trigger/Content`；显示/隐藏策略待接线。

用例：`packages/ui/test/uiFoundationAdaptersSecond.test.ts`。

## 8. Dialog adapter（2026-10-19 已落地）

`adapters/dialog.tsx`：`Dialog`（受控 open / 非受控 defaultOpen + onOpenChange）、
`DialogContent`（打开时渲染 foundation Dialog，负责焦点/Esc/Tab）、`DialogTrigger`/`DialogClose`
（克隆子元素 onClick，尊重 `defaultPrevented`）、`DialogPortal`（passthrough），并转发
Header/Title/Description/Footer。用例用 `renderToStaticMarkup` 覆盖受控/非受控的渲染条件。

## 9. 逐屏切换清单

引用文件数、批次与验收记录格式见 `2026-10-19-m4-ui-cutover-plan.md`。
批次 1 的 adapter 已全部就绪（`separator/radio-group/avatar/scroll-area/card/progress/alert-dialog/checkbox/alert/textarea`），
可直接开始替换 import。
