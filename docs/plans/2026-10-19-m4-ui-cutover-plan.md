# M4 UI 逐屏切换清单（2026-10-19）

> 依据 D8：原地重建 `packages/ui` + 逐屏 adapter + 验收通过后删除旧实现。
> 方法：`grep -rl "components/ui/<name>" packages/ui/src` 统计**引用文件数**（不是出现次数，
> 也不是行数）；不读取旧实现体。
>
> **状态（2026-10-19）：清单内 21 个模块 + 后续波次的 collapsible/spinner/context-menu/
> command/label/kbd/hover-card 已切到 `ui-foundation/adapters`。** 剩余 `components/ui/`
> 引用集中在 code/pptx viewer 与 pptx 选择操作条；旧文件统一保留到人工看屏验收后删除。
> `ui-foundation/` 目录的历史格式债务已清零（`oxfmt --check` 115 文件全绿）。

## 基础件切换顺序（按引用面从小到大）

| 模块 | 引用文件数 | adapter 状态 | 批次 |
| --- | --- | --- | --- |
| `separator` | 0 | ready | 1 |
| `radio-group` | 0 | ready | 1 |
| `avatar` | 1 | ready | 1 |
| `scroll-area` | 2 | ready | 1 |
| `card` | 4 | ready（全家族） | 1 |
| `progress` | 4 | ready | 1 |
| `alert-dialog` | 5 | ready（Dialog 组合） | 1 |
| `checkbox` | 6 | ready | 1 |
| `alert` | 9 | ready（variant 映射） | 1 |
| `textarea` | 9 | ready | 1 |
| `tabs` | 12 | ready（variant/asChild/forceMount） | 2 |
| `badge` | 15 | ready · 已切换 | 1（随批 2） |
| `popover` | 16 | ready（受控/非受控 + 定位 + Anchor） | 2 |
| `tooltip` | 18 | ready（悬停/聚焦 + 延迟 + 定位） | 2 |
| `switch` | 24 | ready | 2 |
| `select` | 28 | ready（根状态 + 定位 + 键盘） | 2 |
| `dialog` | 33 | ready（含 overlay/关闭按钮） | 2 |
| `dropdown-menu` | 38 | ready（根/Trigger/Content/Sub） | 2 |
| `input` | 39 | ready · 已切换（size/htmlSize 兼容） | 3 |
| `toast` | 63 | ready（命令式队列 + 四向栈） | 3 |
| `button` | 226 | ready · 已切换 | 3 |

**计数会随切换变化**：每批开始前重跑上面的 grep，不要用手写数字排期。

## 批次策略

### 批次 1（低风险，adapter 已齐）

`separator → radio-group → avatar → scroll-area → card → progress → alert-dialog →
checkbox → alert → textarea`（可加 `badge`）。

- 每模块一次提交：替换 import 到 `@/ui-foundation/adapters/*`；
- 跑 `pnpm --filter @mode/ui test` + `pnpm typecheck`；
- 人工看对应屏（允许视觉差异、不允许功能差异）；
- 验收通过后删除旧 `components/ui/<name>.tsx`，更新 adapter map。

### 批次 2（中风险，需补 adapter 或交互语义）

`tabs / popover / tooltip / switch / select / dialog / dropdown-menu`。

- 先补缺失 adapter（Tabs/Popover 的旧导出名，Select/DropdownMenu 的根状态与 Trigger）；
- Tooltip 的显示/隐藏策略必须在接线层实现（悬停/焦点/延迟），不能只换壳。

### 批次 3（大面）

`input / toast / button`（226 个文件引用 button）。

- `button` 用 codemod/逐屏替换，每次只动一个屏的 import；
- `toast` 先补堆叠/定位（旧模块有 `upsertToastItem`、`resolveToastStackClassName` 语义）；
- 每批完成后重跑重合度，确认该子目录的 identical/normalized 下降。

## 验收记录格式（每模块）

```text
模块：button
切换文件：N 个屏 / M 个 import
测试：pnpm --filter @mode/ui test（结果）
类型：pnpm typecheck（结果）
人工：屏名 + 截图检查结论
删除：components/ui/button.tsx（commit）
```

## 当前前置缺口

