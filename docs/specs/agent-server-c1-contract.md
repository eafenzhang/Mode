# Spec：C1 Agent Server 黑盒契约（会话生命周期 / 存储行形状）

状态：**Task 0 捕获完成（2026-11-16）**。本文只记录 oracle（现网 Node `mode.cjs` 源码入口）
的黑盒观察结果，作为 C1 Rust `agent-server` / `session-store` 的对拍基线；不是产品设计稿。

关联：`docs/specs/agent-core-parity.md`、`docs/specs/tauri-agent-runtime.md`、
`docs/plans/2026-11-16-lt1-c1-rust-agent-server.md`、`docs/specs/clean-room-policy.md`。

## 0. 来源、纪律与复现

| 项            | 值                                                                                                             |
| ------------- | -------------------------------------------------------------------------------------------------------------- |
| oracle 入口   | `node node_modules/tsx/dist/cli.mjs apps/mode-cli/packages/cli/src/main.ts app-server --stdio`                 |
| 驱动          | `packages/desktop/tests/agent-runtime-oracle/driver.mts` 的 `startOracleRuntime`                               |
| mock provider | `127.0.0.1:0` 上的 `anthropic-messages` 端点；剧本由 `mock-provider.mts` 提供                                  |
| 隔离根        | `mkdtemp` 目录；同时注入 `MODE_DATA_BASE_DIR`、`MODE_HOME`、`HOME`、`USERPROFILE`                              |
| 会话参数      | `oracle-mock / mock-1`，`reasoningLevel:"disabled"`，`thoughtLevel:"disabled"`，`titleGenerationEnabled:false` |
| 录制脚本      | `packages/desktop/tests/agent-runtime-oracle/probe-session-lifecycle.mts`                                      |
| 夹具          | `packages/desktop/tests/agent-runtime-oracle/fixtures/session-lifecycle/session-lifecycle-{events,db}.json`    |

复现命令：

```bash
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-session-lifecycle.mts
pnpm exec oxfmt packages/desktop/tests/agent-runtime-oracle/fixtures/session-lifecycle/
```

- `expect` 全部由脚本从 oracle 运行结果投影/规范化后写入；脚本不含手写期望值。
- 连续两次运行（同机器、同隔离条件）产出的两个夹具**逐字节相同**。
- 夹具放在 `fixtures/session-lifecycle/` 子目录而不是 `fixtures/` 根下：`fixtures.test.mts` 会把
  根下每个 `*.json` 当作三层行为夹具并要求 case 名与 `FIXTURE_CASES` 逐字相等；本夹具是
  方法序/行形状夹具，不属于那套回放。

隔离断言（夹具 `expect.isolation`，两条都必须为真）：

| 断言                                          | 观察值                                           |
| --------------------------------------------- | ------------------------------------------------ |
| `<隔离根>/.mode/.mode-root.json` 存在         | `true`                                           |
| `<隔离根>/.mode/cli/log` 存在（CLI 布局哨兵） | `true`                                           |
| stdout 解析失败行                             | `0`                                              |
| mock provider 收到的路径                      | `["/v1/messages","/v1/messages","/v1/messages"]` |

驱动还拒绝把数据根指向真实 `~/.mode`（含旧根 `~/.zcodium`、`~/.zcode`），并覆盖
`MODE_PERSONAL_PROVIDER_CONFIG_FILE` / `MODE_BUILTIN_PROVIDER_CONFIG_FILE` 指向本次隔离根与仓库内
builtin 配置；真实 `~/.mode` 不参与本次观察。

## 1. 输入

工作目录只种一个文件：`hello.txt` = `"hello\n"`。mock 剧本按「请求里 assistant 消息条数」取步：

```json
[
  { "text": "first turn answer" },
  { "toolCalls": [{ "name": "Read", "input": { "file_path": "<cwd>/hello.txt" } }] },
  { "text": "second turn answer" }
]
```

两次 `session/send` 的 prompt 分别是 `"first prompt"` 与 `"Read hello.txt and summarize"`。
`session/create` 参数（线上逐字）：

```json
{
  "workspace": { "workspacePath": "<cwd>", "workspaceKey": "<cwd>" },
  "model": {
    "providerId": "oracle-mock",
    "modelId": "mock-1",
    "options": { "reasoningLevel": "disabled" }
  },
  "thoughtLevel": "disabled",
  "titleGenerationEnabled": false
}
```

