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

| #   | 场景                                                          | 证据来源                                      | 状态                          |
| --- | ------------------------------------------------------------- | --------------------------------------------- | ----------------------------- |
| A1  | 冷启动：读取 `TuiStartupOptions`，建立会话并绘制空 transcript | `app-startup.tsx`、`app-empty-transcript.tsx` | 自动化（PTY harness，2026-10-22） |
| A2  | 恢复/切换会话：会话元信息、标题、消息数正确                   | `TuiSessionMetadata`、`app-turn-complete.ts`  | 无自动化                      |
| A3  | 一轮对话完成：终态、用量/耗时、错误分支                       | `app-turn-complete.ts`、`app-result.ts`       | 自动化（PTY，终态响应）+ 事件归约（批 25） |
| A4  | 紧凑时间线：长会话折叠与展开                                  | `app-compact-timeline.ts`                     | 部分：识别/时间线（批 6、21） |
| A5  | 排队输入：busy 时输入进入队列，空闲后按序提交                 | `app-queued-inputs.tsx`                       | 自动化（PTY，排队面板）+ 队列逻辑（批 30） |
| A6  | 输入历史：上下键召回与持久化边界                              | `app-input-history.ts`                        | 无自动化                      |

### B. 流式输出

| #   | 场景                                                          | 证据来源                                                             | 状态                    |
| --- | ------------------------------------------------------------- | -------------------------------------------------------------------- | ----------------------- |
| B1  | 文本增量：`text_delta` 逐块追加，不重排                       | `app-model-streaming.ts`、`app-transcript-stream.ts`                 | 自动化（PTY harness）+ 文本投影（批 16） |
| B2  | 工具输入增量：`tool_input_start/delta/end` 与最终 `tool_call` | 同上                                                                 | 自动化（PTY，增量不入正文） |
| B3  | 思考块：reasoning 与正文分区渲染                              | `app-thought-components.tsx`                                         | 自动化（PTY，折叠块）+ 思考增量（批 16） |
| B4  | Markdown/代码高亮：主题与终端宽度变化重排                     | `app-markdown.tsx`、`app-markdown-theme.ts`、`app-terminal-width.ts` | 无自动化                |
| B5  | 中断/取消：用户中断后流停止、终态正确                         | `app-submit.ts`、`app-submit-controller.ts`                          | 自动化（PTY，Esc 中止） |

### C. 审批与提问

| #   | 场景                                             | 证据来源                                                            | 状态                           |
| --- | ------------------------------------------------ | ------------------------------------------------------------------- | ------------------------------ |
| C1  | `allow_once`：批准后工具继续，规则不落盘         | `app-approval.ts`、`app-approval-panel.tsx`、`TuiRequestPermission` | 部分：文案/预览（批 13、36）   |
| C2  | `allow_project`：批准后本项目后续同规则不再询问  | 同上                                                                | 部分：文案/作用域（批 13、36） |
| C3  | `deny`：拒绝文案、工具不执行、会话继续           | 同上                                                                | 自动化（PTY，deny）+ 文案/取消（批 13、26） |
| C4  | `AskUserQuestion`：问题面板、多选/自由输入与取消 | `app-question-panel.tsx`、`app-question-state.ts`                   | 自动化（PTY，作答+Review 提交）+ 状态机（批 12、26、29） |
| C5  | 审批与流式并发：审批期间流式事件不丢、不越序     | `app-permission.ts`                                                 | 自动化（PTY，等待期间流式不丢） |

### D. 工具与 diff

| #   | 场景                                                   | 证据来源                                            | 状态                         |
| --- | ------------------------------------------------------ | --------------------------------------------------- | ---------------------------- |
| D1  | 工具卡片：scheduled/started/progress/result/batch 五态 | `app-tool-components.tsx`、`app-tool-transcript.ts` | 自动化（PTY，running/completed）+ 输入投影（批 33） |
| D2  | diff 呈现：新增/删除行、上下文折叠                     | `app-tool-diff-display.ts`                          | 部分：展示/宽度（批 15、22） |
| D3  | 路径展示：相对路径、过长路径截断                       | `app-tool-path-display.ts`                          | 自动化（批 18、19）          |
| D4  | 工具错误：`tool_execution_failed` 与未注册工具         | `app-transcript-errors.ts`                          | 自动化（PTY，failed 卡 + 错误文本）+ 错误文案（批 14） |

### E. 输入与命令中心