1. Tabs / Popover 的旧命名 adapter；
2. Select / DropdownMenu 的根状态机与 Trigger 语义；
3. Tooltip 的显示策略；
4. Toast 的堆叠/定位；
5. `button` 的 226 个引用点需要按屏分批，不能一次替换。

## 批次 1 执行记录（2026-10-19）

| 模块 | 动作 | 结果 |
| --- | --- | --- |
| `separator` | `button-group.tsx` 的相对 import 改到 adapter；旧文件无其他引用 | **旧文件已删除**（`components/ui/separator.tsx`） |
| `avatar` | `WorkspaceSidebarFooter.tsx` 改到 `adapters/avatar` | 类型/测试通过；**旧文件待人工看屏后删除** |
| `scroll-area` | `AutomationEditView.tsx`、`ConversationShareSelectionPanel.tsx` 改到 `adapters/scroll-area` | 类型/测试通过；`type`/`scrollbarClassName` 被 adapter 接受但**暂不生效**（只影响滚动条可见性）；旧文件待看屏后删除 |
| `radio-group` | 无旧文件/无引用 | 无需动作 |

验证：`pnpm --filter @mode/ui test` 309 passed、`pnpm typecheck` 通过。

### 批次 1 续（2026-10-19 第二轮）

以下模块的全部 import 已切到 `ui-foundation/adapters`，类型与 UI 测试通过；
旧文件**待人工看屏后删除**：

| 模块 | 切换文件数 | adapter 兼容处理 |
| --- | --- | --- |
| `card` | 4 | 全家族直接转发 |
| `progress` | 4 | 兼容旧 `indicatorClassName` 与 `segments`（分段渲染） |
| `alert` | 9 | 旧 `variant` → tone |
| `textarea` | 9（含 `input-group` 相对引用） | 直接转发 |
| `checkbox` | 6 | 兼容 `checked: boolean \| "indeterminate"` + `onCheckedChange`；`checkIconStrokeWidth` 接受后忽略 |

`alert-dialog` 暂缓：旧导出集合包含 Action/Cancel 等组合语义，需单独补 adapter。

### 批次 2 开始（2026-10-19 第三轮）

| 模块 | 切换文件数 | adapter 兼容处理 |
| --- | --- | --- |
| `switch` | 24 | 接受旧 `size`（当前忽略，视觉差异登记）；`label` 缺省时取 `aria-label` |
| `alert-dialog` | 5 | 复用 Dialog 组合；`Action/Cancel` 走 `DialogClose` + Button 并映射旧 `variant/size`；`asChild` 接受但未实现；`Overlay` 为空实现 |

验证：UI 309 passed、typecheck/arch/language 通过。旧文件仍待看屏后删除。

### 批次 2 续（2026-10-19 第四轮）

| 模块 | 切换文件数 | adapter 兼容处理 |
| --- | --- | --- |
| `tabs` | 12 | `value/defaultValue/onValueChange` 根状态；`TabsList` 支持 `variant="line"`；`TabsTrigger` 支持 `asChild` 与 `data-state/data-active`；`TabsContent` 支持 `forceMount`（非激活保留在 DOM，标记 inactive） |
| `popover` | 16 | 受控/非受控 open；`PopoverAnchor` 支持 `asChild` 与 `virtualRef`；`PopoverContent` 按锚点 rect 计算 fixed 坐标并输出 `--radix-popover-trigger-width` / `--radix-popover-content-available-height`；接线 `onInteractOutside/onPointerDownOutside/onFocusOutside/onEscapeKeyDown/onOpenAutoFocus/onCloseAutoFocus`；`collisionBoundary/sticky/hideWhenDetached/updatePositionStrategy` 接受但暂不生效（登记为视觉差异） |

验证：UI 316 passed、typecheck/lint/architecture/language/format 通过。
`PopoverContent` 由 `position: fixed` 直接渲染在组件树内（未使用 portal），
外层 `overflow: hidden` 的容器可能裁切浮层；看屏时重点检查菜单类浮层。

### 批次 2 续（2026-10-19 第五轮）

