# Spec：C2 Agent 黑盒契约（compact / memory / contextUsage / fresh-install ledger）

状态：**T0 捕获完成（2026-11-17）**。本文只记录 oracle（现网 Node `mode.cjs` 源码入口，
`app-server --stdio`）的黑盒观察结果，作为 C2 Rust `agent-server` / `session-store` 的
对拍基线；不是产品设计稿，也不含手写期望值。所有 `expect` 由探针从真实运行录制。

关联：`docs/specs/agent-c2-context-memory.md`、
`docs/plans/2026-11-17-lt1-c2-context-memory.md`、`docs/specs/agent-server-c1-contract.md`、
`docs/specs/tauri-agent-runtime.md`、`docs/specs/clean-room-policy.md`。

## 0. 来源、纪律与复现

| 项            | 值                                                                                                             |
| ------------- | -------------------------------------------------------------------------------------------------------------- |
| oracle 入口   | `node node_modules/tsx/dist/cli.mjs apps/mode-cli/packages/cli/src/main.ts app-server --stdio`                 |
| 驱动          | `packages/desktop/tests/agent-runtime-oracle/driver.mts` 的 `startOracleRuntime`                               |
| mock provider | `127.0.0.1:0` 上的 `anthropic-messages` 端点；剧本由 `mock-provider.mts` 提供                                  |
| 隔离根        | `mkdtemp` 目录；同时注入 `MODE_DATA_BASE_DIR`、`MODE_HOME`、`HOME`、`USERPROFILE`                              |
| 会话参数      | `oracle-mock / mock-1`，`reasoningLevel:"disabled"`，`thoughtLevel:"disabled"`，`titleGenerationEnabled:false` |
| 数据布局      | `<隔离根>/.mode/cli/db/db.sqlite`；CLI 布局哨兵 `<隔离根>/.mode/cli/log`                                       |

复现命令（全部从仓库根目录执行）：

```bash
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c2-compact.mts
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c2-compact-race.mts
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c2-compact-failure.mts
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c2-auto-compact.mts
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c2-memory.mts
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c2-context.mts
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c2-fresh-install.mts
```

夹具落点：`packages/desktop/tests/agent-runtime-oracle/fixtures/c2/`。`fixtures.test.mts`
只读取 `fixtures/` 根下 `*.json`，因此 `c2/` 子目录不会进入 C1 的 38 条三层回放。

隔离断言（每个夹具的 `expect.*.isolation`，全部为真）：

| 断言                                          | 观察值                                |
| --------------------------------------------- | ------------------------------------- |
| `<隔离根>/.mode/.mode-root.json` 存在         | `true`                                |
| `<隔离根>/.mode/cli/log` 存在（CLI 布局哨兵） | `true`                                |
| stdout 解析失败行                             | `0`                                   |
| mock provider 收到的路径                      | 仅 `/v1/messages`（各用例次数见夹具） |

驱动拒绝把数据根指向真实 `~/.mode`（含旧根 `~/.zcodium`、`~/.zcode`），并覆盖
`MODE_PERSONAL_PROVIDER_CONFIG_FILE` / `MODE_BUILTIN_PROVIDER_CONFIG_FILE`；真实
`~/.mode` 不参与本次观察。

C2 夹具的可复现性已实测：`compact-manual`、`compact-no-instructions`、
`compact-expected-revision`、`context-usage-settings`、`memory-recall`、
`fresh-install-ledger`、`auto-compact-long-history`、`compact-provider-*` 连续两次探针运行
写出的内容逐字节相同（落盘后统一再跑 `oxfmt`）；`compact-same-tick` 因并发时序会出现
不同 trial 结果，夹具按 trial 记录实际值。

## 1. `session/compact`

### 1.1 参数与应答

线上接受的参数（`modeSessionCompactParamsSchema` 的运行时验证）：

```json
{
  "sessionId": "<id>",
  "inputId": "input_compact_manual_1",
  "instructions": "Focus on decisions and open questions; keep identifiers verbatim.",
  "expectedRevision": 10
}
```

- `inputId`、`instructions`、`expectedRevision` 都可省略；省略 `instructions` 时
  `turn.started.input` 是 `"/compact"`，带 instructions 时是
  `"/compact <instructions>"`。
- 有效 `expectedRevision` 取 `session/read` 的 `runtime.stateRevision`（4 轮 turn 后为 `10`，
  1 轮后为 `4`）；compact 成功后 `stateRevision` 递增（10 → 11）。
- 应答顶层键固定为 `response`、`snapshot`、`compact`：
  - 正常受理：`{response:"", snapshot:{…}, compact:{state:"accepted", inputId}}`；
  - 并发第二个：`{response:"", snapshot:{…}, compact:{state:"already_running", inputId}}`
    （见 §1.6，时序相关）；
  - `compact.operationId` **未观察到**；`compact.inputId` 回显请求值。夹具记录原始
    `inputIdEcho`（`requested` / `returned` / `equal:true`），不是规范化占位符。
- compact 是**异步受理**：`session/compact` 的应答先返回，摘要/失败通过后续事件与
  `turn.completed` / `turn.failed` 表达。
- `expectedRevision` 过期：JSON-RPC 错误应答；`compact-expected-revision` 记录
  `events:[]`，且 `dbBeforeRows` / `dbAfterRows` 的 8 张表行数逐表相等
  （`allTableRowCountsStable:true`），不是只比 message 行数：

```json
{
  "code": -32009,
  "data": { "actualRevision": 4, "expectedRevision": 1004 },
  "message": "Session state revision mismatch"
}
```

- 会话有 turn 正在运行：`-32010` / `"Cannot compact while a prompt is running"`；
  没有 `turn.*` 事件，`message`/`part`/`turn_usage`/`model_usage` 不新增 compact 行。

### 1.2 事件序列（手动 compact，带 instructions）

`session/event` 共 9 条（`compact-manual` 夹具，turnNumber=4，preCompactTokenCount=2792）：

| #   | type              | 关键载荷（观察值）                                                                                                                                                                                                                                                                                                                                     |
| --- | ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 0   | `turn.started`    | `turnNumber:4`、`input:"/compact Focus on decisions and open questions; keep identifiers verbatim."`、`inputId`、`inputVisibility:"model-only"`                                                                                                                                                                                                        |
| 1   | `session.updated` | `status:"started"`、`operationId`、`messageId`、`partId`、`trigger:"manual"`、`phase:"standalone_turn"`、`compactReason:"user_requested"`、`display:"separator"`、`preCompactTokenCount:2792`、`sourceCommandId`、`startedAt`                                                                                                                          |
| 2   | `session.updated` | `{messageCount:14, providerId:"oracle-mock", modelId:"mock-1", toolCount:"<count>"}`                                                                                                                                                                                                                                                                   |
| 3   | `session.updated` | `type:"model_request_started"`、`querySource:"compact"`、`modelCall.operation:"context_compaction"`、`modelCall.actorKind:"system"`、`attributes:{compactionOuterAttempt:1, compactionTrigger:"manual"}`、`callCause:"initial"`、`providerKind:"anthropic"`、`transport:"sse"`                                                                         |
| 4   | `session.updated` | `type:"model_request_completed"`、`finishReason:"stop"`、`usage:{inputTokens:10,outputTokens:12,totalTokens:22}`、`streamOutputCommitted:true`                                                                                                                                                                                                         |
| 5   | `session.updated` | `{content:"post compact answer: continuing after summary", stopReason:"stop", usage:{…22}, querySource:"compact", toolCallCount:0}`                                                                                                                                                                                                                    |
| 6   | `session.updated` | boundary：`boundaryId`、`trigger:"manual"`、`phase:"standalone_turn"`、`compactReason:"user_requested"`、`summarySource:"model"`、`preCompactTokenCount:2792`、`postCompactTokenCount:22`、`truePostCompactTokenCount:2852`、`summarizedMessageCount:8`、`keptMessageCount:0`、`summaryMessageIds:[…]`、`customInstructions:true`、`traceId`、`turnId` |
| 7   | `session.updated` | `status:"completed"`、`replace:true`、`boundaryId`、`summaryMessageId`、`tailStartMessageId`、`postCompactTokenCount:22`、`truePostCompactTokenCount:2852`、`endedAt`                                                                                                                                                                                  |
| 8   | `turn.completed`  | `response:"Compacted"`、`tokenCount:22`、`usage:{source:"provider", modelRequestCount:1, …}`、`resultType:"success"`、`historyRoundCount:1`、`cacheStats:{totalMessages:6,cachedMessages:5,lastCacheHit:false}`、`inputId`                                                                                                                             |

