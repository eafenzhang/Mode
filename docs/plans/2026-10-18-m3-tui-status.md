# M3 生态域：TUI 场景清单与决策（2026-10-18）

> 路线图 M3 的 TUI 验收是「对照旧 TUI 的会话/审批/流式场景清单逐项过」。
> 本文件先建立清单（从 `@mode/tui` 的**公开导出面**与模块职责整理，未读上游衍生实现体），
> 再给出 M3 是否需要用 `ratatui` 重写的决策。

## 结论先行

**M3 不新建 `crates/tui`，旧 TUI 保持现状。** 理由：

1. 旧 TUI 是 `apps/mode-cli/packages/tui` 的 TypeScript/React（`@mbears/opentui-*`）客户端，
   通过 CLI 的 `runTui` 与公开 hooks（`TuiOptions`、`TuiSubscribeSessionEvents`、
   `TuiRequestPermission`、`TuiSubmitPrompt` 等）消费会话事件；它是 **Agent 客户端**，
   不是宿主进程的一部分，放进 Node sidecar 没有收益。
2. 语言政策允许 TypeScript；M5 的语言终态是 Rust + TS + HTML/CSS，TS TUI 不构成违规。
3. 重写成 `ratatui` 等于写第二个客户端，需要重新实现 80+ 模块的交互（侧栏、子代理、
   工作流、diff 高亮、输入法/剪贴板、主题），而 M3 没有可复用的 UI 组件与验收工具；
   收益只有在「CLI 变成 Rust 单二进制」时才出现（M4/M5 的运行时迁移）。

**DR-TUI-1（2026-10-18）**：TUI 沿用现有 TS 实现，M3 只需逐场景验收；
`crates/tui`（ratatui）延后到运行时移出 Node sidecar、CLI 需要 Rust 单二进制时再评估。
届时的行为基线就是下面这张表。

## 公开面证据

`apps/mode-cli/packages/tui/src/index.ts` 只导出：

- `runTui`（入口）；
- 能力接口类型：`TuiOptions`、`TuiStartupOptions`、`TuiSelection`、
  `TuiSessionMetadata`、`TuiSubscribeSessionEvents`、`TuiSubmitPrompt`、`TuiSendInput`、
  `TuiRequestPermission`、`TuiReadSubagents`、`TuiReadSubagentTranscript`、
  `TuiListWorkflowRuns`、`TuiReplayWorkflowRuns`、`TuiListMcpServers`、
  `TuiListWorkspacePathSuggestions`、`TuiSetMode`、`TuiModelOption`、`TuiEffortOption`、
  `TuiSlashCommandSuggestion`、`TuiClipboardImage`、`TuiWriteClipboardText` 等。

这说明 TUI 与运行时的耦合面是「事件订阅 + 权限请求 + 输入提交 + 子代理/工作流查询」，
与 Tauri 渲染层复用同一套 Host 服务语义；没有 Electron 专有依赖。

## 场景清单（验收基线）

> 状态口径：**无自动化** = 该场景没有可跑的测试，只有人工终端验收；
> **类型面** = 公开类型/编译守卫覆盖，不代表交互验收。

### A. 启动与会话生命周期

| #   | 场景                                                          | 证据来源                                      | 状态     |
| --- | ------------------------------------------------------------- | --------------------------------------------- | -------- |
| A1  | 冷启动：读取 `TuiStartupOptions`，建立会话并绘制空 transcript | `app-startup.tsx`、`app-empty-transcript.tsx` | 无自动化 |
| A2  | 恢复/切换会话：会话元信息、标题、消息数正确                   | `TuiSessionMetadata`、`app-turn-complete.ts`  | 无自动化 |
| A3  | 一轮对话完成：终态、用量/耗时、错误分支                       | `app-turn-complete.ts`、`app-result.ts`       | 无自动化 |
| A4  | 紧凑时间线：长会话折叠与展开                                  | `app-compact-timeline.ts`                     | 无自动化 |
| A5  | 排队输入：busy 时输入进入队列，空闲后按序提交                 | `app-queued-inputs.tsx`                       | 无自动化 |
| A6  | 输入历史：上下键召回与持久化边界                              | `app-input-history.ts`                        | 无自动化 |

### B. 流式输出

| #   | 场景                                                          | 证据来源                                                             | 状态     |
| --- | ------------------------------------------------------------- | -------------------------------------------------------------------- | -------- |
| B1  | 文本增量：`text_delta` 逐块追加，不重排                       | `app-model-streaming.ts`、`app-transcript-stream.ts`                 | 无自动化 |
| B2  | 工具输入增量：`tool_input_start/delta/end` 与最终 `tool_call` | 同上                                                                 | 无自动化 |
| B3  | 思考块：reasoning 与正文分区渲染                              | `app-thought-components.tsx`                                         | 无自动化 |
| B4  | Markdown/代码高亮：主题与终端宽度变化重排                     | `app-markdown.tsx`、`app-markdown-theme.ts`、`app-terminal-width.ts` | 无自动化 |
| B5  | 中断/取消：用户中断后流停止、终态正确                         | `app-submit.ts`、`app-submit-controller.ts`                          | 无自动化 |