## 2. 方法顺序（线上观察）

### 2.1 Host → runtime 请求

请求按下列顺序发出；`session/close` / `resume` / `read` / `messages` / `list` 在第二次 turn
终态之后才发出。

| #   | method              | params（规范化）                                                       | result 顶层键                                                                                  |
| --- | ------------------- | ---------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| 1   | `session/create`    | `workspace`、`model`、`thoughtLevel`、`titleGenerationEnabled`         | `messages, projection, protocol, runtime, session, settings, slashCommands, todos, todoGroups` |
| 2   | `session/subscribe` | `{sessionId, deliveryKind:"desktop-continuous", includeSnapshot:true}` | `eventSeq, events, sessionId, snapshot`                                                        |
| 3   | `session/send`      | `{sessionId, content:"first prompt"}`                                  | `accepted, sessionId, stateRevision`                                                           |
| 4   | `session/send`      | `{sessionId, content:"Read hello.txt and summarize"}`                  | `accepted, sessionId, stateRevision`                                                           |
| 5   | `session/close`     | `{sessionId}`                                                          | `closed`                                                                                       |
| 6   | `session/resume`    | `{sessionId}`                                                          | 与 `session/create` 相同的 9 个顶层键                                                          |
| 7   | `session/read`      | `{sessionId}`                                                          | 与 `session/create` 相同的 9 个顶层键                                                          |
| 8   | `session/messages`  | `{sessionId}`                                                          | `messages`                                                                                     |
| 9   | `session/list`      | `{includeArchived:true}`                                               | `sessions`                                                                                     |

关键返回值的观察值：

- `session/create`：`session.sessionId` 存在（顶层没有）；`session.title` 是 **空串**；
  `runtime.stateRevision = 2`；`runtime` 只有 `eventSeq/pendingRequestIds/goalVerifications/
goalVerificationTimeline/stateRevision`，**没有** `deliveryKind` 与 `contextUsage`；
  `session` 只有 `createdAt/mode/model/traceId/sessionId/sessionKind/status/target/title/
updatedAt/workspace`，**没有** `titleSource`。
- `session/subscribe`：`{eventSeq:0, events:[], sessionId, snapshot}`；`snapshot.runtime`
  多一个 `deliveryKind:"desktop-continuous"`。
- `session/send`：两次都是 `{accepted:true, sessionId, stateRevision}`；`stateRevision` 分别是
  **3** 与 **5**。
- `session/close`：`{closed:true}`，没有其他字段。
- `session/resume` / `session/read`：顶层与 create 相同，但 `messages` 有 **6** 条；
  `session.title = "first prompt"`、`session.titleSource = "first_input"`；
  `runtime` 增加 `contextUsage`（`cache` 10/0/0、`hitRateRequestCount:3`、`totalInputTokens:30`、
  `used:22`、`size:200000`、`cost:null`）；`runtime.eventSeq = 2`、`runtime.stateRevision = 0`。
  `projection` 仍是 `turnCount:0`、`totalTokenCount:0`、`contextUsed:0`（未按历史重算）。
- `session/messages`：`{messages:[{info,parts}]}`，6 条。`info` 键：
  `agent, cost, finish, messageId, model, parentMessageId, path, role, semantics, sessionId,
time, tokens`。消息与 part 类型：

  | #   | role      | finish       | parts                           |
  | --- | --------- | ------------ | ------------------------------- |
  | 0   | assistant | `completed`  | `timeline`                      |
  | 1   | user      | —            | `text`                          |
  | 2   | assistant | `stop`       | `step-start, text, step-finish` |
  | 3   | user      | —            | `text`                          |
  | 4   | assistant | `tool-calls` | `step-start, tool, step-finish` |
  | 5   | assistant | `stop`       | `step-start, text, step-finish` |

- `session/list`：`{sessions:[...]}`，1 条；每条键为
  `createdAt, mode, traceId, sessionId, sessionKind, status, title, titleSource, updatedAt,
workspace`，`title` 是首条 prompt。

### 2.2 Runtime → host 反向请求

`session/requestRuntimePreferences` 共出现 **4 次**（不是每 turn 都出现）：

