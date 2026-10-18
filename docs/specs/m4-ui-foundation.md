# Spec：M4 UI 基础层（tokens 与 primitives）实现契约

状态：2026-10-18 首版。上游规格是 `DESIGN.md`；本文件把它收敛成 P1.1 可执行的接口与验收，
不重复设计叙事。**clean-room**：只取材于 `DESIGN.md`、既有 spec 与 token 名称；
不读 `packages/ui` 的实现体。主题的具体色值属于既有的数据契约（CSS 变量层），
实现时按变量名取值，不把旧组件的样式逻辑带进来。

## 1. Token 契约

### 1.1 主题

| 主题选项 | 实际变量集 |
| --- | --- |
| System | 跟随系统 → Light / Dark |
| Light Theme | Zai Light 变量集 |
| Dark Theme | Zai Dark 变量集 |

默认 light/dark 变量作为兜底；新组件必须同时在 Zai Light、Zai Dark 下检查。

### 1.2 颜色（语义层，禁止裸色值）

必须使用 `DESIGN.md` 列出的语义变量。分层：

- 页面：`background`；
- 结构面：`header` / `panel` / `sidebar`；
- 内容容器：`surface` / `surface-hover` / `card` / `card-selected`；
- 浮层：`popover` / `menu` / `menu-hover`；
- 输入：`input` / `input-focused` + 三种 border 状态；
- 文本：`foreground` / `-subtle` / `-subtlest` / `-inverse`；
- 状态：`success` / `warning` / `destructive`（各自 foreground）、`idle-task`、
  `diff-added` / `diff-removed`；
- 交互等待：`interaction-confirmation-*`（所有等待徽标统一绿色，不再区分 Ask/permission）；
- 浮层辅助：`toast` / `tooltip*` / `tag` / `find-highlight*` / `hover` / `selected` /
  `primary` / `primary-foreground` / `secondary`。

**禁止**：`text-white/60`、`border-white/10`、任意 neutral alpha、为“更醒目”借用
success/warning/destructive。

### 1.3 字体与 `text-ui-*`

- 界面文字一律 `text-ui-{2xs,xs,sm,caption,base,lg,xl}`；禁止 Tailwind 内置
  `text-sm/base/xs` 与任意 `text-[Npx]`。
- `--ui-font-size` 默认 14px；xl/lg/base/caption/sm/xs/2xs = +4/+2/0/−1/−2/−4/−5。
- `text-ui-2xs` 只允许工作流时间轴的轴标签/通道码/单位后缀。
- 移动 Web 可编辑输入用固定 `text-mobile-input-safe`（16px），不作为层级 token。
- 代码/Diff/终端用独立字号设置；其外围控件仍用 `text-ui-*`。
- 调整字号只改 `--ui-font-size`，绝不改 `html` 根字号；图标/间距/圆角不随字号缩放。
- 角色映射：h1→xl、h2→lg、h3–h6 与正文/按钮/标题→base、次要说明→sm、徽标/快捷键/
  极弱元数据→xs；Markdown inline code 用 `font-mono text-ui-sm`。

### 1.4 间距、尺寸、圆角、深度

- 间距基数 4px；常用 4/8/12/16/20–24；避免任意值。
- 图标 baseline `size-4`；控件高度 `h-6/7/8/9`；方形图标按钮 `size-6/7/8/9`。
- **圆角按容器嵌套**，不按重要性：
  - 第一个可见圆角容器 = `rounded-xl`；
  - 嵌套依次 `lg → md → sm`（`sm` 为下限）；
  - 普通控件在无父容器/xl 上为 `lg`，父 lg→md，父 md/sm→sm；
  - Dialog shell = `2xl`（三个预览 dialog 例外为 `xl`）；
  - 菜单/选择浮层 shell = `lg`，选项 hover/selected = `md`，选项内控件 = `sm`；
  - `2xl` 白名单：主聊天输入 shell、会话状态浮层、Toast、品牌图标背板；
  - `rounded-full` 只用于刻意的 pill/圆形。
- 深度：Base 无阴影；Surface 靠边框；Overlay `shadow-md`（菜单/浮层/Dialog）；
  Attention `shadow-lg` 仅 Toast/重要浮动卡。

### 1.5 动效

- 快速、低戏剧性；overlay 用淡入/轻微缩放/方向滑入；不做长弹簧动画。

## 2. Primitives 契约（P1.1 范围）

### 2.1 Button