- **没有** `compact.started` / `compact.completed` 之类的独立事件类型；compact 的可观察信号
  全部落在 `session.updated` 与 `turn.completed` 上。
- `session.updated` 是并发状态推送，按 raw 顺序记录；模型请求类两条是
  `model_request_started` → `model_request_completed`。
- 不带 instructions 的 `compact-no-instructions` 事件类型序列与上表相同，仅
  `turn.started.input:"/compact"`、`preCompactTokenCount:2743`、
  `postCompactTokenCount:22`、`truePostCompactTokenCount:2849`、
  `summarizedMessageCount:4`、`customInstructions:false`。

### 1.3 compact 期间/之后的 DB 行

`db.sqlite` 只读观察（表名/列定义沿用 C1 契约 §3）。compact 成功后（4 turn 用例）：

| 表/列                     | 观察值                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| ------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `session.time_compacting` | 采样从发出请求持续到 accepted 对应的终态（`turn.completed` / `turn.failed`）才停止；窗口内 `distinctValues:["null"]`，结束后也是 `null`。**非空值未观察到**（见 §6）。`time_updated` 同时记录 before/after 关系：`changed:true`、`increased:true`（`compact-manual.timeUpdated`）                                                                                                                                                                                                                                                                     |
| `session_input`           | compact 带 `inputId` 时仍 **0 行**（与 C1 的 send 观察一致）                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `session_entry`           | 不新增，仍 3 行（`runtime/model_selection`、`runtime/bash_shell_selection`、`runtime/execution_state`）                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `message`                 | 新增 2 行：<br>1) 合成 timeline 消息：`role:"assistant"`、`finish:"completed"`、`semantics:{origin:"system",kind:"timeline_event",uiVisibility:"visible",providerVisibility:"hidden",transcriptVisibility:"visible"}`、tokens 全 0；<br>2) 摘要消息：`role:"user"`、`summary:{title:"Compact summary", body:"<模型返回文本>", diffs:[]}`、`semantics:{origin:"agent_runtime",kind:"compact_summary",uiVisibility:"hidden",providerVisibility:"visible",transcriptVisibility:"hidden"}`、`modelSelection`、`model`、`tools`，**没有** `time.completed` |
| `part`（timeline 消息）   | 2 行：`timeline`（`timelineType:"context_compaction"`、`display:"separator"`、`status:"completed"`、`operationId`、`sourceCommandId`、`trigger:"manual"`、`phase:"standalone_turn"`、`compactReason:"user_requested"`、`boundaryId`、`summaryMessageId`、counts、`time`）；`compaction`（`auto:false`、`timelineStatus:"completed"`、`replace:true`、`tail_start_id`、counts、`time`）                                                                                                                                                                |
| `part`（摘要消息）        | 2 行：`text`（`synthetic:true`，正文 = 固定前缀 + 模型摘要 + 固定续接提示；见下）；`compaction`（`tail_start_id`、`compactBoundary:{…summarySource:"model", summarizedMessageCount:8, keptMessageCount:0, customInstructions:true, traceId, turnId}`、`operationId`）                                                                                                                                                                                                                                                                                 |
| `turn_usage`              | 每个 compact 1 行：`user_message_id:null`、`status:"completed"`、`model_request_count:1`、`input_tokens:10`、`output_tokens:12`、`computed_total_tokens:22`、错误字段 `null`                                                                                                                                                                                                                                                                                                                                                                          |
| `model_usage`             | 每个 compact 1 行：`query_source:"compact"`、`id` 前缀 `usage_model_compact_`、`assistant_message_id:null`、`parent_user_message_id:null`、`finish_reason:"stop"`、`raw_usage_json`/`provider_metadata_json` 为 JSON 文本                                                                                                                                                                                                                                                                                                                             |

摘要消息的 `text` part 正文（观察值）：

```text
This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.

<模型返回的摘要正文>
Continue the conversation from where it left off without asking the user any further questions. Resume directly — do not acknowledge the summary, do not recap what was happening, do not preface with "I'll continue" or similar. Pick up the last task as if the break never happened.
```

行数（4 turn + 手工 compact）：`message=11`、`part=21`、`turn_usage=5`、`model_usage=5`、
`session_entry=3`、`session_input=0`。2 turn + 无 instructions 用例：
`message=7`、`part=13`、`turn_usage=3`、`model_usage=3`。

### 1.4 compact 的模型请求（provider 可见）

compact 使用与会话相同的 provider/模型；`querySource="compact"`。观察到的请求体：

- `model:"mock-1"`、`max_tokens:8192`、`stream:true`、`thinking:{"type":"disabled"}`；
- `system` 3 段：身份段（41 chars）、系统提示主体（1218 chars）、
  `# Communicating…` 起的第三段（compact 请求为 5404 chars；`memoryEnabled:true` 时
  同一段会变成 7255 chars，见 §3.2）；
- `messages` = 当前历史 + 末尾一条 `user` 摘要指令：以
  `"CRITICAL: Respond with TEXT ONLY. Do NOT call any tools."` 开头，包含
  `<analysis>/<summary>` 要求与示例，末尾是：
  - 有 instructions：
    `"Additional Instructions:\n<instructions>\n\nREMINDER: Do NOT call any tools. …"`；
  - 无 instructions：直接 `"REMINDER: …"`。
- `tools`：本次运行观察到 23 个（主 turn 请求为 21 个）；差异来自 `SendFile` /
  `HandoffMessage` 等工具面。工具面随运行环境变化，**不要把 23/21 写死**。
- `messageCount` 事件与请求中的消息数随历史增长；compact 请求本身消费一次剧本步
  （mock provider 的 assistant 计数决定返回文本）。

### 1.5 compact 之后的下一 turn 请求

`compact-manual` 在 compact 后立即再发一个 turn，捕获的 provider 请求：

- `messages` 角色只有 `["user", "system"]`；
- 唯一 user 消息包含 3 个 text part：
  1. `<system-reminder>` + `# currentDate`；
  2. 摘要续接文本（`This session is being continued…` + 摘要 + `Continue the conversation…`）；
  3. 新 prompt `"post compact prompt"`；
- system 消息为技能清单；**旧 turn 的消息不再出现**。
- 该请求 `tools` 为 21 个（与 compact 请求的 23 个不同）；工具面不作为 C2 契约。

### 1.6 并发第二次 compact：`already_running` 与 `skipped` 两种真实结果

`probe-c2-compact-race.mts` 在同一 tick 连发两个 `session/compact`，跑 4 轮 trial。
机器录制结果（`compact-same-tick`）：