| 时机                                              | `scope`                   |
| ------------------------------------------------- | ------------------------- |
| `session/create` 之后、`session/subscribe` 之前   | `runtime-materialization` |
| 第 1 次 `session/send` 之后（文本 turn 开始运行） | `user-execution`          |
| `session/resume` 之后、`session/read` 之前        | `runtime-materialization` |
| 同上                                              | `user-execution`          |

第 2 次 `session/send`（Read turn）期间**没有**新的反向请求：runtime 已由 create/resume 的
materialization 建好，turn 复用现有 runtime。`session/close` 也没有反向请求。

本用例的 `Read` 是只读工具，没有 `interaction/requestPermission`。反向请求的参数只有
`{sessionId, scope}`。

> 早期 `probe-tool-surface.mts` 的「一次会话只出现两次」对应**单 turn** 场景；本夹具进一步
> 钉住触发边界：新 runtime（create 或 resume）materialize 时一对，之后复用的 turn 不再发。

### 2.3 启动与状态通知

- `startup/storageState` 共 **27** 帧：`checking ×3` → `migrating ×22` → `committing ×1` →
  `ready ×1`。本次使用全新隔离根，`migration.kind = "initialize"`，`ready` 帧的
  `executedCount = committedCount = 22`。
  这是 **fresh-install** 路径；C1 只接已由 Node 迁移完成的数据根（缺账本 fail-closed）。
- `state.updated` 共 4 帧：每个 turn 一次 `prompt_started`（`patch.status:"running"`）与一次
  `prompt_completed`（patch 带 settings/model 等）。
- 被夹具排除的仪表帧（`expect.ignoredNotifications`）：`computer-use/operation-event ×6`、
  `v4/telemetry/event ×18`、`process/mcpTelemetry ×3`。它们不是 C1 会话契约。
- 一次观察到的环境相关计数（夹具折叠成 `<count>`）：`session.updated.toolCount = 21`；
  `contextUsageBreakdown[].chars = [6714, 41, 1031, 81943, 4076, 40]`（依次为
  `system_prompt / meta_user_context / skills / system_tool_schemas / mcp_tool_schemas / messages`）；
  `scheduled.inputByteLength = 96`。这些随 MCP/技能/工具面与路径长度变化，不能写进 C1 断言。

### 2.4 会话事件序列

`session/event` 共 **28** 条，`seq` 从 **1** 连续到 **28**；信封字段为
`deliveryKind, eventId, payload, seq, sessionId, timestamp, traceId, turnId, type`
（`session.titleUpdated` 同样带 `turnId`）。首条事件的信封（规范化后）：

```json
{
  "deliveryKind": "desktop-continuous",
  "eventId": "<id>",
  "payload": { "previousTitle": "", "source": "first_input", "title": "first prompt" },
  "seq": 1,
  "sessionId": "<id>",
  "timestamp": "<time>",
  "traceId": "<id>",
  "turnId": "<id>",
  "type": "session.titleUpdated"
}
```

| 阶段                                    | 事件（按序）                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| --------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 第一次 send 后                          | `session.titleUpdated`（`title:"first prompt"`、`source:"first_input"`）                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| 文本 turn                               | `turn.started`（`turnNumber:0`）→ `session.updated`（messageCount/provider/model/toolCount/iteration）→ `session.updated`（model_request_started）→ `model.streaming`（`text_delta`）→ `session.updated`（model_request_completed）→ `session.updated`（content/stopReason:"stop"/toolCallCount:0）→ `turn.completed`（`resultType:"success"`、`tokenCount:22`）                                                                                                                                                                                    |
| 工具 turn（Read）                       | `turn.started`（`turnNumber:1`）→ `session.updated` ×2 → `model.streaming` ×4（`tool_input_start`、`tool_input_delta`、`tool_input_end`、`tool_call`）→ `tool.updated`（`scheduled`）→ `tool.updated`（`started`）→ `session.updated` ×2（model_request_completed、stopReason:"tool-calls"）→ `tool.updated`（`result`）→ `tool.updated`（`batch`）→ `streamRecovery.updated`（`kind:"tool_result"`）→ `session.updated` ×2 → `model.streaming`（`text_delta`）→ `session.updated` ×2 → `turn.completed`（`resultType:"success"`、`tokenCount:52`） |
| close / resume / read / messages / list | **没有 session 事件**（`boundaries` 在 close 后到 list 后都停在 28）                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |

