# M4 UI 替换：人工验收与旧文件删除清单（2026-10-19）

> 代码迁移已完成：业务代码对 `packages/ui/src/components/ui/*` 的引用为 0，
> 迁移守卫 `packages/ui/test/uiFoundationMigrationGuard.test.ts` 覆盖全部已迁移模块。
> 本文件只列**必须由人完成**的看屏验收与删除步骤；不包含上游实现内容。

## 1. 验收前基线

- `pnpm typecheck`、`pnpm --filter @mode/ui test`、`pnpm --filter @mode/desktop-tauri test` 全绿；
- 重合度（report）：headline **9.84%**（identical 2656 文件 / 25,042 行，normalized 188 / 20,355）；
- 旧 `components/ui/*` 全部保留，尚未删除。

## 2. 看屏清单（桌面 + 窄屏 Web 各跑一遍）

| 区域 | 重点场景 |
| --- | --- |
| 基础件 | Button 各 variant/size、Input/Textarea 聚焦与错误态、Select（键盘/搜索/自定义 indicator）、Checkbox/Switch/Radio、Dialog（遮罩/关闭钮/Esc/焦点）、Alert/AlertDialog |
| 菜单与浮层 | DropdownMenu（含 Sub/Radio/Checkbox/Shortcut）、ContextMenu（右键定位）、Popover（定位/外部点击）、Tooltip（延迟/焦点）、HoverCard |
| 布局 | 三栏 frame、Resizable 拖拽与 layoutId 持久化恢复、Side Pane tabs、折叠/滚动遮罩 |
| 会话 | 消息流、工具卡片、审批块、排队输入、代码块（行号/markedLines/focusedRange/选区/gutter 评论） |
| 设置 | 设置页各分区、插件市场（列表/详情/已安装）、自动化编辑、快捷键 |
| 文档预览 | PDF（翻页/缩放锚点/range 传输）、Diff（Patch/MultiFile）、PPTX（渲染/缩略图/选择/加入会话/PDF 导出）、Markdown/代码预览 |
| 远程 | LAN/SSH/WSL/Docker 面板、远程桌面、手机远控 |

## 3. 已知视觉/交互差异（允许视觉差异，不允许功能差异）

- Select：无 typeahead，`item-aligned` 统一按 popper 定位；
- Popover：`collisionBoundary/sticky/hideWhenDetached/updatePositionStrategy` 接受未生效；
- Tooltip：`skipDelayDuration`/`disableHoverableContent` 接受未生效；
- Toast：`anchorId` 未做锚点定位，无进出场动画；
- Dialog：`modal` 与关闭后焦点恢复未实现；遮罩/关闭钮由 adapter 提供；
- DropdownMenu：`loop` 总是循环，`modal` 不锁滚动；
- PDF：旧手势预览层未复刻（缩放锚点已补）；
- CodeView：注释/选区/gutter 为 clean-room 实现，视觉与旧 `@pierre/diffs` 可能不同；
- PptxPreviewSurface：**整块新实现，未做真窗口验收**，重点看缩略图/选中框/操作条/导出。

## 4. 验收通过后的删除步骤

1. 确认 `grep -rn "components/ui/" packages/ui/src --include=*.ts --include=*.tsx` 只剩旧目录内部引用；
2. 删除旧 `packages/ui/src/components/ui/*`（保留 `README.md`；`lib/` 下被业务使用的工具不动）；
3. 跑 `pnpm typecheck && pnpm --filter @mode/ui test && pnpm --filter @mode/desktop-tauri test && pnpm architecture:check --changed && pnpm lint`；
4. 重跑 `pnpm exec tsx scripts/upstream-overlap-check.ts`，把新 headline 写回路线图与 overlap 计划；
5. 本地提交（逐文件 `git add`），提交信息标注 M4 UI 收尾；**不推送**。

## 5. 删除后仍待人工的事项

- 真实 Windows UAC / Chrome 实测（browser-import-helper P2）；
- M3 三个 opt-in 真机步骤（产品 UI 挂载、双实例深链、真 webview 往返、自绘标题栏）；
- IM 真账号、两台设备 LAN/Web、市场窗口 E2E；
- macOS/Linux CI 出包、三平台签名、Electron EOL、迁移演练、软著/商标。