| trial | 第一次   | 第二次            | DB 结果（`dbCounts` / `dbSkippedRows`）                                                               |
| ----- | -------- | ----------------- | ----------------------------------------------------------------------------------------------------- |
| 1     | accepted | `already_running` | 1 个摘要；`compaction` part 2 个；skipped 0                                                           |
| 2     | accepted | `already_running` | 1 个摘要；`compaction` part 2 个；skipped 0                                                           |
| 3     | accepted | `already_running` | 1 个摘要；`compaction` part 2 个；skipped 0                                                           |
| 4     | accepted | accepted          | 1 个摘要；`compaction` part 3 个；1 个 `finish:"skipped"` 消息 + 1 个 `timelineStatus:"skipped"` part |

以上是当前 checked-in `fixtures/c2/compact-same-tick.json` 的 `outcomes`（3×
`accepted+already_running`、1×`accepted+accepted`）。trial 结果随时序会切换；
重新跑探针时必须同时更新夹具与该表，禁止手改计数。

- `already_running` 应答形状：
  `{response:"", snapshot:{…}, compact:{state:"already_running", inputId}}`；
  第二次没有 `turn.started` / `turn.*` 事件，也没有新摘要（`statusEvents` 只含第一次）。
- `accepted` + skipped 形状：第二次先返回 accepted；第一次完成后，第二次发出
  `turn.started` + `session.updated{status:"skipped", operationId, sourceCommandId,
preCompactTokenCount, replace:true, startedAt, endedAt}` + `turn.completed`
  （`response:"Context is up to date; no compression needed"`、`tokenCount:2843`、
  `resultType:"success"`）。DB 新增一个 `finish:"skipped"` 的 timeline 消息与一个
  `timelineStatus:"skipped"` 的 `compaction` part，**不新增摘要**；两者的完整行在
  `compact-same-tick.json` 的 `dbSkippedRows`，不是只记计数。
- 因此「并发第二个一定 already_running」**不是稳定契约**；两种结果都真实存在，
  取决于第二个请求到达时第一个 compact 是否已在执行。C2 实现必须同时能表达这两种
  结果（或至少不能在录到 skipped 时崩溃）。
- 运行中 turn 的 compact 是另一条路径：`-32010 Cannot compact while a prompt is running`，
  不产生 compact 事件/DB 行（`compact-while-turn-running`）。
- `session.time_compacting` 的采样窗口覆盖到 accepted 对应的终态；两种并发下
  `distinctValues` 仍只有 `"null"`（`sampledUntilTerminal:true`）。

### 1.7 compact 快照的时序

- 第一次 accepted 应答的 `snapshot.runtime.contextUsage` 是**请求时**的快照
  （`used:22`、`size:200000`、带 `cache` 与 `breakdown`）；
  `snapshot.projection.contextUsed:22`、`turnCount:4`、`stateRevision:11`。
- 第二次 accepted（排队后执行）的应答快照已是 compact 后状态：
  `contextUsage:{cost:null,size:200000,used:2843}`（**没有** `cache`/`breakdown`），
  `projection.contextUsed:2843`、`turnCount:3`。
- 即 `session/compact` 的应答 snapshot 不保证是 compact 完成后的最终快照；T2 不能
  把 accepted 应答当成「摘要已落库」的证据。

### 1.8 compact 失败（模型响应不可用）

`probe-c2-compact-failure.mts` 用 mock provider 剧本制造两种失败；`session/compact`
仍然先返回 `{state:"accepted"}`，随后异步 `turn.failed`：

| 用例                         | provider 响应            | `turn.failed.error`                                                         | `retryable` |
| ---------------------------- | ------------------------ | --------------------------------------------------------------------------- | ----------- |
| `compact-provider-tool-call` | `tool_use`（Read）       | `code:"MODEL_ERROR"`、`message:"Tool use is not allowed during compaction"` | `false`     |
| `compact-provider-empty`     | `toolCalls:[]`（无文本） | `code:"MODEL_ERROR"`、`message:"Failed to generate compact summary"`        | `true`      |

共同观察：

- 事件序列：`turn.started` → `session.updated{status:"started"}` →
  `session.updated{messageCount…}` → 两条模型请求推送 →
  `session.updated{status:"failed", reason:"model_error", attempt:1, replace:true, …}` →
  `turn.failed`（`turnPhase:"compact"`、`inputId`）。
- DB：不写 `compact_summary`；新增 1 条 `message`（`finish:"failed"` 的 timeline 消息）+
  2 个 `part`（`timeline` `status:"failed"`、`compaction` `timelineStatus:"failed"`、
  `reason:"model_error"`、`attempt:1`）；原历史保留。完整行在夹具的
  `dbAfterRows`，不是只记计数。
- `session.time_compacting` 仍为 `null`。
- `turn_usage`：`user_message_id:null`、`status:"error"`、`error_type:"unknown_error"`、
  `error_code:"UNKNOWN_ERROR"`、tokens 全 0。
- `model_usage`：`query_source:"compact"`、`status:"completed"`、
  `assistant_message_id:null`、`parent_user_message_id:null`、`finish_reason:"tool-calls"`；
  tool-call 用例 `tool_call_count:1`、input/output 10/20；empty 用例
  `tool_call_count:0`、input/output 仍是 10/20（provider 的原始 usage 不变）。
- 网络层 provider 失败**未观察到**：尝试把隔离数据根的 personal provider 配置改成
  `http://127.0.0.1:1` 后 compact 仍使用原 mock（配置在运行时物化时缓存），因此只能
  录制「provider 响应不可用」这一类失败（见 §6）。

### 1.9 Rust C2 现状（T2a/T2b，2026-11-17）

T2a 的成功路径已验收；T2b 的并发/失败形状在同一任务内完成对拍，不再是「未实现」。
以下是与 §1.1–§1.8 观察值的实现注记。

- **受理快照**：`accepted` 应答的 `snapshot` 在受理临界区内构造，是**请求时**状态
  （与 §1.7 一致）；C1 的最小快照没有 `messages`/`breakdown`，本切片不补。受理推进
  一次 `stateRevision`，终态再推进一次（oracle 只观察到受理后的 10 → 11；终态 +1
  沿用 C1 的「每次接受/终态推进」规则）。`already_running` 不推进 revision，快照带
  `activeTurnId` + `activeTurnKind:"compact"`（§1.6 trial 1 形状）。
- **事件面**：Rust 产出 `turn.started` → `turn.completed`（成功/跳过）或
  `turn.failed`（provider / 配置失败）；oracle 的 `session.updated`
  （`status:"started"`、`model_request_started/completed`、边界、`status:"completed"`）
  不在产出面，与 C1 的「登记缺省事件类型」同一口径。
- **单飞**：同一会话同一时刻只执行一个 compact。执行中第二个返回
  `already_running`（不入队、不动 revision）；第一个仍在 **Queued** 窗口时到达的
  第二个按 §1.6 trial 4 的 `accepted → skipped` 形状执行——`turn.completed`
  `response:"Context is up to date; no compression needed"`，只落 skipped timeline
  消息 + 2 个 part，不写第二份摘要、不发起模型请求。其中 timeline part 为
  **家族一致性推断**（失败路径同形），探针过滤器只能裁定 compaction part，无法
  单独裁定 timeline part。
- **失败 compact 的 `contextUsage.used`**：Rust 只在 compact **成功**时让 usage 进入
  `runtime.contextUsage`（失败请求的 usage 只落 `model_usage`/`turn_usage` 行）；
  失败后的 read 视图保留主 prompt 的最后一次请求 total——oracle 的该维度未观察
  （§1.8 无 contextUsage 记录），登记为 C3 观察点。
- **skipped 的 `turn_usage`**：夹具 `compact-same-tick` 的 `dbSkippedRows` 只有
  message + compaction part，没有 skipped `turn_usage` 行；Rust 不写该行（不按
  「每个终态一行」的 C1 惯例补零值行）。