`session.updated` 是并发状态推送，按 `tauri-agent-runtime.md §3` 属无序组；本夹具按 raw 顺序
记录，对拍时按该表的规则折叠，不要把相邻 `session.updated` 的相对顺序当契约。
其余顺序逐字记录：`scheduled` 在 `started` 前、单次调用内部 `result` 在 `batch` 前、
`streamRecovery.updated` 在 `batch` 后。

`turn.completed.usage` 的 10 个键：`source, modelRequestCount, inputTokens, outputTokens,
totalTokens, cacheReadTokens, cacheWriteTokens, reasoningTokens, webFetchRequests,
webSearchRequests`。文本 turn 的 `usage` 为 10/12/22，工具 turn 为 20/32/52。

`tool.updated` 的 `scheduled` 载荷（观察）：`toolCallId, assistantMessageId, toolName,
dependencies, parallelGroupIndex, canRunParallel, schedule.parallelGroups,
schedule.executionOrder, kind:"scheduled", inputByteLength, inputOmitted:true,
inputRef:"model_stream"`；入参只在 `model.streaming` 的 `kind:"tool_call"` 里。
`result` 载荷：`{toolCallId, result:{success, content, perf:{totalMs}, truncated,
originalBytes, returnedBytes, budgetStrategy}, duration, kind:"result"}`。

## 3. `db.sqlite` 行形状

数据库路径：`<隔离根>/.mode/cli/db/db.sqlite`。只读连接用 Node `node:sqlite` 的
`DatabaseSync(path, { readOnly: true })`；行顺序统一 `select * from <table> order by rowid`。
JSON 文本列在夹具里已解析成对象。

### 3.1 列定义（PRAGMA table_info，observed）

```text
session (1 row)
id TEXT PK | project_id TEXT NOT NULL | workspace_id TEXT | parent_id TEXT | slug TEXT NOT NULL
directory TEXT NOT NULL | path TEXT | title TEXT NOT NULL | version TEXT NOT NULL | share_url TEXT
summary_additions INTEGER | summary_deletions INTEGER | summary_files INTEGER | summary_diffs TEXT
revert TEXT | permission TEXT | time_created INTEGER NOT NULL | time_updated INTEGER NOT NULL
time_compacting INTEGER | time_archived INTEGER | task_type TEXT NOT NULL DEFAULT 'interactive'
title_source TEXT NOT NULL DEFAULT 'first_input' | title_message_id TEXT | time_title_updated INTEGER
trace_id TEXT

message (6 rows)
id TEXT PK | session_id TEXT NOT NULL | time_created INTEGER NOT NULL | time_updated INTEGER NOT NULL
data TEXT NOT NULL | sequence INTEGER

part (12 rows)
id TEXT PK | message_id TEXT NOT NULL | session_id TEXT NOT NULL | time_created INTEGER NOT NULL
time_updated INTEGER NOT NULL | data TEXT NOT NULL | sequence INTEGER

session_input (0 rows)
id TEXT PK | session_id TEXT NOT NULL | kind TEXT NOT NULL | delivery TEXT NOT NULL
payload TEXT NOT NULL | admitted_sequence INTEGER NOT NULL | promoted_sequence INTEGER
promoted_message_id TEXT | status TEXT NOT NULL | status_reason TEXT
time_created INTEGER NOT NULL | time_updated INTEGER NOT NULL

session_entry (3 rows)
id TEXT PK | session_id TEXT NOT NULL | type TEXT NOT NULL | time_created INTEGER NOT NULL
time_updated INTEGER NOT NULL | data TEXT NOT NULL
```

### 3.2 `session`（1 行）

| 列                                                                         | 观察值 / 形状                                                                             |
| -------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| `id` / `slug`                                                              | `sess_<uuid>`；`slug` 与 `id` 相同                                                        |
| `project_id`                                                               | `proj_` + cwd 小写、非字母数字连续段折成单个 `-`（Windows 路径形如 `proj_c-users-...`）   |
| `workspace_id` / `parent_id`                                               | `null`                                                                                    |
| `directory` / `path`                                                       | 会话工作目录（同值）                                                                      |
| `title`                                                                    | 首条 prompt（`"first prompt"`）；**create 返回时快照里的 title 是空串**，首条 send 后才写 |
| `version`                                                                  | `"0.0.0"`（源码运行）                                                                     |
| `permission`                                                               | JSON 文本：`{"mode":"build"}`                                                             |
| `time_created` / `time_updated` / `time_title_updated`                     | epoch 毫秒                                                                                |
| `time_compacting` / `time_archived` / `summary_*` / `revert` / `share_url` | `null`                                                                                    |
| `task_type`                                                                | `"interactive"`                                                                           |
| `title_source`                                                             | `"first_input"`                                                                           |
| `title_message_id`                                                         | `null`                                                                                    |
| `trace_id`                                                                 | UUID（每次会话一个）                                                                      |