| 模块 | 切换文件数 | adapter 兼容处理 |
| --- | --- | --- |
| `tooltip` | 18（含 `replay.ts` 的 `TooltipProvider` 再导出） | 补根 `Tooltip`：受控/非受控 open、`delayDuration` 继承 Provider（默认 0）、指针悬停/聚焦打开、离开/失焦关闭、Escape 关闭；`TooltipContent` 复用 Popover 定位数学并 portal 到 body；`skipDelayDuration` / `disableHoverableContent` 接受但暂不生效（登记为交互差异） |

验证：UI 319 passed、typecheck/lint/architecture/language/format 通过。
`ControlHintTooltip` 等自带 `open` 的调用点仍由外部分支控制显隐。

### 批次 2 续（2026-10-19 第六轮）

| 模块 | 切换文件数 | adapter 兼容处理 |
| --- | --- | --- |
| `select` | 28 | 补根 `Select`（受控/非受控 value 与 open）；内容常驻 DOM（关闭时 `hidden`）以便 `SelectValue` 从已挂载选项取文案；触发器支持旧 `variant`（input/default/outline/secondary/ghost/destructive）与自定义 `indicator`，`size="xs"` 归一到 `sm`；`position="item-aligned"` 与 `"popper"` 统一按 popper 定位；补 ArrowUp/Down 循环、Escape、外部点击关闭与 `onCloseAutoFocus`；`required/name` 接受但暂不生成隐藏 input |

验证：UI 321 passed、typecheck/lint/architecture/language/format 通过。
已知差异：无 typeahead；`item-aligned` 的贴选项对齐未实现；`onOpenAutoFocus` 接受但自动聚焦由 adapter 决定。

### 批次 2 收尾（2026-10-19 第七轮）

| 模块 | 切换文件数 | adapter 兼容处理 |
| --- | --- | --- |
| `dropdown-menu` | 38 | 补根 `DropdownMenu`（受控/非受控 open、`closeMenu` 回焦触发器）、`Trigger`（asChild/disabled）、`Content`（`forceMount`、fixed 定位、Arrow/Home/End/Escape、外部点击、`onOpenAutoFocus/onCloseAutoFocus`）、`Item/CheckboxItem/RadioGroup/RadioItem/Label/Separator/Shortcut` 与 `Sub/SubTrigger/SubContent`（右侧浮层、指针离开延时关闭、ArrowLeft/Escape 回父级）；输出 `--radix-dropdown-menu-content-available-width/height` 变量 |

验证：UI 325 passed、typecheck/lint/architecture/language/format 通过。

### 批次 3 开始（2026-10-19 第九轮）

| 模块 | 切换文件数 | adapter 兼容处理 |
| --- | --- | --- |
| `input` | 39 | 补旧 `size`（xs 归一到 sm）与 `htmlSize`（写原生 size），保留 `fieldSize/parentRadius/invalid`；补 `data-slot="input"` |

验证：UI 325 passed、typecheck/lint/architecture/language/format 通过。

### 批次 3 续（2026-10-19 第十轮）

| 模块 | 切换文件数 | adapter 兼容处理 |
| --- | --- | --- |
| `toast` | 63 | 新增命令式队列 `toastStore`（`toast/updateToast/dismissToast`、`dedupeKey` 替换并置底、默认 3000ms）与 `toast.tsx` 宿主：首次调用挂 body，四向栈（top-center/top-right/bottom-left/bottom-center），`warning` 用 destructive 边框，action/dismiss 按钮按选项渲染 |

验证：UI 328 passed、typecheck/lint/architecture/language/format 通过。
已知差异：`anchorId` 已存储但未做锚点定位；`info/update` 仅保留 data-variant，未加额外配色；toast 无进出场动画。

### 批次 3 收尾（2026-10-19 第十一轮）

| 模块 | 切换文件数 | adapter 兼容处理 |
| --- | --- | --- |
| `button` | 226 | 补旧 `warning` variant（secondary + 警告语义色）、`icon-md` size（归一到 icon）与 `data-slot/data-variant/data-size`；`asChild` 在旧调用点没有实际使用，暂不实现 |