- variant：`primary | outline | secondary | ghost | destructive | link`；
- size：`xs | sm | default | lg | icon-*`（icon 一律方形）；
- 状态：default / hover / active / focus-visible / disabled / loading；
- loading 时保留宽度、禁用重复提交、`aria-busy="true"`；
- 键盘：Enter/Space 触发；`focus-visible` 必须有可见焦点环；
- 不能把所有动作提升为 primary；同一面板保持清晰主次。

### 2.2 Input / Textarea / Select trigger

- 背景 `bg-input`，边框 `input-border`；hover / focus 分别用对应 token；
- 仅真实校验错误用 destructive 边框/文案，并配 `aria-invalid`；
- 圆角按控件表；
- Select trigger 外观遵循输入框语义，展开面板用菜单语言；
- 移动 Web 可编辑控件用 `text-mobile-input-safe`。

### 2.3 Card / Panel

- `bg-card border-card-border`，圆角从 xl 起；低强调容器用 `bg-surface`；
- 选中态 `bg-card-selected`；主内容卡常规 16px 横向 padding；
- 卡面必须比浮层安静，不与 popover/menu 混用。

### 2.4 Menu / Popover / Dialog

- Menu：`bg-menu`、`border-popover-border`、`rounded-lg`、`shadow-md`、`p-1`；
  选项行 `rounded-md`、`px-2`、`gap-2`、`text-ui-base`、固定 2px 垂直间距；
  hover `bg-menu-hover`；选中优先 check/radio/尾部标记，不用整行强填充；
- Popover：`bg-popover` + `border-popover-border`，独立浮层从 `rounded-xl` 起；
- Dialog shell：`rounded-2xl`（三个预览例外 `rounded-xl`），内容容器重新从 xl 起；
- 交互浮层必须盖过 Tooltip；菜单阴影从第一帧到焦点/悬停都保持；
- 焦点管理：打开时聚焦首个可交互项/对话框，Esc 关闭并把焦点还给 trigger，
  Tab 循环限制在 dialog 内（modal）。

### 2.5 Tabs / Selection

- 非活动 tab 中性；活动 tab 用更强 surface 对比，不用 brand 填充；
- 列表/选中行用 `bg-selected`；
- 键盘：Tab 进入 tablist；`ArrowLeft`/`ArrowRight` 循环聚焦，`Home`/`End` 到首尾；
  焦点移动后由调用方决定是否激活（自动/手动激活模式），组件本身只移动焦点。

### 2.6 Tooltip / Toast

- Tooltip：`bg-tooltip text-tooltip-foreground`，标题/正文 `text-ui-sm`，快捷键 `text-ui-xs`；
  不得覆盖活动浮层；键盘 focus 也应可触发；
- Toast：`bg-toast`、紧凑 padding、`shadow-lg`、`rounded-2xl`。

### 2.7 Collapsible / Spinner

- Collapsible：只负责展开状态与 `data-state` 语义；内容关闭时默认不渲染，
  `forceMount` 时保留 DOM 并标 `data-state="closed"`；触发器输出 `aria-expanded`
  与 `aria-controls`，点击/键盘 Enter/Space 切换；
- Collapsible 的视觉全部由调用方 `className` 决定（不引入自带边框/padding 的壳）；
- Spinner：内联 SVG + `animate-spin`，颜色继承 `currentColor`，尺寸由调用方
  `className`（`size-*`）决定；`role="status"`、`aria-label` 由调用方提供，
  缺省时 `aria-hidden="true"`，不参与 Tab 顺序。

### 2.8 Command palette

- 容器沿用菜单语言：`bg-popover`、`border-popover-border`、`rounded-lg`；
- 输入行在容器顶部，搜索图标与输入框 `text-ui-base`，输入框无独立边框/背景；
- 列表可滚动并保持 `min-h-0`；分组标题 `text-ui-xs text-foreground-subtlest`；
- 选项行与菜单一致（`rounded-md`、`px-2`、`gap-2`、hover/selected `bg-menu-hover`），
  cmdk 的 `data-selected="true"` 必须映射到同一 hover 视觉；
- 快捷键提示 `ms-auto text-ui-xs text-foreground-subtlest`；
- `CommandDialog` 由 Dialog + Command 组合，标题/描述只做可访问性（`sr-only`），
  不重复渲染可见标题。

### 2.9 Label / Kbd

- Label：`text-ui-sm font-medium text-foreground`，`htmlFor` 指向对应控件；
  不改变控件的 disabled/required 语义；