### 3.3 `message`（6 行）

`sequence` 是会话内 0 起的全局序号；`data` 是 JSON 文本。

| seq | role      | finish       | data 形状要点                                                                                                                                                                                                                                                                                                                                                                                        |
| --- | --------- | ------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0   | assistant | `completed`  | **合成 timeline 消息**：`parentID` 指向自己，`tokens` 全 0（无 `total`），`semantics.origin:"system"`、`kind:"timeline_event"`、`providerVisibility:"hidden"`；`modelId/providerId/mode/planEnabled/agent/path/cost` 存在。                                                                                                                                                                          |
| 1   | user      | —            | `time.created`；`modelSelection:{providerId,modelId,options:{reasoningLevel}}`；`contextSnapshot.envInfo:{cwd,platform,shell,osVersion,nodeVersion,isGitRepository,gitStatus}`；`semantics:{origin:"real_user",kind:"user_prompt",uiVisibility,providerVisibility,transcriptVisibility}`；`anchor:{turnId,origin:"realUser"}`；`tools:{<ToolName>:true,...}`；`model:{providerID,modelID,variant}`。 |
| 2   | assistant | `stop`       | 文本回复；`parentID` = 上一条 user id；`tokens:{total:22,input:10,output:12,reasoning:0,cache:{read:0,write:0}}`；`semantics.origin:"agent_runtime"`、`kind:"assistant_response"`；`anchor:{turnId, historyRoundCount:1, orderedMessageIds:[user,assistant], boundaryMessageId:<self>, goalBoundary:{kind:"none"}}`。                                                                                |
| 3   | user      | —            | 同 seq 1（第二条 prompt）。                                                                                                                                                                                                                                                                                                                                                                          |
| 4   | assistant | `tool-calls` | 工具轮：`tokens:{total:30,input:10,output:20,...}`；`anchor` **只有** `turnId`（没有 `historyRoundCount/orderedMessageIds/boundaryMessageId`）。                                                                                                                                                                                                                                                     |
| 5   | assistant | `stop`       | 工具后的收尾文本；`tokens:{total:22,...}`；`anchor:{turnId, historyRoundCount:2, orderedMessageIds:[3 条], boundaryMessageId:<self>, goalBoundary:{kind:"none"}}`。                                                                                                                                                                                                                                  |

所有 assistant 消息的公共字段：`role:"assistant"`、`time:{created,completed}`、`modelId`、
`providerId`、`mode:"build"`、`planEnabled:false`、`agent:"mode-agent"`、`path:{cwd,root}`、
`cost:0`、`semantics`（除合成消息外为 `agent_runtime/assistant_response/visible`）。

原始 id 形状：普通消息 `msg_<base36 时间戳>_<uuid>`；合成 timeline 消息
`msg_part_<base36 时间戳>_<uuid>_message`。

### 3.4 `part`（12 行）

每个消息内 `sequence` 从 0 起；`data` 是 JSON 文本。

| 消息               | seq | type          | data 形状                                                                                                                                                                                                                                           |
| ------------------ | --- | ------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 合成 timeline 消息 | 0   | `timeline`    | `{type:"timeline", timelineType:"model_change", display:"separator", status:"completed", time:{start,end}, toModel:{providerID,modelID,label}, fromModelSelection:{providerId,modelId,options,label}, toModelSelection:{providerId,modelId,label}}` |
| user #1            | 0   | `text`        | `{type:"text", text:"first prompt", time:{start,end}}`                                                                                                                                                                                              |
| assistant #2       | 0   | `step-start`  | `{type:"step-start"}`                                                                                                                                                                                                                               |
| 同上               | 1   | `text`        | `{type:"text", text:"first turn answer", time:{start,end}}`                                                                                                                                                                                         |
| 同上               | 2   | `step-finish` | `{type:"step-finish", reason:"stop", cost:0, tokens:{total:22,input:10,output:12,reasoning:0,cache:{read,write}}}`                                                                                                                                  |
| user #3            | 0   | `text`        | `{type:"text", text:"Read hello.txt and summarize", time:{start,end}}`                                                                                                                                                                              |
| assistant #4       | 0   | `step-start`  | `{type:"step-start"}`                                                                                                                                                                                                                               |
| 同上               | 1   | `tool`        | 见下                                                                                                                                                                                                                                                |
| 同上               | 2   | `step-finish` | `{type:"step-finish", reason:"tool-calls", cost:0, tokens:{total:30,input:10,output:20,...}}`                                                                                                                                                       |
| assistant #5       | 0   | `step-start`  | `{type:"step-start"}`                                                                                                                                                                                                                               |
| 同上               | 1   | `text`        | `{type:"text", text:"second turn answer", time:{start,end}}`                                                                                                                                                                                        |
| 同上               | 2   | `step-finish` | `{type:"step-finish", reason:"stop", cost:0, tokens:{total:22,...}}`                                                                                                                                                                                |