验证：UI 328 passed、typecheck/lint/architecture/language/format 通过。
批次 1–3 的旧 `components/ui/*` 文件全部保留到人工看屏验收后统一删除（D8 第 4 条）。
已知差异：`loop` 接受但当前总是循环；子菜单为点击/悬停双开、`modal` 接受后不锁滚动；
`DropdownMenuItem` 的 `onSelect` 在 onClick 内触发（Radix 的键盘高亮语义由原生 focus 承担）。

### 批次 2 收尾（2026-10-19 第八轮）

| 模块 | 切换文件数 | adapter 兼容处理 |
| --- | --- | --- |
| `dialog` | 33 | 补 `showOverlay/showCloseButton/overlayClassName`：adapter 渲染遮罩层并把面板居中（宽度缺省补 `w-full`，无 `max-w-*` 时补 `max-w-lg`）；`showCloseButton` 默认 true，内置关闭按钮；`onEscapeKeyDown` 走捕获阶段并尊重 `preventDefault`；`onInteractOutside` 接遮罩点击；`modal/onCloseAutoFocus` 接受后不生效 |
| `badge` | 15 | 直接转发旧导出名 |

验证：UI 325 passed、typecheck/lint/architecture/language/format 通过。

### 后续波次（2026-10-19 第十二轮）

| 模块 | 切换文件数 | adapter 行为 |
| --- | --- | --- |
| `collapsible` | 14 | 新增 foundation `Collapsible`：受控/非受控 open、`asChild` 根/触发器/内容、关闭默认不渲染、`forceMount` 保留并标 `data-state="closed"`、触发器 `aria-expanded` |
| `spinner` | 11 | 新增 foundation `Spinner`：内联 SVG + `animate-spin`，颜色继承 `currentColor`；`label` 缺省时 `aria-hidden` 且 `focusable="false"` |
| `context-menu` | 14 | 复用 DropdownMenu 根状态/条目/子菜单；右键写入视口坐标，`DropdownMenuContent` 新增 `anchorPoint` 支持指针定位 |

验证：UI 335 passed、typecheck/lint/architecture/language/format（改动文件）通过。
已知差异：`CollapsibleContent asChild` 只合并 `data-state`/ref；context-menu 的键盘导航沿用菜单实现，
指针越界收敛与菜单一致。

### 后续波次（2026-10-19 第十三轮）

| 模块 | 切换文件数 | adapter 行为 |
| --- | --- | --- |
| `command` | 7 | 新增 foundation `Command`（cmdk 公共 API：过滤/键盘/选中由库负责）；输入行保留 `data-slot="command-input"` 供搜索聚焦查询；`CommandDialog` 由 adapter 的 Dialog 组合，标题/描述仅 `sr-only` |
| `label` / `kbd` | 1 / 1 | 新增 foundation `Label`、`Kbd`/`KbdGroup`；token 与语义元素直出 |
| `hover-card` | 3 | 新增 adapter：`open/defaultOpen`、`openDelay=700`/`closeDelay=300`、内容可悬停、Escape 关闭并回焦；定位复用 Popover 数学并 portal 到 body |

验证：UI 341 passed、typecheck/lint/architecture/language/format（改动文件）通过。
已知差异：Command 的视觉细节由 cmdk data 属性承载；HoverCard 关闭后不自动恢复焦点（仅 Escape 路径）。

### 后续波次（2026-10-19 第十四轮）

| 模块 | 切换文件数 | adapter 行为 |
| --- | --- | --- |
| `input-group` | 2 | 新增 foundation InputGroup：容器统一边框/焦点态，`InputGroupInput/Textarea` 去壳，Addon 支持 inline/block 四个方向，组内按钮复用 foundation Button（ghost + icon 尺寸） |
| `scroll-fade-viewport` | 2 | 新增 foundation ScrollFadeViewport：ref 指向真实滚动元素，按上/下溢出方向给 1rem 渐变遮罩，`onScroll` 照常透传 |

另：`packages/ui/src/ui-foundation/`（115 文件）已统一过一次 `oxfmt`，`fmt:check` 该目录全绿。
验证：UI 343 passed、typecheck/lint/architecture/language/format 通过。

### 后续波次（2026-10-19 第十五轮）