- **provider 失败判定**：按**实际工具调用数**区分两种失败，不看 `stopReason`——
  T0 的 empty 用例同样返回 `finishReason:"tool-calls"`（`toolCalls:[]`），
  `tool_call_count > 0` → `Tool use is not allowed during compaction`（retryable
  false），零调用 + 空文本 → `Failed to generate compact summary`（retryable true）。
  失败行按夹具：1 条 `finish:"failed"` timeline 消息 + 2 个 part（timeline/
  compaction `status:"failed"`、`reason:"model_error"`、`attempt:1`）、
  `turn_usage(error/UNKNOWN_ERROR/零 tokens)`、`model_usage` 保留 provider 原始
  usage 与 `finishReason:"tool-calls"`；原历史不动。
- **无模型 compact**：`turn.started` → `turn.failed`
  （`CONFIGURATION_ERROR` / `Select a model before continuing`）。compact 不进入
  prompt 的 `CommandInbox`，收口**显式**落 `Error`（不按 `inbox.complete` 的
  `Stale` 保持 Running）；该路径不写 compact message/part/model_usage 行
  （oracle 未观察），只写零用量 `turn_usage(failed)`。
- **resume 边界**：冷物化时以**最后一条** `compaction(timelineStatus:"completed")`
  所在消息为模型历史边界；边界之前的消息（含 tool pair）保留在 DB 但不进入模型
  请求，边界起（含摘要消息）照常恢复。失败/skipped 的 compaction part 不建立边界；
  多次 compact 取最新一条。
- **`preCompactTokenCount`**：本地历史估算（4 字符 ≈ 1 token，与 `trim_messages`
  同口径），不是 provider usage；`truePostCompactTokenCount = pre + post` 是可复算
  的本地定义（oracle 的公式未观察）。`postCompactTokenCount` 取 compact 请求的
  provider total。
- **摘要行 `tools`**：夹具只保留「对象」这一事实（折叠为 `<tool-flags>`），键集合
  未观察；Rust 写空对象占位，不冒充具体工具开关。
- **生成 id**：`operationId`/`boundaryId`/`traceId`/`spanId`/摘要消息 id 均为运行时
  生成的非空 id，只对齐夹具的「非空 id」形状；`compact.operationId` 仍恒省略。
- **`session.time_compacting`**：整个窗口与终态后保持 `null`（§1.3 观察值）。
- **自动 compact**：未实现（§2 观察为「未触发」）；15 轮 × 100k 字符的长历史只做
  普通模型请求，不产生 compaction/摘要/compact usage 行。
- **摘要续接文本**与 `summary.body`：前者逐字复用夹具的固定前缀/后缀，后者是模型
  返回原文；`session_entry` 不新增行，`session_input` 保持 0 行。

## 2. 自动 compact（长历史观察）

`probe-c2-auto-compact.mts`：脚本化 provider 每轮返回 100000 字符正文，连续 15 轮，
每轮后读 `session/read` 并直接计数 DB。机器录制（`auto-compact-long-history`）：

| 指标                                         | 观察值                                                                                         |
| -------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| 轮数                                         | 15                                                                                             |
| 最后 `contextUsage.breakdown.messages.chars` | `1,401,053`                                                                                    |
| `contextUsage.used` / `size`                 | `22` / `200000`                                                                                |
| `compactSummaryMessages` / `compactionParts` | `0` / `0`                                                                                      |
| `session.time_compacting`                    | `null`                                                                                         |
| compact 相关事件                             | 无                                                                                             |
| 事件类型（全量）                             | `session.titleUpdated`、`turn.started`、`session.updated`、`model.streaming`、`turn.completed` |

**未观察到自动 compact 触发**：15 轮、1.4M 字符历史下没有 compact 行或事件。不能从本
观察推导阈值；只记录「该规模 + 该 mock usage 下未触发」。注意 `contextUsage.used` 始终是
provider 报告的 last-request total（22），而 `breakdown.messages.chars` 是估算字符数；两者
不是同一事实源。自动 compact 是否使用字符估算、或只在 provider 报错/`used` 接近窗口时
触发，本环境无法区分（见 §6）。

## 3. Memory

### 3.1 目录与 workspaceId

`memoryEnabled:true` 时 oracle 会创建：

```text
<MODE_DATA_BASE_DIR>/.mode/cli/memories/projects/<workspaceId>/memory/
```

本文其余部分的 `<data-root>` 指 driver/隔离根，即 `MODE_DATA_BASE_DIR`；
`project-memory` 的 `dataRoot` 入参是 `<data-root>/.mode`。

路径层级：driver 设置 `MODE_DATA_BASE_DIR=<隔离根>`；oracle 在其下创建 `.mode`。
所以 `project-memory` 的 `dataRoot` 入参是 `<隔离根>/.mode`（Host `resolveDataRoot`
的形态），而不是 `MODE_DATA_BASE_DIR` 本身。T3 从 `MODE_DATA_BASE_DIR` 构造路径时
必须补 `.mode`；本探针记录的实际路径是
`<data-root>\.mode\cli\memories\projects\<workspace-id>\memory`（夹具
`memoryRoot` 由实际目录 `normalizeC2String` 得到，不是手写常量）。观察到的 `workspaceId`：

- 形状 `<slug>-<16 位小写十六进制>`；`slug` 是 cwd 基名的小写化（观察值
  `mode-c2-memory-ws-<随机后缀>` 对应 `mode-c2-memory-ws-<随机后缀>-<16hex>`）；
- 与 DB `session.workspace_id`（`null`）不同，也与 `session.project_id`
  （`proj_...`）不同；
- hash 的生成算法**未观察**（黑盒只看到 16 位十六进制结果），探针从实际目录读出
  workspaceId，不自行推导。

`memoryEnabled:false` 有两组对照：

- 全新数据根（`withoutMemoryFreshRoot`）：`memoriesRootExists:false`、
  `memoriesRootEntries:[]`，即不创建 `memories/`；
- 复用已有记忆目录的数据根（`withoutMemory`）：请求中没有 `# Memory` 段、没有
  `MEMORY.md` 块、没有任何 sentinel，即不读取已有目录。

### 3.2 `memoryEnabled:true` 的注入

用同一数据根重启（让运行时在 materialization 时读取刚写入的文件），发送
`"probe with memory"`。主请求观察：

| 项         | 观察值                                                                                                                                                                                                                                   |
| ---------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 文件       | `MEMORY.md`（index）与 `item-one.md`（item）由探针写入；内容见 `memory-recall` 输入的 sha256                                                                                                                                             |
| system 段  | 3 段；第三段 `blockIndex:2`、`blockChars:7255`，其中 `# Memory` 段 `startOffset:3233`、`endOffset:5084`、`length:1851`；`beforeHead` 结尾为 `...don't guess.`，`afterHead` 以 `# Environment` 开头（完整位置见 `memorySectionPosition`） |
| 路径       | `# Memory` 段内写绝对路径 `<data-root>\.mode\cli\memories\projects\<workspace-id>\memory/`，并声明「目录已存在，用 Write 直接写」                                                                                                        |
| 索引注入   | `MEMORY.md` 内容进入**首条 user 消息**的 `<system-reminder>` → `# agentsMd` 段，标题为 `Contents of <path> (user's auto-memory, persists across conversations):`，后面接 `# currentDate`；不是独立的 System 消息                         |
| item 注入  | `item-one.md` 的正文（`BLUE-LANTERN-9`）**没有**出现在主请求（`containsItemSentinel:false`）；只有索引 `MEMORY.md` 里的 `ALPHA-123` 出现                                                                                                 |
| 请求形状   | `messages` 角色 `["user","system"]`；user 的首个 text part 是 system-reminder，第二个是用户 prompt；system 消息是技能清单                                                                                                                |
| 记忆段开关 | 关闭时第三段 system 提示为 5404 chars，且没有 `# Memory` 段；开启时为 7255 chars                                                                                                                                                         |

