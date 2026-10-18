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
- 列表/选中行用 `bg-selected`。

### 2.6 Tooltip / Toast

- Tooltip：`bg-tooltip text-tooltip-foreground`，标题/正文 `text-ui-sm`，快捷键 `text-ui-xs`；
  不得覆盖活动浮层；键盘 focus 也应可触发；
- Toast：`bg-toast`、紧凑 padding、`shadow-lg`、`rounded-2xl`。

## 3. 布局与响应式

- 桌面/宽 Web：会话、终端、Side Pane 三个独立 frame；4px 可拖拽间隔；
  resize handle 4px 透明命中区，hover/focus/drag 显示 2px `foreground-subtlest/50` 线；
- 手机远控保持单列 + drawer；
- 断点只改布局/宽度/可见性/密度，不改变组件语义；主操作在所有断点保留；
- 文本容器加 `min-w-0`，滚动容器加 `min-h-0`；优先 `w-full + max-w-*`。

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
用例 `packages/ui/test/uiFoundationButton.test.ts`。后续 primitives 复用这些函数，
不再各写一套。

## 6. 不在本 spec 范围

- 具体业务页面（会话、设置、远程桌面等）——按 P1.2–P1.6 各自替换；
- 旧组件的迁移策略与 `ui-next` 落点（D8 待裁决）；
- 图标资产来源与品牌策略（见两份 assets inventory）。