- Kbd：`rounded-sm border border-border bg-surface px-1 text-ui-xs text-foreground-subtle`；
- KbdGroup：`inline-flex items-center gap-1`，用于组合键展示。

### 2.10 HoverCard

- 视觉沿用 Popover：`bg-popover` + `border-popover-border` + `rounded-xl` + `shadow-md`，
  内容 padding 由调用方 `className` 决定；
- 悬停/聚焦打开，默认 `openDelay≈700ms`、`closeDelay≈300ms`，可通过 props 覆盖；
- 浮层本身可悬停：指针移入内容时取消关闭计时；
- 定位与 Popover 共用视口收敛口径；Esc 关闭；关闭后焦点回到触发器。

### 2.11 InputGroup

- 容器与 Input 同语言：`border border-input-border bg-input rounded-md`，聚焦时
  `focus-within:border-input-border-focused`；内部 input/textarea 去掉独立边框与背景；
- Addon 用 `align`（`inline-start` / `inline-end` / `block-start` / `block-end`）表达位置，
  行内 addon 与输入同高、`px-1.5`；
- 组内按钮使用 Button 的 `ghost` 语义与 `icon-*` 尺寸，不引入第二套按钮样式；
- 组内控件继承容器圆角，不再各自圆角。

### 2.12 ScrollFadeViewport

- 滚动容器在内容可向上/向下滚动时，用 1rem 渐变遮罩提示边界；
- 无溢出或到达端点时对应方向不显示遮罩；
- ref 必须指向真实滚动元素（调用方用它读写 `scrollTop`）；
- 遮罩不影响命中与滚动，`onScroll` 等原生事件照常透传。

### 2.13 Chart

- `ChartConfig` 的每项 `{ label?, icon?, color? }` 映射为容器内的 `--color-<key>` CSS 变量，
  颜色可以是 token 或 `var(--color-*)`；
- `ChartContainer` 只提供 `ResponsiveContainer`、config 上下文与尺寸类，不假设图表类型；
  `responsive={false}` 时直接渲染子节点（静态渲染/测试）；
- `ChartTooltipContent` 使用 Popover 语言（`bg-popover`、`border-popover-border`、`rounded-lg`、
  `shadow-md`），支持 `indicator=dot|line|dashed`、`hideLabel`、`hideIndicator`、
  `labelFormatter` 与 `formatter`；无 active/payload 时返回 null；
- `ChartLegend` / `ChartLegendContent` 与 tooltip 共用 label/color 查找规则；
- 文本使用 `text-ui-xs`/`text-ui-sm`，数值列 `tabular-nums`。

### 2.14 Resizable panels

- `ResizablePanelGroup` 包装 react-resizable-panels 的 `Group`：`orientation` 默认 horizontal；
  传 `layoutId`（+ 可选 `panelIds`）时通过 `useDefaultLayout` 持久化/恢复布局，不传则不持久化；
- `ResizablePanel` 直接映射 `Panel`，保留 `panelRef`/`elementRef`/`defaultSize`/`minSize`/`maxSize`/`collapsible`；
- `ResizableHandle` 映射 `Separator`，只给最小定位基类，视觉线/命中区由调用方 `className` 决定；
  分隔条 4px 命中区、2px 可见线遵循设计规范 §3。

### 2.15 DiffViewer

- 单文件 patch 用 `PatchDiff`，双文件内容用 `MultiFileDiff`（@pierre/diffs 公共 React API）；
- `lightTheme`/`darkTheme` 组合成 `options.theme = {light, dark}`，`themeType` 决定当前主题；
- 容器负责可滚动边界与 `--diffs-*` 变量（背景、字体族 `var(--font-mono)`、字号由 `fontSizePx`），
  diff 本体沿用库的 Shadow DOM 渲染；
- `selectedLines` 透传库类型；`disableWorkerPool` 透传；
- 外层 `className` 控制滚动/宽度，`diffClassName` 作用于 diff 根节点。

### 2.16 Lightweight diff preview

- 用于超大 patch / 多文件 patch 的降级预览：逐行读取，不构建富 diff；
- 行分类：`+` = added、`-` = removed、其余 = context；`marker` 保留首字符，
  `content` 去掉前缀；
- 行号从 1 开始，受 `codePreviewSettings.showLineNumbers` 控制；行高/字号来自设置；
  `wrapLongLines` 时使用 `whitespace-pre-wrap break-words`，否则横向滚动；