即：**索引优先**。项目记忆的 item 文件未进入主请求；如果 C2 要实现 item 召回，必须由
额外观察支撑，不能从本夹具推导。

### 3.3 记忆写路径（memory extraction subagent）

`memoryEnabled:true` 的同一个 turn 还产生**第二个模型请求**。夹具的 `arrival` 记录：
extraction 请求到达时 `terminalCountAtArrival:1`、`lastEventTypeAtArrival:"turn.completed"`
（主请求为 `0` / `session.updated`），即请求在 turn 终态之后到达：

- 识别标记：最后一条 user 消息包含 `"You are now acting as the memory extraction subagent."`；
- 消息角色：`["user","system","assistant","user"]`；系统提示同样是 3 段（含 `# Memory`）；
- 最后一条 user 消息（观察值，节选）：
  - `"Analyze the most recent ~3 messages above and use them to update your persistent memory systems."`
  - `"Available tools: Read, Grep, Glob, read-only Bash …, and Edit/Write for paths inside the memory directory only"`
  - `"## Existing memory files\n\n- item-one.md (<time>)"`（`MEMORY.md` 不列为 item）
  - `"If nothing is worth saving, output only 'Nothing to save.'"`
- tools：23 个，包含 `Edit`、`Write`、`Bash`、`Read`（写入边界由提示词约束在记忆目录内）。
- 由于 mock provider 只返回文本（`"memory turn answer"`），没有实际写入；turn 前后
  `MEMORY.md` / `item-one.md` 的 size 与 sha256 完全不变。
- 结论：**存在 agent 记忆写入路径**（记忆抽取子代理 + Write/Edit 工具），但 T0 只观察到
  请求形状，未观察到真实写入、写入频率、失败/回滚语义。

### 3.4 `memoryEnabled:false` 对照

同一数据根、同一记忆文件、`memoryEnabled:false` 重启后发送
`"probe without memory"`：

- 只有 1 个模型请求；
- system 提示没有 `# Memory` 段；
- 首条 user 消息没有 `agentsMd`/`MEMORY.md` 段；
- `containsIndexSentinel:false`、`containsItemSentinel:false`；
- 没有 memory extraction 请求。

### 3.5 Rust C2 现状（T3，2026-11-17）

T3 实现只读召回：`memoryEnabled` 来自宿主对 `session/requestRuntimePreferences`
的应答（无 hub / 未应答时按保守默认关闭），`crates/agent-server/src/memory.rs`
加载（fail-soft），`crates/agent-core/src/context.rs` 的记忆槽装配。与
§3.1–§3.4 观察值的对照与偏差：

- **注入面**：`# Memory` 段并入唯一的系统消息末尾（T1 槽位），agentsMd 块并入
  请求里**首条 user 消息**文本的开头（reminder 之后空行接原 prompt）；
  `memory: None` 与 C1 v0 装配逐字一致。
- **请求形状**：保持 C1 的 `[System, User]` 装配。§3.2 观察的 provider 角色
  `["user","system"]`（user 先、system 技能清单在后）与 user 消息的两个 text part
  是 oracle 的 wire 形状，不在 provider 中立的 `ModelRequest` 层镜像（wire 投影归
  T4/T5 适配层）；reminder 与 prompt 并入同一文本（单 text part）。
- **注入文案**：`# Memory` 段与 agentsMd 块**逐字重建**自夹具
  `memory-recall.json` 的录制文本（机器录制的数据契约，与 §1.3 摘要续接文本同一
  口径，不是提示词创作），路径用真实目录替换（原生分隔符；`# Memory` 段路径带
  尾随 `/`，agentsMd 标题带 `\MEMORY.md`）。`# currentDate` 的日期格式未观察
  （夹具折叠 `<date>`），实现取 UTC 日历日 `YYYY-MM-DD`，登记为待观察假设。
- **workspaceId**：hash 算法未观察（§3.1），实现不推导——扫描
  `projects/` 下 `<slug(cwd 基名小写)>-<16 位小写 hex>` 形状的普通目录，恰好一个
  命中才使用；零命中 / 多命中 fail-soft（不注入）。索引读取走 `project-memory`
  公开 API（普通文件、无 symlink、大小写精确、5 MiB）。
- **目录不创建**（与 §3.1 的偏差）：oracle 在 `memoryEnabled:true` 启用时创建记忆
  目录；Rust 只读不创建。数据根没有 `memories/projects` 时（fresh root + enabled）
  不注入，等价 `withoutMemoryFreshRoot` 的观察面（无 `# Memory`、无 agentsMd、
  无 sentinel）。
- **fail-soft**：目录缺失、workspaceId 歧义、`MEMORY.md` 缺失 / 不可读、超限
  （5 MiB）一律不注入、turn 继续；错误不进模型上下文，不写 `turn.failed`。
  `memoryEnabled:false` 时零文件读取（spec §5.1）。目录存在但 `MEMORY.md` 缺失 /
  不可读的形态未观察，按同一 fail-soft 口径整体不注入（含 `# Memory` 段）。
- **fail-soft 静默（复核修复登记，2026-11-18）**：T0 没有观察到 oracle 的记忆失败
  诊断面（无日志/事件/协议字段观察），C2 **不发明**——失败路径在线上静默，不产出
  任何日志输出或协议/snapshot 字段。替代诊断是 per-session 运行时状态
  `SessionRuntime::last_memory_error`（`agent_core::Session::rule_store_error` 同款
  覆盖写语义：每个 prompt turn 的记忆加载结果覆盖写入——fail-soft 失败
  `Some(原因)`、加载成功或 `memoryEnabled:false` 关闭 `None` 清空；compact / 配置
  失败 turn 不写）。只供测试与宿主诊断读取，不影响 turn 行为。
- **写路径缺口**（§3.3）：memory extraction 子代理**未实现**——T0 只观察到请求
  形状，没有真实写入、写入频率、失败/回滚的观察；C2 不新增工具、不实现抽取
  子代理。enabled 会话只有主请求 1 个模型请求；
  `extractionInstruction` / `existingFilesBlock` 不产出。
- **读取时机**：实现按 turn 读取（每 turn 装配前重读）；§3.2 的探针注释暗示
  oracle 在 materialization 时读取。T0 没有「会话中途记忆文件变化」的可观察证据
  （写路径未实现，文件不会变），两种时机当前不可区分，登记为解释口径。
- **compact 请求**：不带记忆段（T2 的 compact 装配保持 `memory: None`）；带记忆
  的 compact 请求未观察（§1.4 的 compact 观察没有记忆开关维度）。

## 4. `contextUsage` 与 `settings.model.available`

### 4.1 `session/create` 时刻

`session/create` 返回的 9 个顶层键：`messages, projection, protocol, runtime, session,
settings, slashCommands, todos, todoGroups`。`runtime` 只有：

```json
{
  "eventSeq": 0,
  "goalVerifications": [],
  "goalVerificationTimeline": [],
  "pendingRequestIds": [],
  "stateRevision": 2
}
```

**没有 `contextUsage`，也没有 `deliveryKind`**；`contextUsageKeys = []`。

### 4.2 正常 turn 之后（`session/read`）

第 1/2/3 轮后 `runtime` 都有 `contextUsage`，键固定为
`breakdown, cache, cost, size, used`（**没有 `percent`**）：