### C. 审批与提问

| #   | 场景                                             | 证据来源                                                            | 状态     |
| --- | ------------------------------------------------ | ------------------------------------------------------------------- | -------- |
| C1  | `allow_once`：批准后工具继续，规则不落盘         | `app-approval.ts`、`app-approval-panel.tsx`、`TuiRequestPermission` | 无自动化 |
| C2  | `allow_project`：批准后本项目后续同规则不再询问  | 同上                                                                | 无自动化 |
| C3  | `deny`：拒绝文案、工具不执行、会话继续           | 同上                                                                | 无自动化 |
| C4  | `AskUserQuestion`：问题面板、多选/自由输入与取消 | `app-question-panel.tsx`、`app-question-state.ts`                   | 无自动化 |
| C5  | 审批与流式并发：审批期间流式事件不丢、不越序     | `app-permission.ts`                                                 | 无自动化 |

### D. 工具与 diff

| #   | 场景                                                   | 证据来源                                            | 状态     |
| --- | ------------------------------------------------------ | --------------------------------------------------- | -------- |
| D1  | 工具卡片：scheduled/started/progress/result/batch 五态 | `app-tool-components.tsx`、`app-tool-transcript.ts` | 无自动化 |
| D2  | diff 呈现：新增/删除行、上下文折叠                     | `app-tool-diff-display.ts`                          | 无自动化 |
| D3  | 路径展示：相对路径、过长路径截断                       | `app-tool-path-display.ts`                          | 无自动化 |
| D4  | 工具错误：`tool_execution_failed` 与未注册工具         | `app-transcript-errors.ts`                          | 无自动化 |

### E. 输入与命令中心

| #   | 场景                                 | 证据来源                                                                                | 状态     |
| --- | ------------------------------------ | --------------------------------------------------------------------------------------- | -------- |
| E1  | 输入面板：光标、换行、粘贴、状态行   | `app-input-pane.tsx`、`app-input-status.tsx`                                            | 无自动化 |
| E2  | 斜杠命令：补全、执行、错误           | `app-keyboard-suggestions.ts`、`TuiSlashCommandSuggestion`                              | 无自动化 |
| E3  | 模式/模型/effort：切换命令与建议面板 | `app-mode-command.ts`、`app-model-command.ts`、`app-effort-command.ts`、`TuiSetMode`    | 无自动化 |
| E4  | 文件提及：`@` 补全与工作区建议       | `app-file-mentions.ts`、`app-file-mention-panel.tsx`、`TuiListWorkspacePathSuggestions` | 无自动化 |
| E5  | 键盘帮助与快捷键：平台差异           | `app-keyboard.ts`、`app-keyboard-helpers.ts`                                            | 无自动化 |

### F. 侧栏、子代理与工作流

| #   | 场景                                     | 证据来源                                                             | 状态     |
| --- | ---------------------------------------- | -------------------------------------------------------------------- | -------- |
| F1  | 侧栏：MCP、修改文件、子代理、API 区块    | `app-sidebar*.tsx`、`TuiListMcpServers`                              | 无自动化 |
| F2  | 子代理：事件、transcript 快照、切换视图  | `app-subagent-*.ts`、`TuiReadSubagents`、`TuiReadSubagentTranscript` | 无自动化 |
| F3  | 工作流：卡片、运行列表、重放、镜像到会话 | `app-workflow-*.ts`、`TuiListWorkflowRuns`、`TuiReplayWorkflowRuns`  | 无自动化 |

### G. 系统面

| #   | 场景                                              | 证据来源                                          | 状态     |
| --- | ------------------------------------------------- | ------------------------------------------------- | -------- |
| G1  | 主题同步：系统主题/设置变化实时生效               | `app-theme-sync.ts`、`theme/`                     | 无自动化 |
| G2  | 语言/本地化：中英文切换与缺键回退                 | `app-locale.ts`、`@mode/i18n`                     | 无自动化 |
| G3  | 剪贴板：文本写入、图片读取（`TuiClipboardImage`） | `app-clipboard-image.ts`、`TuiWriteClipboardText` | 无自动化 |
| G4  | 网络/复制状态：连接事件、复制反馈                 | `app-network-events.ts`、`app-copy.ts`            | 无自动化 |
| G5  | 动效降级：motion 开关与无动画终端                 | `app-motion.tsx`                                  | 无自动化 |

## 编译与类型面（2026-10-18 实跑）

