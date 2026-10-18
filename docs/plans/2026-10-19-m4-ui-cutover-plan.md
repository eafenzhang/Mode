# M4 UI 逐屏切换清单（2026-10-19）

> 依据 D8：原地重建 `packages/ui` + 逐屏 adapter + 验收通过后删除旧实现。
> 方法：`grep -rl "components/ui/<name>" packages/ui/src` 统计**引用文件数**（不是出现次数，
> 也不是行数）；不读取旧实现体。

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
| `tabs` | 12 | **缺 adapter**（Tabs/Tab 已有基础件） | 2 |
| `badge` | 15 | ready | 1（可随批 2） |
| `popover` | 16 | **缺 adapter** | 2 |
| `tooltip` | 18 | 只有静态语义 | 2 |
| `switch` | 24 | ready | 2 |
| `select` | 28 | 子件 ready；根状态机待补 | 2 |
| `dialog` | 33 | ready | 2 |
| `dropdown-menu` | 38 | 子件 ready；根/Trigger 待补 | 2 |
| `input` | 41 | ready | 3 |
| `toast` | 63 | 仅视觉 Toast；堆叠/定位待补 | 3 |
| `button` | 226 | ready | 3 |

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