```json
{
  "cache": {
    "inputTokens": 10,
    "cacheReadTokens": 0,
    "cacheWriteTokens": 0,
    "latestHitRate": 0,
    "hitRate": 0,
    "hitRateRequestCount": 3,
    "totalInputTokens": 30,
    "totalCacheReadTokens": 0,
    "totalCacheWriteTokens": 0
  },
  "cost": null,
  "size": 200000,
  "used": 22,
  "breakdown": [
    { "source": "system_prompt", "chars": "<count>" },
    { "source": "meta_user_context", "chars": "<count>" },
    { "source": "skills", "chars": "<count>" },
    { "source": "system_tool_schemas", "chars": "<count>" },
    { "source": "mcp_tool_schemas", "chars": "<count>" },
    { "source": "messages", "chars": "<count>" }
  ]
}
```

- `size` 来自模型能力 `contextWindow`（mock 配置为 `200000`）；
- `used` 是**最后一次模型请求的 total tokens**（mock 每次 10+12=22），不是累计值；
  三轮后仍是 22；
- `hitRateRequestCount` / `totalInputTokens` 随请求轮数累加（1→2→3，10→20→30）；
- `breakdown.chars` 随工具面/路径/插件缓存变化；夹具按 C1 规范折叠为 `<count>`。
  观察到的量级：`system_prompt` 约 6.7k（memory 关闭）/ 8.6k（memory 开启）、
  `system_tool_schemas` 约 81.9k、`mcp_tool_schemas` 约 4.1k、`messages` 随历史增长
  （长历史用例见 §2）；这些数字不作为精确断言；
- `projection`：`contextUsed:22`、`contextWindow:200000`、`turnCount` 1/2/3、
  `totalTokenCount` 22/44/66、`currentTurnId` 存在、`status:"idle"`；
- `runtime.deliveryKind:"desktop-continuous"`（read 视图；create 没有该字段）。

### 4.3 `close → resume → read`

`resume`/`read` 的 `runtime`：

```json
{
  "contextUsage": { "cache": { … }, "cost": null, "size": 200000, "used": 22 },
  "eventSeq": 2,
  "goalVerifications": [],
  "goalVerificationTimeline": [],
  "pendingRequestIds": [],
  "stateRevision": 0
}
```

- `contextUsage` 仍存在，但**没有 `breakdown`**（键为 `cache,cost,size,used`）；
- `stateRevision` 归 0，`eventSeq` 归 2；
- `projection` 计数归 0（`contextUsed:0`、`turnCount:0`、`totalTokenCount:0`），
  而 `contextUsage.used` 仍是 22：两套计数不是同一事实源（与 C1 观察一致）。

### 4.4 `settings.model.available`

`settings` 键：`mode, model, permission, thoughtLevel`。`settings.model` 键：
`available, current, lastUsed`。`available` 只有当前 provider 的 1 条：

```json
[
  {
    "ref": { "providerId": "oracle-mock", "modelId": "mock-1" },
    "label": "mock-1",
    "providerLabel": "Oracle Mock",
    "contextWindow": 200000,
    "maxOutputTokens": 8192,
    "reasoning": {
      "levels": [
        { "value": "disabled", "label": "disabled" },
        { "value": "enabled", "label": "enabled" }
      ],
      "defaultLevel": "enabled"
    },
    "properties": {
      "inputFormat": {
        "supportsText": true,
        "supportsImage": false,
        "supportsVideo": false,
        "supportsAudio": false,
        "supportsPdf": false
      },
      "outputFormat": { "supportsText": true }
    }
  }
]
```

- `current` 是 `{providerId, modelId, options:{reasoningLevel:"disabled"}}`；
  `lastUsed` 只有 `{providerId, modelId}`；
- `reasoning.defaultLevel` 是 `"enabled"`，与会话当前 `reasoningLevel:"disabled"` 不同；
- `settings` 中没有 `contextUsage`；`runtime.contextUsage` 才是 usage 事实源；
- `slashCommands` 含内置 `compact`（`description:"Compact the current conversation with optional instructions."`、`inputHint:"/compact [instructions]"`）；
  该条目在 `context-usage-settings.json` 的 `createSnapshot.compactSlashCommand` 逐字记录。

### 4.5 Rust C2 现状（T6，2026-11-18）

Rust `agent-server` 的快照/读面按 §4 对齐后的形状（`crates/agent-server/src/state.rs`
的 `build_snapshot` / `methods/mod.rs` 的 `cold_snapshot`；测试
`crates/agent-server/tests/snapshot.rs`）。逐条对照：

- **顶层 9 键常驻**：`slashCommands` / `todos` / `todoGroups` 恒出现（空会话为 `[]`）；
  `runtime.goalVerifications` / `goalVerificationTimeline` 恒为 `[]`（C1 观察形状，
  此前 Rust 缺省省略，T6 起补齐）。`session/goal` 仍是 `-32601`（分派表 default）。
- **`target` 键恒在（显式 `null`）**：快照的 `session.target`（C1 契约 §2.1 观察键）
  与 `projection.target`（`context-usage-settings.json` 的 projection 录制值）在无
  goal 事实时为 `"target": null` 而不是缺键；`session/list` 行不带 target（观察的
  `firstKeys` 无该键），两个投影面分开维护。
- **`settings` 4 键**：`mode` / `model` / `permission` / `thoughtLevel`。
  `permission` 为 `{mode}`（与 session 同源；无 `rulesRevision`）。
- **`settings.model.available` = hub 目录投影**：`ProviderConfigHub::provider_catalog`
  按会话 provider 取 resolver 全部模型（保序），逐条投影
  `ref{providerId,modelId}`（不带 options）、`label=modelId`、
  `providerLabel=providerName`、`contextWindow`/`maxOutputTokens`（模型规则数字）与
  `properties{inputFormat,outputFormat}`（协议必填形状；未配置的 `supports*` 按
  `false` 落，不编真值）。目录解析失败（文件层不可用等）退「无目录」：
  `available:[]`，create 不阻断（与能力的 fail-soft 同口径）。
- **`reasoning` 投影是登记的确定性选择**：`levels` 取
  `optionSpecs.reasoningLevel.values`（`value`=`label`=档位原文）；
  `defaultLevel` 取**最后一个档位**——与观察值 `["disabled","enabled"]→"enabled"`
  一致，但 oracle 的真实推导规则未观察（单点假设，若后续出现不同观察需重录）。
- **`thoughtLevel`**：`enabled` = 模型有档位（`levels` 非空；观察 `current:"disabled"`
  且 `enabled:true` 的形状）；`available` 与 `reasoning.levels` 同源同序；
  `current` 来自 thoughtLevel 事实（create/resume 参数、`session/setThoughtLevel`；
  冷恢复时回退到 `runtime/model_selection` entry 的 `options.reasoningLevel`——
  oracle 的真实恢复源未观察，Rust 用已持久化事实）；不产出 `defaultLevel`（观察无）。
- **`current` / `lastUsed`**：`current` 带 `options.reasoningLevel`（模型选择事实，
  create 参数 / entry / setModel 三处同源）；`lastUsed` 只有 `{providerId,modelId}`。
  无模型会话缺省省略（C1 裁决 7 口径不变）。
- **`projection.contextUsed` = 本 runtime 生命周期内最近一次模型请求的 total**
  （观察 22；不是累计——`totalTokenCount` 才是累计 22/44/66）；冷物化归 0，
  不从消息重算（与 `contextUsage.used` 的恢复语义不同源）。`currentTurnId` =
  本 runtime 生命周期内最近一次已开始的 turn（`turn.started` 时写，终态不清）；
  create/resume 后缺省省略。
- **`slashCommands` builtin 数据契约**（逐字，`source:"builtin"`，顺序取夹具录制顺序
  `goal, compact, init, plan`，非字母序）：
  `goal`（`Show or set the current session goal.` / `/goal [pause|resume|clear|replace <objective>|<objective>]`）、
  `compact`（`Compact the current conversation with optional instructions.` /
  `/compact [instructions]`）、
  `init`（`Create or update workspace AGENTS.md instructions.` / `/init [notes]`）、
  `plan`（`Switch to Plan mode and optionally send a task.` / `/plan [task]`）。
  没有 custom 来源（Rust 无自定义命令面）。