| 模块 | 切换文件数 | adapter 行为 |
| --- | --- | --- |
| `chart` | 6 | 新增 foundation Chart：`ChartConfig` → `--color-<key>` 变量、`ChartContainer`（ResponsiveContainer + `responsive={false}` 静态模式）、`ChartTooltipContent`（dot/line/dashed、hideLabel/hideIndicator、labelFormatter/formatter）、`ChartLegendContent` |

验证：UI 346 passed、typecheck/lint/architecture/language/format 通过。
已知差异：只生成单套 CSS 变量（未区分明暗主题变量）；tooltip 的容器定位/动画交给 recharts。

### 后续波次（2026-10-19 第十六轮）

| 模块 | 切换文件数 | adapter 行为 |
| --- | --- | --- |
| `resizable` | 3 | 新增 foundation Resizable：`Group/Panel/Separator` 映射；`layoutId`（+`panelIds`）走 `useDefaultLayout` 持久化，SSR 下注入 no-op storage；`ResizableHandle` 只给最小定位基类，视觉线与命中区由调用方 className 决定 |

验证：UI 348 passed、typecheck/lint/architecture/language/format 通过。
已知差异：未提供旧实现的额外 grip 图标；持久化键沿用调用方 `layoutId`。

### 后续波次（2026-10-19 第十七轮）

| 模块 | 切换文件数 | adapter 行为 |
| --- | --- | --- |
| `diff-viewer` | 3 | 新增 adapter：单文件 `PatchDiff` / 双文件 `MultiFileDiff`；`lightTheme`+`darkTheme` 组合为 `options.theme`，`themeType` 透传；容器提供 `--diffs-*` 背景/字体变量与 `fontSizePx`，`selectedLines`/`disableWorkerPool` 透传 |

验证：UI 351 passed、typecheck/lint/architecture/language/format 通过。
已知差异：`selectedLines` 沿用库类型，未做旧 API 的 side 归一；测试只覆盖纯样式/主题函数，
避免在测试进程加载 @pierre/diffs 重型运行时。

### 后续波次（2026-10-19 第十八轮）

| 模块 | 切换文件数 | adapter 行为 |
| --- | --- | --- |
| `lightweight-diff-preview` | 2 | 新增 foundation：行分类（added/removed/context）、marker/content、行号、`wrapLongLines`、diff 语义色与 `renderLineContent` 接管 |
| `highlighted-lightweight-diff-preview` | 2 | 新增 foundation：异步 Shiki token（>120k 字符或纯文本语言跳过），按行渲染 token，结构复用 lightweight；纯行契约拆到 `lightweightDiffTokens.ts` 便于单测 |

验证：UI 355 passed、typecheck/lint/architecture/language/format 通过。
已知差异：行号按数组下标从 1 开始（props 未提供起始行）；超大 patch 高亮阈值沿用 120k 字符。

### 后续波次（2026-10-19 第十九轮）

| 模块 | 切换文件数 | adapter 行为 |
| --- | --- | --- |
| `flip-metric-value` | 1 | 新增 foundation FlipMetricValue：逐字符渲染、`tabular-nums`、空格/标点宽度、值变化字符 `animate-in` 150ms、`prefers-reduced-motion` 降级、`animateInitial` |

验证：UI 357 passed、typecheck/lint/architecture/language/format 通过。
已知差异：动画只对变化字符播放，不做旧实现的整列翻牌。

### 后续波次（2026-10-19 第二十轮）

| 模块 | 切换文件数 | adapter 行为 |
| --- | --- | --- |
| `pdf-viewer` | 6（4 类型引用 + 2 动态 import） | 新增 adapter：react-pdf Document/Page；URL/Blob/ArrayBuffer/Uint8Array 归一（ArrayBuffer 复制防 detach）、range 传输（256 KiB chunk、关自动预取）；页码输入/前后页/25%–400% 缩放、Ctrl/Cmd+滚轮缩放、labels 覆盖、加载/失败/无数据文案 |

验证：UI 361 passed、typecheck/lint/architecture/language/format 通过。
已知差异：缩放不再围绕指针锚点（旧实现的手势锚点/预览层未复刻）；`pdf-viewer` 不放进
adapter barrel，避免只想要轻量 adapter 的调用方被迫加载 pdf.js 与 Vite `?url` worker。