| #   | 场景                                 | 证据来源                                                                                | 状态                                    |
| --- | ------------------------------------ | --------------------------------------------------------------------------------------- | --------------------------------------- |
| E1  | 输入面板：光标、换行、粘贴、状态行   | `app-input-pane.tsx`、`app-input-status.tsx`                                            | 无自动化                                |
| E2  | 斜杠命令：补全、执行、错误           | `app-keyboard-suggestions.ts`、`TuiSlashCommandSuggestion`                              | 自动化（PTY，补全+提交）+ 补全/窗口（批 11、34） |
| E3  | 模式/模型/effort：切换命令与建议面板 | `app-mode-command.ts`、`app-model-command.ts`、`app-effort-command.ts`、`TuiSetMode`    | 自动化（PTY，Shift+Tab→setMode）+ 过滤/选择/补全（批 21、22、34） |
| E4  | 文件提及：`@` 补全与工作区建议       | `app-file-mentions.ts`、`app-file-mention-panel.tsx`、`TuiListWorkspacePathSuggestions` | 自动化（PTY，@ 面板 + Tab 补全）+ 可视窗口（批 16） |
| E5  | 键盘帮助与快捷键：平台差异           | `app-keyboard.ts`、`app-keyboard-helpers.ts`                                            | 部分：快捷键逻辑（批 2、5、23、34、35） |

### F. 侧栏、子代理与工作流

| #   | 场景                                     | 证据来源                                                             | 状态                         |
| --- | ---------------------------------------- | -------------------------------------------------------------------- | ---------------------------- |
| F1  | 侧栏：MCP、修改文件、子代理、API 区块    | `app-sidebar*.tsx`、`TuiListMcpServers`                              | 无自动化                     |
| F2  | 子代理：事件、transcript 快照、切换视图  | `app-subagent-*.ts`、`TuiReadSubagents`、`TuiReadSubagentTranscript` | 部分：谓词/快照（批 17、27） |
| F3  | 工作流：卡片、运行列表、重放、镜像到会话 | `app-workflow-*.ts`、`TuiListWorkflowRuns`、`TuiReplayWorkflowRuns`  | 部分：镜像/步数（批 39、40） |

### G. 系统面

| #   | 场景                                              | 证据来源                                          | 状态                        |
| --- | ------------------------------------------------- | ------------------------------------------------- | --------------------------- |
| G1  | 主题同步：系统主题/设置变化实时生效               | `app-theme-sync.ts`、`theme/`                     | 无自动化                    |
| G2  | 语言/本地化：中英文切换与缺键回退                 | `app-locale.ts`、`@mode/i18n`                     | 无自动化                    |
| G3  | 剪贴板：文本写入、图片读取（`TuiClipboardImage`） | `app-clipboard-image.ts`、`TuiWriteClipboardText` | 部分：图片校验（批 23）     |
| G4  | 网络/复制状态：连接事件、复制反馈                 | `app-network-events.ts`、`app-copy.ts`            | 部分：网络/复制（批 3、28） |
| G5  | 动效降级：motion 开关与无动画终端                 | `app-motion.tsx`                                  | 无自动化                    |

## 编译与类型面（2026-10-18 实跑）

```bash
cd apps/mode-cli/packages/tui && pnpm exec tsc --noEmit
```

结果：**通过（退出码 0，无输出）**。这说明公开类型与模块依赖链当前一致；
它不覆盖任何交互场景。

## 仍未完成项

**必须由人（需要交互式终端）**：

- 上表场景中，纯逻辑部分已由 `test/*.test.mts` 覆盖（批次见文末）；渲染、键盘焦点、
  真实流式顺序与落盘仍只能在真实终端逐项过，未标“部分/自动化”的场景完全依赖人工；
- 重点风险场景：C1–C3 三态审批落盘、B2 工具输入增量、B5 中断、C5 审批并发、
  F2 子代理切换、G3 平台剪贴板图片。

**可自动执行的后续**：

- 尚未覆盖的纯逻辑：`app-turn-complete` 的恢复分支、`app-sidebar-*` 的布局/状态组合、
  `app-input-status` 的展示文案；`app-input-history` 依赖 React 运行时，暂不能脱离渲染测试；
- 交互层保持人工验收，直到 CLI 迁移到 Rust 客户端。

**已登记差异**：`@mode/tui` 的纯逻辑测试已接入 release verify；测试不覆盖 opentui 渲染与
真实终端交互，表中的“部分/自动化”只表示纯逻辑契约，不等于交互验收完成。

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
- 第十八批（`app-tool-path-display`、`app-model` 常量）：POSIX 路径在 cwd 内转相对、
  容忍尾部斜杠、cwd 外与相对路径保持原样、空路径无展示值；审批决策常量与写入审批
  spec 的三态（`allow_once`/`allow_project`/`deny`）一致；