登记差异（Rust ≠ oracle 观察，不硬凑）：

| # | 项 | oracle 观察 | Rust 现状 |
| - | -- | ----------- | --------- |
| 1 | create `runtime.stateRevision` | `2` | `0`（revision 语义是「每次接受/终态 +1」，create 无接受动作） |
| 2 | resume 后 `runtime.eventSeq` | `2` | `0`（物化不发事件；oracle 的 resume 产生 2 条事件） |
| 3 | hot read `runtime.deliveryKind` | 有已投递输入后 `desktop-continuous`，resume 后没有 | 不产出（`session/subscribe` 的快照仍回显订阅链路，C1 口径不变） |
| 4 | `contextUsage.breakdown` | 完整 read 带 6 段 chars | 不产出（键固定 `cache,cost,size,used`；T1 起登记） |
| 5 | 冷读（close 后不 resume 的 read） | 未观察 | 顶层 9 键同面；`settings.model` 无事实（`available:[]`、无 current/lastUsed） |
| 6 | `cache.latestHitRate` / `hitRate` 公式 | 观察点恒 `0`（§4.2） | 非零公式（最近一次/合计的 `cache_read/(input+cache_read)`）是 T1 的确定性选择，真实推导未观察；汇总见 `tauri-agent-runtime.md` §C2.8 |

## 5. Fresh-install 迁移账本

`probe-c2-fresh-install.mts`：空隔离数据根启动 oracle（driver 会创建一次会话），只读
`db.sqlite` 的 `sqlite_master` 与 `schema_migration`；同一数据根再启动一次验证不重写。
**未读取 TypeScript 迁移实现**。

### 5.1 启动阶段

| 启动   | `startup/storageState` 帧数 | phase 序列                                                       |
| ------ | --------------------------- | ---------------------------------------------------------------- |
| 第一次 | 27                          | `checking ×3` → `migrating ×22` → `committing ×1` → `ready ×1`   |
| 第二次 | 5                           | `checking ×3` → `committing ×1` → `ready ×1`（没有 `migrating`） |

### 5.2 `schema_migration`（22 行）

`time_applied` 是机器时间，夹具折叠为 `"<time>"` 并只保留单调性；契约判定字段是
`id` / `checksum` / `app_version` / 顺序。

| id                                              | checksum                                                           | app_version |
| ----------------------------------------------- | ------------------------------------------------------------------ | ----------- |
| `0001_base_session_store`                       | `60e2d6a38ab36f31417c4f92c02690c96c7dcaaa0b6abe1741117d62a55c6462` | `0.2.0`     |
| `0002_local_setting`                            | `22a6ada9325c9ad55a00a1f0ecf72332c63c1e89fc3d91fec8d155bb0058b465` | `0.2.0`     |
| `0003_backfill_permission_local_setting`        | `bd880375ba7b948c8bcda847e7eb52bc568d065bcba48190f6c3bfb14d11b7dc` | `0.2.0`     |
| `0004_session_target`                           | `df670752991c78e38e2f25b0a003abc0e9689bc8d4894351358f2f83335a3ae1` | `0.7.0`     |
| `0005_session_target_accounting`                | `6138ed4562dfdd5b571d39d3b62266c55dd9eb8f2f41948e046441a0e4a954cc` | `0.7.0`     |
| `0006_input_history_attachments`                | `a2eab98649d738e15bdae27de7c5c114713f7777c55aa4111b19e03ee5ced54f` | `0.11.0`    |
| `0007_workflow_script_runtime`                  | `0068fc4bcaffe4de4669442a62eee227726e0e8e81b61c7458213a20ac596009` | `0.13.0`    |
| `0008_workflow_definition_scope`                | `f7ee304e4005c291fb8883cfc180005263e6c6b2f94077487443f2c17a71d3eb` | `0.13.0`    |
| `0009_session_title_metadata`                   | `3855cf957177ae6319ae91866cdff59e946ca07a10b36b5b59689818bd13fe00` | `0.14.0`    |
| `0010_usage_observability`                      | `36918b0a98f465fe844097aa60c65ef73ea9c62cc266f02742bf3fc2cedf860b` | `0.15.0`    |
| `0011_session_target_summary_title`             | `2b7723479426a4e7a1ed9901ed817495c1bbf63e9547eb1c63cd9e24bf9305f8` | `0.15.0`    |
| `0012_session_trace_id`                         | `9dcef90998dd00c8ed1b22a2170e180eb15471b93947e65ebe101d41e96bdb60` | `0.15.0`    |
| `0013_session_target_active_run_accounting`     | `7ab185540ebb7d26c5403ca52a50de6cf161c79cb93ccd47ff1b43b87415fe1c` | `0.15.0`    |
| `0014_message_part_sequence`                    | `66b45c45e4d3a1a60829f193f38d865dcdbd3de2eb78aa79ba954fe7ef1aab08` | `0.15.0`    |
| `0015_message_part_sequence_backfill_and_guard` | `da3046bf061ebb5ba253bb772f0fc9e4d1f2856dac4cdbc4cc0a65aae00e8511` | `0.15.2`    |
| `0016_session_input_ledger`                     | `18d51ae3f5e1425dc1e5c809282129fdc7430cacb6b1ce517b412ddbc34be790` | `0.15.2`    |
| `0017_session_input_start_now_delivery`         | `8c2da5985ecdf342438a2df713c9e18114276a0e85ce6f2e4f79bdf1596b52f8` | `0.15.2`    |
| `0018_session_input_failed_status`              | `a4d1a7b7c5d4af426b695769ed0f3a031efac8aa7af1f6412a85292c42b5d15b` | `0.15.2`    |
| `0019_dwf_journal`                              | `1c8da5568d2357f25c392be486fae80e1f3ad3ebc830dab1bee941634573e8ba` | `0.16.5`    |
| `0020_provider_model_selection`                 | `681180b4fcb497e13289b6970021678f24c975efdf3538777541105c5b2b444e` | `0.16.5`    |
| `0021_official_glm_selection`                   | `433a8da454682406e962b043f2e0f412e9801bf7e4641ff25760aa16625ebafe` | `0.16.5`    |
| `0022_backfilled_session_reasoning`             | `aee472fe499e9c25d2e71df0fbc68d92e462b30d1d02919ca35d540bf133c99a` | `0.16.5`    |

第二次启动后：`rowCount` 仍 22，`ids`/`checksums`/`app_version`/顺序逐字不变
（`ledgerStable:true`）。

### 5.3 `sqlite_master` 与 pragmas

第一次启动：`sqlite_master` 69 条（`index` 44、`table` 23、`trigger` 2），第二次启动
逐条不变（`sqliteMasterStable:true`）。完整 `sql`/`tbl_name` 在
`fixtures/c2/fresh-install-ledger.json` 的 `expect.firstStart.sqliteMaster.entries`，
不在这里重复（避免与真实库漂移）。deviation：T0 任务书写的是 session-store 子集，
这里有意保留全部 69 条（含 workflow/dwf 表）供 T7 对齐完整 schema；表名清单：

```text
dwf_actor, dwf_event, dwf_node, dwf_run, input_history, local_setting,
message, model_usage, part, permission, schema_migration, session,
session_entry, session_input, session_target, session_task_link, todo,
tool_usage, turn_usage, workflow_activity, workflow_definition,
workflow_event, workflow_run
```

触发器：`message_sequence_autofill`、`part_sequence_autofill`。

`pragma` 观察值（只读连接）：`user_version=0`、`journal_mode=wal`、
`page_size=4096`、`foreign_keys=1`。

### 5.4 空库的归属文件