```bash
cd apps/mode-cli/packages/tui && pnpm exec tsc --noEmit
```

结果：**通过（退出码 0，无输出）**。这说明公开类型与模块依赖链当前一致；
它不覆盖任何交互场景。

## 仍未完成项

**必须由人（需要交互式终端）**：

- 上表 33 个场景目前**没有任何自动化测试**，只能在真实终端里逐项过；
- 重点风险场景：C1–C3 三态审批落盘、B1/B2 流式顺序、D2 diff 折叠、F2 子代理切换、
  G3 剪贴板图片（平台相关）。

**可自动执行的后续**：

- 为纯逻辑模块补单测（不依赖 opentui 渲染）：`app-question-state.ts`、
  `app-approval.ts` 的决策映射、`app-input-history.ts`、`app-tool-diff-display.ts`
  的行分类、`app-terminal-width.ts` 的宽度换算。这些不需要伪终端，也不会读上游实现体
  （以公开行为与本 spec 的场景为准）；
- 交互层保持人工验收，直到 CLI 迁移到 Rust 客户端。

**已登记差异**：`@mode/tui` 没有任何测试文件，CI 也不跑 TUI 测试；本清单是现状记录，
不是「已完成验收」。

## 纯逻辑测试补充（2026-10-19）

- `@mode/tui` 新增 `test` 脚本（`tsx --test test/*.test.mts`）；
- 首批覆盖 `app-terminal-width` 的公开导出（黑盒契约，不读实现）：
  `displayWidth`（ASCII/东亚宽字符）、`truncateDisplay`（省略号/未超宽/空串）、
  `wordWrappedLineCount`（空串至少一行、折行计数）；
- 第二批（`app-keyboard-helpers`）：命令补全写入、输入历史导航条件、workflow 裸 +/- 展开闸门、
  Ctrl-C 两段式退出窗口、Shift+Tab 模式切换；
- 第三批（`app-copy`）：可复制选区判定、copied/unavailable/failed/empty 状态映射；
- 第四批（`app-event-data`）：contextWindow 非负数值判定、错误文案提取与 Unknown 回退；
- 第五批（`app-sidebar-shortcut`）：Ctrl+X leader 状态机（B/M/A 切换、超时与修饰键回退）；
- 第六批（`app-compact-timeline`）：compact 命令识别（前缀/词边界/大小写/换行折叠）；
- 第七批（`app-modified-files`）：同路径累加/前置顺序、100 条上限；
- 第八批（`app-event-data` 扩展）：provider 计数形态默认值、serverToolUse 推导、usage/cache 空值；
- 第九批（`app-motion`）：spinner 帧序列/周期与负时间归一；
- 第十批（`app-input`）：clampIndex 边界、modelCommandQuery 前缀与分隔语义；
- 第十一批（`app-input` 续）：slash 命令过滤（名称/别名、大小写、空白终止）与可见窗口映射；
- 已接入 `release.yml` 的 verify：在 bootstrap 链构建后执行 `pnpm --filter @mode/tui test`；
- 第十二批（`app-question-state`）：提问面板初始状态（首题/首选项、非编辑/非 review、空答案）；
- 第十三批（`app-approval`）：审批决策文案（`allow_once`/`allow_project`，未知值与
  `allow_always` 回退 Deny）、权限输入预览（字符串原样、其余紧凑 JSON、长文本不截断）；
- 第十四批（`app-transcript-errors`、`app-model-ref`）：系统错误消息加 `Error:` 前缀、
  空文案回退 Unknown error、保留原消息且不修改入参数组；模型展示按首个斜杠拆分
  provider/模型、无 provider 与空选择用占位符；
- 第十五批（`app-tool-diff-display`、`app-selection-state`）：空 diff 的默认行集、
  filePath 沿用与 file 占位、结构化 hunk 合成 diff 文本、truncated 生成 meta 提示行；
  选择状态缺省返回 undefined、默认 filter 空且选中首项、下标夹取到有效范围、
  cursor/query 保留；
- 第十六批（`app-transcript-stream`、`app-file-mentions`）：文本/思考增量同 id 累加、
  空增量返回原数组、消息完成清 streaming、思考完成按 id 置 thought、响应判定要求
  投影标记且去空白后完全相等；提及窗口的空列表/非正可视数、越界夹取、滚动起点与
  窗内选中下标；
- 第十七批（`app-selection-keyboard`、`app-subagent-transcript`）：选择过滤只匹配
  keywords（大小写不敏感子串、空过滤全量、过滤词优先于输入值）、下标夹取、可视窗口、
  可打印键与输入展示；子代理快照默认空 transcript（Map 容器）、沿用 sessionId/消息、
  未知事件被忽略且保持引用；
- 后续可继续覆盖 `app-input-history` 等纯逻辑模块；交互场景仍待人工终端验收。