`tool` part 的 `data`（观察值，路径用占位符）：

```json
{
  "type": "tool",
  "callID": "<tool-call-id>",
  "declarationIndex": 0,
  "tool": "Read",
  "state": {
    "status": "completed",
    "input": { "file_path": "<cwd>/hello.txt" },
    "output": "1\thello\n2\t",
    "title": "Read",
    "metadata": {
      "schemaVersion": 1,
      "serialization": {
        "truncated": false,
        "originalBytes": 10,
        "returnedBytes": 10,
        "budgetStrategy": "truncate"
      },
      "readFileState": {
        "schemaVersion": 1,
        "tool": "Read",
        "path": "<cwd>\\hello.txt",
        "content": "hello\n",
        "isPartialView": false,
        "readAtMs": "<time>",
        "revisionId": "mtime:<n>:size:6",
        "mtimeMs": "<time>",
        "sizeBytes": 6
      }
    },
    "time": { "start": "<time>", "end": "<time>" }
  }
}
```

原始 part id：普通部件 `part_<base36 时间戳>_<uuid>`；timeline 部件
`part_part_<base36 时间戳>_<uuid>_timeline`。

### 3.5 `session_input`（0 行）

本次生命周期（create + 两次 send + close + resume + read/messages/list）**没有写入任何
`session_input` 行**；表存在且列如上。为了排除「只是缺参数」的解释，探针还提供
`--input-variants` 模式（不产出夹具），用同一 driver 跑三种变体：

- `persistence:"deferred"` + `inputId`
- 默认 persistence + `inputId`/`queryId`
- `persistence:"immediate"`

复现命令与观察值：

```bash
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-session-lifecycle.mts --input-variants
# { "variants": ["deferred+inputId","default+inputId+queryId","immediate"],
#   "totalSessionInputRows": 0 }
```

⇒ `agent-core-parity.md §5.1` 里「`session/create` 落 session + 初始 `session_input`」这句
**与本次 stdio 观察不符**。C1 不应为了对齐这句话凭空写 `session_input`；v4 `command` 路径
（`packages/shared/src/mode-protocol-v4/command.ts` 的 `inputAccepted.inputId` / `delivery`）
才使用该表，C1 的 v1 `session/*` 面未观察到写入。

### 3.6 `session_entry`（3 行）

| id（原始模式）                             | `type`                         | `data`                                                                                                                                                                 |
| ------------------------------------------ | ------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `<sessionId>:runtime-model-selection`      | `runtime/model_selection`      | `{"modelSelection":{"providerId":"oracle-mock","modelId":"mock-1","options":{"reasoningLevel":"disabled"}}}`                                                           |
| `<sessionId>:runtime:bash_shell_selection` | `runtime/bash_shell_selection` | `{"dialect":"git-bash","display":{"name":"Git Bash"},"id":"auto:git-bash","label":"Git Bash","path":"C:\\Program Files\\Git\\bin\\bash.exe","source":"auto-detected"}` |
| `<sessionId>:runtime-execution-state`      | `runtime/execution_state`      | `{"mode":"build","planEnabled":false}`                                                                                                                                 |

注意第 1 个 id 用 `:runtime-`，第 2 个用 `:runtime:`——两种分隔符都是线上事实，不要归一。
`bash_shell_selection` 的 shell/path 是机器环境值；夹具已折叠成 `<shell-*>`/`<bash-path>`，
C1 不能把本机 Git Bash 的路径写成固定值。