- 第十九批（`app-tool-path-display` 续）：Windows 路径归一化——分隔符统一为反斜杠、
  重复分隔符折叠、UNC 前导双斜杠保留、cwd 前缀剔除容忍尾部/混合分隔符与盘符大小写。
  行为由输入形态驱动（模拟 `process.platform = linux` 输出一致），可安全在 Linux CI 运行；
  上一轮“反斜杠被吞”结论是探针 heredoc 转义丢失造成的伪像，函数本身正确；
- 第二十批（`app-event-data` 续）：`validContextUsage` 只保留非负数值的
  contextUsed/contextWindow；`modelNetworkRequestTargetFromPayload` 仅在 url 为非空字符串时
  采用，否则回退占位 `model`；
- 第二十一批（`app-input` effort、`app-compact-timeline`）：effort 过滤（前缀/空查询/
  id/label/description 匹配）、命令选择与下标夹取、可视窗口；compact 时间线同 id 原位更新、
  不同 id 追加且保留非时间线消息，只有 failed 可重试并产出 `/compact`，失败收尾把最近
  started 置为 failed；
- 第二十二批（`app-input` model、`app-shiki-diff-view`）：model 过滤按 `provider/model`
  值、label 与 providerLabel 匹配，空查询全量，选择态产出与夹取；diff 视图在 121 列起
  切分栏、120 列及以下统一视图；
- 第二十三批（`app-keyboard-suggestions`、`app-input` 剪贴板）：建议导航仅在选中态生效，
  上下键移动并在边界夹取、消费按键，优先级 model → effort → mode → slash；剪贴板图片只
  接受受支持的 image/\* 且 data URL 非空；
- 第二十四批（`app-input` 提交/草稿转换）：`toPromptInput` 无附件时返回纯文本、只保留正文
  引用到的附件并按类型映射，模型选择原样带入；`toDraftAttachments` 文件用 `@path` 占位、
  图片按 data URL 推导媒体类型、缺 path/content 丢弃、无占位生成 `[image #id]`；
- 第二十五批（`app-events`）：`turn_started` 设置活动 turn 与 thinking；`turn_error` 记录
  错误、追加系统消息并清空排队；`turn_complete` 收尾状态、用量映射、缓存与回退响应；
  未知事件不触发任何 handler；
- 第二十六批（`app-question-state` 续）：Enter 选中当前项并进入 review；review 中再次
  Enter 以 `modify` 决策提交（携带 answers/questions）；Escape 以 `deny` 放弃；缺少
  questionState 时不改队列；
- 第二十七批（`app-subagent-events`）：只有工具事件且 `payload.source = subagent` 才算子代理
  镜像；目录刷新由生命周期事件触发，镜像事件与逐 token 的 model_streaming 不触发；
- 第二十八批（`app-network-events`）：请求事件缺字段/非法状态被忽略，合法事件补齐
  source=unknown 与时间戳，同 id 原位更新并前置，最多保留 8 条；模型事件完成/进行/失败/
  停滞/重试的状态映射、POST/source=model 归因、url 回退 model 与状态文案；
- 第二十九批（`app-question-state` 导航与多选）：上下键移动选中项（首项处向上保持），
  空格在单选下无效；多选时空格切换条目，Enter 把选中项汇总为逗号分隔答案并进入 review；
  先前“down 报错”是探针缺少 `sequence` 字段的伪像；
- 第三十批（`app-queued-inputs`）：新 id 追加、同 id 原位更新、内容未变返回原数组；
  `preserveExistingText` 保留非空旧文本；按 id 批量移除，空列表返回原引用、无匹配返回
  新数组；
- 第三十一批（`app-submit`）：非空 response 追加 agent 消息并保留既有消息；空/空白
  response 不追加但返回新数组；`restoredMessages` 存在时整体替换既有 transcript（空数组
  亦然）；恢复的 thought part 补 status/字符数，tool part 补 detailLines 与标题；
- 第三十二批（`app-modified-files` 提取）：单文件 diff 按 workspaceDirectory 转相对路径，
  无 workspace 保持绝对路径；多文件 diff 丢弃空路径、负数归 0、小数向下取整；未知 kind
  或缺 display 返回空数组；
- 第三十三批（`app-tool-transcript` 投影）：Read 标题取文件名并展示 offset/limit；
  Bash 只展示 command；Write 展示 file_path、Edit 只给标题；Grep/Glob 展示 pattern 与
  可选 path；未知工具按数字/嵌套对象/数组/布尔/null 概括展示；
- 第三十四批（`app-keyboard-helpers` 补全）：model/effort/mode 补全按选中项写回
  `/model provider/model`、`/effort id`、`/mode id`，下标夹取；无选中态或空列表不写草稿；