第一次启动同样写出 `<隔离根>/.mode/.mode-root.json` 与 CLI 布局哨兵
`<隔离根>/.mode/cli/log`，`startup/storageState` 最终 `ready`；本次观察的会话表/消息表
由 driver 的 `session/create` 继续写入，与迁移账本互不影响。

### 5.5 Rust C2 现状（T7，2026-11-18）

Rust `session-store` 实现了 fresh-install 迁移（`crates/session-store/src/migrations.rs`），
门帧契约见 `agent-c2-context-memory.md` §6.1；与本文观察值的对照：

- **账本 22 行逐字节一致**：`id`/`checksum`/`app_version`/顺序按 §5.2 的 fixture
  录制值冻结在源码里；`time_applied` 用机器毫秒、同事务内非递减。第二次启动不重写
  账本（§5.2 的 `ledgerStable`）。
- **`sqlite_master` 69 条一致**：每条 CREATE 语句用 fixture `entries[].sql` 的
  oracle 原文加 `IF NOT EXISTS`（存储时被 SQLite 剥掉），新建库的 `sql` 文本、
  `tbl_name`、顺序（`order by type, name`，`sqlite_%` 除外）与 §5.3 一致；
  pragmas 里 `journal_mode=wal` 在 initialize 路径落盘，`foreign_keys` 是连接级
  pragma（fixture 的 `1` 是探针连接自己的设置），`page_size`/`user_version` 取默认。
- **门帧**：`checking ×3 → migrating ×22 → committing ×1 → ready ×1`（27 帧）与
  `checking ×3 → committing ×1 → ready ×1`（5 帧）按 §5.1 产出；migrating 帧带
  `completed/total/migrationId`，`migration` 事实的形状取自 C1 观察的
  `session-lifecycle-events.json`（`kind/executedCount/committedCount/
  lastAppliedMigrationId`）。
- **单步 SQL 切分不可观察**：22 步的对象归属按迁移名推导，纯数据回填/枚举扩列等
  无法推导的步登记为 no-op 占位或缺列守卫；可观察契约只有最终 schema + 账本字节
  （§7 第 7 条），两者均逐字节对齐。

## 6. 未观察 / 无法观察（明确登记）

| 观察项                                       | 尝试与结果                                                                                                                                                                                              |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `session.time_compacting` 进行中非空值       | 采样从请求发出持续到 accepted 对应的终态（`turn.completed`/`turn.failed`）才停止；手动与并发 trial 的 `distinctValues` 都只有 `"null"`（`sampledUntilTerminal:true`）。窗口内与结束后均未观察到非空值。 |
| 自动 compact 触发/阈值                       | 15 轮 ×100000 字符，`messages.chars=1,401,053`、`used=22`、`size=200000`，无 compact 行/事件。记录为「未观察」，不推导阈值。                                                                            |
| `already_running` 是唯一并发结果             | 4 trial 中 3 次 `already_running`、1 次 `accepted`+`skipped`；两种都真实存在。不能把 `already_running` 写成必然。                                                                                       |
| provider 网络失败                            | 把 personal provider 配置改为 dead port 后 compact 仍打原 mock（运行时缓存），未得到网络错误；只录到「provider 响应不可用」（tool_use / 空文本）。                                                      |
| 项目记忆 item 文件注入主请求                 | `item-one.md` 正文未出现在主请求；只观察到 `MEMORY.md` 索引。item 的选择/召回规则未观察。                                                                                                               |
| agent 实际写记忆                             | 观察到 memory extraction 子代理请求与 Write/Edit 工具，但 mock 只回文本，没有写盘；写入频率、失败/回滚未观察。                                                                                          |
| `workspaceId` hash 算法                      | 只观察到 `<小写 slug>-<16 hex>` 形状；无法从黑盒推导 hash 输入/算法。探针从实际目录发现 id。                                                                                                            |
| memory 读取失败路径（缺失/超限/symlink）     | T0 未构造；属 T3 的测试范围。                                                                                                                                                                           |
| `contextUsage.percent`                       | 不存在该字段；只有 `used` / `size`。                                                                                                                                                                    |
| `session.updated` 的具体 `toolCount` / chars | 随环境（插件缓存、工具面、路径长度）变化；夹具按 C1 规则折叠为 `<count>`。                                                                                                                              |
| Anthropic / Responses wire                   | 按 C2 计划不在 T0 oracle 范围；以公开 API 文档 + T4/T5 本地 SSE 夹具为准。                                                                                                                              |

## 7. 对 C2 实现的直接含义（只陈述观察差异）

1. `session/compact` 的 accepted 是**异步受理**；失败以 `turn.failed` 出现，不能只按
   应答判断成功。
2. 并发第二次的稳定部分只有「不会产生第二份摘要」；`already_running` 与
   `accepted→skipped` 都是真实结果，实现不能假定单一分支。
3. `session.time_compacting` 在「请求发出 → compact 终态」的采样窗口内始终为 `null`；
   C2 若按 spec 更新该列，需要补充可观察证据，否则只应保持 `null`/不依赖它。
4. 自动 compact 未观察到；C2 不应自行启用阈值。
5. Memory 注入是「系统提示中的 `# Memory` 段 + 首条 user 消息的 `agentsMd` 索引块」，
   不是独立 System 消息；`MEMORY.md` 索引优先，item 文件未进入主请求；写路径是
   记忆抽取子代理（Write/Edit 受提示词约束）。
6. `contextUsage.used` 是最后一条模型请求的 total tokens，不是累计上下文；
   resume 后 `contextUsage` 无 `breakdown`、`projection` 计数归 0。
7. Fresh-install 需要 22 条迁移账本与 69 条 `sqlite_master` 条目（含 workflow/dwf 表），
   id/checksum 以 §5.2 为准；第二次启动不得重写账本。

## 8. 复现与夹具对照

| 探针                           | 夹具                              | 内容                                                             |
| ------------------------------ | --------------------------------- | ---------------------------------------------------------------- |
| `probe-c2-compact.mts`         | `compact-manual.json`             | 4 turn + 带 instructions/expectedRevision 的 compact + 后续 turn |
|                                | `compact-no-instructions.json`    | 2 turn + 默认 compact（`customInstructions:false`）              |
|                                | `compact-expected-revision.json`  | 过期 expectedRevision 的 `-32009`                                |
| `probe-c2-compact-race.mts`    | `compact-same-tick.json`          | 4 trial 同 tick 双 compact：`already_running` / `skipped`        |
|                                | `compact-while-turn-running.json` | turn 运行中 compact 的 `-32010`                                  |
| `probe-c2-compact-failure.mts` | `compact-provider-tool-call.json` | provider 返回 tool_use → `turn.failed`（retryable=false）        |
|                                | `compact-provider-empty.json`     | provider 返回空文本 → `turn.failed`（retryable=true）            |
| `probe-c2-auto-compact.mts`    | `auto-compact-long-history.json`  | 15 轮长历史，自动 compact **未观察**                             |
| `probe-c2-memory.mts`          | `memory-recall.json`              | true/false 对照、MEMORY.md 注入、extraction 请求                 |
| `probe-c2-context.mts`         | `context-usage-settings.json`     | create/read/resume 的 contextUsage 与 model.available            |
| `probe-c2-fresh-install.mts`   | `fresh-install-ledger.json`       | 空库 22 条账本、69 条 sqlite_master、二次启动不变                |

回归约束：C1 的 38 条 Rust `agent-server` 三层回放在本次改动后仍全绿
（`pnpm test:agent-runtime-oracle` 中 38 条 `Rust agent-server 夹具回放：…` 全部
通过）；该套件另有 3 条 `session-stream` 基线红（C1 契约 §5 已登记，属既有问题，
与 `fixtures/c2/` 无关）。