- 颜色只用 diff 语义色（`bg-diff-added/removed`），不借用 success/destructive；
- `renderLineContent` 允许调用方接管行内容（高亮版用它渲染 token）。

### 2.17 FlipMetricValue

- 逐字符渲染指标值，数字使用 `tabular-nums`，空格保留可断行宽度；
- 值变化时对变化的字符做 `animate-in`（淡入 + 轻微上移），时长 150ms；
- `prefers-reduced-motion: reduce` 时不播放动画；
- `animateInitial` 控制首次渲染是否动画；`aria-label` 提供完整值，子字符对读屏隐藏。

### 2.18 PdfViewer

- `source` 支持 URL/Blob/ArrayBuffer/Uint8Array 与 range 传输（`requestRange` + `totalBytes`）；
  ArrayBuffer/Uint8Array 进入 pdf.js 前复制，避免 worker detach 影响调用方复用；
- range 模式关闭自动预取/流式加载，chunk 256 KiB；
- 工具栏提供上一页/下一页、页码输入（Enter/失焦提交并夹在 1..numPages）、放大/缩小
  （25%–400%，步进 25%）；按钮用 `aria-label` 来自 `labels`；
- 加载中/加载失败/无数据均给出可见文案；`onLoadError` 透传 Error；
- 文档选项复用 `createPdfJsDocumentOptions`（cMap/字体资源），`labels` 只覆盖传入的键。

### 2.19 PptxSelectionActionBar

- 内部保持 children（幻灯片渲染）原样，操作条通过 portal 定位在选区可视区顶部中央；
- `boundary` 用于夹取水平位置，窗口 resize / 滚动时重新计算；
- 非编辑态显示 AI 编辑与退出选择两个按钮；编辑态显示 textarea、取消与「加入会话」；
- 提交前 trim，空内容不提交；Escape 在编辑态取消编辑、否则退出选择；IME 组合中不响应 Escape；
- `disabled` 时所有动作禁用。

## 3. 布局与响应式

- 桌面/宽 Web：会话、终端、Side Pane 三个独立 frame；4px 可拖拽间隔；
  resize handle 4px 透明命中区，hover/focus/drag 显示 2px `foreground-subtlest/50` 线；
- 手机远控保持单列 + drawer；
- 断点只改布局/宽度/可见性/密度，不改变组件语义；主操作在所有断点保留；
- 文本容器加 `min-w-0`，滚动容器加 `min-h-0`；优先 `w-full + max-w-*`。

**实现落点（P1.2 静态骨架）**：`shellStyles.ts` / `AppShell.tsx`——会话/终端/Side Pane
三个独立 frame、4px 间隔、4px 分隔条命中区（hover/focus/drag 2px 线）与 mobile drawer；
`AppShell.header` 渲染在会话 frame 内。`headerStyles.ts` / `WorkspaceHeader.tsx` 提供
标题/副标题/状态/操作槽位（文案由调用方传入，`ms-auto` 适配 RTL）。
**尺寸换算（2026-10-18）**：`resizeMath.ts`——`clampPanelSize` / `resizeFromPointer` /
`resizeFromKeyboard`（方向键 ±16px、Home/End 到边界、bounds 归一）/ `pointerDelta`
（竖向向右为正、横向向上为正）；`ResizeHandle` 在提供 `size`/`bounds`/`onResize` 时支持
键盘调整并输出 `aria-valuenow/min/max`。`usePanelResize` 负责 pointer capture、拖拽态与
实时换算（`data-dragging` 交给调用方）。尺寸持久化归调用方的状态层（store/settings），
组件不自己落盘。**组合（2026-10-19）**：`WorkspaceFrame` 持有 terminal/sidePane 尺寸，
通过 `PanelResizeBinding` 把 `usePanelResize` 的指针/键盘绑定交给 `AppShell`；
`panelSizePersistence.ts` 定义 `PanelSizePersistence` 契约（含内存实现与
`resolveInitialPanelSize` 的 clamp）。

## 4. 可访问性与 i18n

- 键盘导航是一等路径；可见焦点不可移除；
- 对比度在 light/dark/Zai 下安全；状态不能只靠颜色表达；
- 图标按钮必须有 `aria-label`；装饰性图标 `aria-hidden`；
- 布局需容忍长翻译，不靠截断作为唯一适配手段。

## 5. 验收

1. **Token 守卫**：新增组件不得出现内置 `text-sm/base/xs`、任意 `text-[Npx]`、
   裸色值/alpha 填充；可用 lint/单测扫描样式字符串。