- 第三十五批（`app-sidebar-keyboard`）：Ctrl+X 只消费按键并提示 leader；leader 后
  B/M/A 分别切换侧栏、修改文件区块、API 区块，按键被消费并按展开/可见状态给出文案；
  普通按键不消费也不改状态；
- 第三十六批（`app-approval` 续）：描述优先 input.description、回退 reason；官方 CUA
  项目审批需 allow + addRules 且规则命中官方能力，并映射专属文案；作用域只取 Bash allow
  规则、前缀用省略号、最多 5 条，无 suggested updates 时从 input 取规则内容；
- 第三十七批（`state` 基础助手）：asRecord 只接受对象、string/number/boolean 字段类型校验
  （空串与空白保留、NaN/Infinity 拒绝）；千分位与耗时单位；截断含省略号计入上限；
  大小写不敏感匹配与空查询恒匹配；prompt 归一与事件名下划线转空格；
- 第三十八批（`app-turn-complete` 回退）：非空 response 追加 `streamProjected` agent 消息
  并保留既有消息；空/空白/缺失不触发写入；`messageId` 不进入投影消息；
- 第三十九批（`app-workflow-mirror`）：步数 = 节点数 + 未入表数，settled 按 phase 与
  未入表结算数；卡片索引按 toolCallId 建表、无关联键的 run 不进表、日志缺省为空数组；
  种子逐个并入且已有 runId 不被覆盖；
- 第四十批（`app-workflow-events`）：无 setter 时直接返回；无效 envelope 保持镜像引用；
  `run-started` 建出运行态条目（revision 1、running、空 usage/actors/nodes）；同一事件
  重放保持引用；`log` 折叠空白并追加到日志尾，空白日志不改日志；
- 后续可继续覆盖 `app-input-history` 等纯逻辑模块；交互场景仍待人工终端验收。

## PTY harness（2026-10-22）

- 落点：`apps/mode-cli/packages/tui/test/tui-pty.test.mts` +
  `test/fixtures/tuiPtyFixture.mts`；`node-pty` 作为包 devDependency。
- 形态：夹具直接调真 `runTui`（不提供 `loadStartupOptions`，跳过启动屏），
  `submitPrompt` 按脚本推 `model_streaming` 文本增量；测试用 `node-pty` 提供 TTY、
  收集并去 ANSI 后断言屏幕文本。
- 已覆盖（15/33）：A1 冷启动首屏、A3 `turn_complete` 终态响应、A5 busy 排队面板、
  B1 两段 `text_delta`、B2 工具输入增量不入正文、B3 折叠思考块、B5 Esc 中止进行中回合、
  C3 审批面板 + 默认 Deny 确认、C4 问题面板 + 作答 + Review 提交、C5 审批等待期间流式不丢、
  D1 工具卡 running→completed、D4 工具 failed 卡与错误文本、
  E2 `/` 补全 + Tab 提交、E3 Shift+Tab 触发 `setMode`、E4 `@` 面板 + Tab 补全路径。
  A5 的「空闲后按序提交」由 runtime 负责，PTY 用例只证明输入进入了队列面板。
- 平台注意：Windows 的 PTY 主端忽略 `encoding` 选项、中文会按系统代码页变乱码，
  所以夹具固定 `locale: "en-US"` 并只用 ASCII 标记断言。清理无语义退出（会话屏
  Ctrl-C 带防误退/中断流语义），键盘退出仍由 `scripts/mode-distribution-smoke.mjs`
  在发行包上覆盖。
- 控制信号两条路：**屏幕短标记**（`PTY_MODE_edit` 等，适合一次性可观察信号）与
  **结果文件**（`PTY_RESULT_FILE` 追加，适合会被重绘覆盖/裁掉、或需要轮询重发按键的信号；
  B5/C4 用它）。
  C3 用后者：轮询结果文件期间周期性重发 Enter，避免「首个按键在负载下偶发丢失」的竞态。
- 仍被按键编码阻塞的三条：**C1**（审批面板上移选择）、**A6**（空草稿上键召回）、
  **E1**（Shift+Enter 换行）。方向键试过 `\u001b[A` / `\u001b[1;1A` / `\u001bOA`，
  换行试过 `\u001b[13;2u` / `\u001b\r` / `\n`，均未到达对应处理器（疑似被输入编辑器消费）；
  需要先确认 opentui 在本环境的方向键/换行编码，或改用鼠标/快捷键。
- 其余 18 个场景按同一骨架追加：在夹具里加 `PTY_SCENARIO` 分支（审批走
  `options.requestPermission`，busy 输入走 `sendInput`，工具/问题走对应 `SessionEventType`）
  + 一条屏幕断言。