### 3.7 wire（`session/messages`）与 DB 的差异

`session/messages` 返回的是**投影**，不是 `message.data` / `part.data` 原样：

- `info` 有 `messageId/model/path/role/semantics/time/tokens...`，但**没有** DB 里的
  `modelSelection`、`contextSnapshot`、`tools`、`anchor`；`model` 也是 `{providerId,modelId}`，
  而 DB 用 `modelId`/`providerId` + `model:{providerID,modelID,variant}`。
- part 用 `partId`（不是 DB 的 `id`）、`callId`（不是 DB 的 `callID`）；tool 的 `state` 用
  `startedAt`/`completedAt`（不是 DB 的 `time.start/end`），且 `metadata` **丢掉**
  `readFileState`（DB 保留）。
- 因此 C1 的 `session-store` 必须写 DB 的富形状；`session/messages` 按协议投影，不能把
  投影后的 part 写回 `part.data`。

## 4. 与 C1 spec / 计划的差异与未决

| #   | 观察                                                                                                                                     | 对 C1 的含义                                                                                                     |
| --- | ---------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| 1   | `session_input` 在 v1 stdio 生命周期里 0 行（含 deferred/inputId 变体）                                                                  | 不要凭空写 `session_input`；`agent-core-parity.md §5.1` 的「初始 session_input」需按实际观察修正或标为 v4 路径。 |
| 2   | `session/requestRuntimePreferences` 在 create 与 resume 各一对；首次 send 复用 create 的 runtime，第二次 send 复用同一 runtime，都不再发 | 反向请求按「新 runtime materialize」触发，不按每个 turn 触发。                                                   |
| 3   | `session/create` 的 `session.title` 是空串；首条 send 后 `session.titleUpdated` 才给出 `first prompt`                                    | DB `session.title` 在 create 时与快照可能不同步；C1 写库/快照要按此顺序。                                        |
| 4   | `resume`/`read` 的 `projection` 计数仍是 0（turnCount/contextUsed/totalTokenCount），但 `runtime.contextUsage` 已带 22                   | 两套计数不是同一事实源；C1 先按快照字段原样返回，不自行重算。                                                    |
| 5   | oracle 产出 `session.updated` / `streamRecovery.updated`                                                                                 | C1 可以缺省（spec §5.2 已允许），但必须在差异表登记「未产出」，不能伪造。                                        |
| 6   | 全新隔离根走 `startup/storageState` initialize（22 条迁移）                                                                              | 本夹具来自 fresh-install 路径；C1 只读已迁移库、缺账本 fail-closed，两者不能混用。                               |
| 7   | `session/close` / `resume` / `read` / `messages` / `list` 不产生 session 事件                                                            | 事件流在 close 后不出现终态类事件；订阅方不能等事件判断 close 完成。                                             |
| 8   | `session/messages` 的投影比 DB 窄（见 §3.7）                                                                                             | 对拍要分「wire 投影」与「DB 行」两层，不能拿 `messages` 结果反推 `part.data`。                                   |

## 5. 回放状态（Task 0 Step 3）

命令：

```bash
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-session-lifecycle.mts --replay
```

在 `crates/agent-server` 尚未实现时，观察到的**红**：

```text
exit code 1
回放为红：agent-server 二进制不存在（MODE_AGENT_SERVER_COMMAND 未设置，
target/{debug,release}/{agent-server,mode-agent} 均不存在）；C1 Task 1 之后才会产出该二进制
```

这是按计划的「binary 不存在」红因；Task 10 再把 `--replay` 扩展成三层同构回放。
`--replay` 现在只校验夹具可读与二进制可拉起，不把 oracle 的期望值当成 Rust 已通过。

## 6. 复现证据

- 两次连续运行 `probe-session-lifecycle.mts`，`session-lifecycle-events.json` 与
  `session-lifecycle-db.json` 均 `diff` 为空（逐字节相同）。
- 夹具中不含原始 uuid、真实用户名或隔离根绝对路径；机器相关值已折叠为
  `<id>/<time>/<cwd>/<data-root>/<count>/<headers>/<tool-flags>/<shell-*>`。
- `validate`：`pnpm exec oxlint` 对两个探针文件 0 warning / 0 error；夹具由
  `pnpm exec oxfmt` 统一格式。