2. **圆角纯函数**：`resolveControlRadius(parentRadius)` 覆盖 none/xl→lg、lg→md、
   md/sm→sm；容器嵌套 xl→lg→md→sm；用例固定。
3. **组件状态矩阵**：每个 primitive 有 default/hover/focus-visible/active/disabled/
   loading（适用时）的示例与键盘用例。
4. **主题**：在 Zai Light、Zai Dark 下逐组件截图检查；System 跟随正确。
5. **字号**：改 `--ui-font-size` 只影响文字；图标/间距/圆角不变。
6. **响应式**：桌面与手机 Web 各跑一遍主流程（会话/审批/工作区/设置）。
7. **clean-room**：实现只引用本 spec、`DESIGN.md` 与 token 变量名；
   评审检查与非接口约定所致的逐字雷同。

**实现落点（P1.1 已起步）**：圆角解析与排版守卫在
`packages/ui/src/lib/uiFoundation.ts`（`resolveControlRadius` / `resolveNestedRadius` /
`findUiTypographyViolations` / `UI_TEXT_TOKENS`），用例
`packages/ui/test/uiFoundation.test.ts`。Button 的类名契约与 primitive 在
`packages/ui/src/ui-foundation/`（`buttonStyles.ts` / `Button.tsx` / `index.ts`），
用例 `packages/ui/test/uiFoundationButton.test.ts`。输入类控件在
`inputStyles.ts` / `Input.tsx` / `Textarea.tsx` / `SelectTrigger.tsx`（半径映射共用
`radius.ts`），用例 `packages/ui/test/uiFoundationField.test.ts`。Surface 层在
`surfaceStyles.ts` / `Card.tsx` / `Menu.tsx` / `Popover.tsx` / `Dialog.tsx`（Dialog 含
焦点进入/恢复、Esc 关闭与 Tab 循环），用例 `packages/ui/test/uiFoundationSurface.test.ts`。
反馈/导航层在 `feedbackStyles.ts` / `Tabs.tsx` / `Tooltip.tsx` / `Toast.tsx`（交互浮层 z-50
高于 Tooltip z-40），用例 `packages/ui/test/uiFoundationFeedback.test.ts`。
展开器与加载指示在 `Collapsible.tsx` / `Spinner.tsx`，用例
`packages/ui/test/uiFoundationCollapsibleSpinner.test.ts`；
命令面板与标签在 `Command.tsx` / `commandStyles.ts` / `Label.tsx` / `Kbd.tsx`，
用例 `uiFoundationCommand.test.ts` / `uiFoundationLabelKbd.test.ts`；
HoverCard 在 `adapters/hover-card.tsx`，用例 `uiFoundationHoverCard.test.ts`；
输入组合与滚动遮罩在 `InputGroup.tsx` / `ScrollFadeViewport.tsx`，用例
`uiFoundationInputGroupScroll.test.ts`；图表组合在 `Chart.tsx`，用例
`uiFoundationChart.test.ts`；面板分隔在 `Resizable.tsx`，用例
`uiFoundationResizable.test.ts`；diff 呈现适配在 `adapters/diff-viewer.tsx`
（纯计算在 `diffViewerStyles.ts`），用例 `uiFoundationDiffViewer.test.ts`；
轻量 diff 在 `LightweightDiffPreview.tsx` / `HighlightedLightweightDiffPreview.tsx`
（行契约在 `lightweightDiffTokens.ts`），用例 `uiFoundationLightweightDiff.test.ts`；
指标翻牌在 `FlipMetricValue.tsx`，用例 `uiFoundationFlipMetric.test.ts`；
PDF 预览在 `adapters/pdf-viewer.tsx`（纯输入/边界在 `pdfViewerSource.ts`），用例
`uiFoundationPdfViewer.test.ts`（只测纯函数，不加载 pdf.js）；PPTX 选区操作条在
`adapters/pptx-selection-action-bar.tsx`（纯计算在 `pptxSelectionState.ts`），用例
`uiFoundationPptxSelection.test.ts`。
后续 primitives 复用这些函数，不再各写一套。

## 6. 不在本 spec 范围

- 具体业务页面（会话、设置、远程桌面等）——按 P1.2–P1.6 各自替换；
- 旧组件的迁移策略与 `ui-next` 落点（D8 待裁决）；
- 图标资产来源与品牌策略（见两份 assets inventory）。
