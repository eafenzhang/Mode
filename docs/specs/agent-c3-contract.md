# Spec：C3 Agent Server 黑盒契约（goal / stream recovery+retry / background / subagents）

状态：**C3-T0 捕获完成（2026-11-18）；T1（goal）Rust 实现已落（§1.11）**。本文只记录
oracle（现网 Node `mode.cjs` 源码入口）的黑盒观察结果，作为 C3 Rust `agent-server` /
`agent-core` 的对拍基线；不是产品设计稿。实现细节以
`docs/specs/agent-c3-recovery-goal.md` 为准，本文只回答「线上到底长什么样」。

关联：`docs/specs/agent-c3-recovery-goal.md`、`docs/plans/2026-11-18-lt1-c3-recovery-goal.md`、
`docs/specs/agent-server-c1-contract.md`、`docs/specs/agent-c2-contract.md`、
`docs/specs/mode-api-retry.md`、`docs/specs/tauri-agent-runtime.md`、
`docs/specs/clean-room-policy.md`。

## 0. 来源、纪律与复现

| 项            | 值                                                                                                                             |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| oracle 入口   | `node node_modules/tsx/dist/cli.mjs apps/mode-cli/packages/cli/src/main.ts app-server --stdio`                                 |
| 驱动          | `packages/desktop/tests/agent-runtime-oracle/driver.mts` 的 `startOracleRuntime`                                               |
| mock provider | `127.0.0.1:0` 上的 `anthropic-messages` 端点；C3 扩展见 `mock-provider.mts`（`failure`/`delayMs`/`textChunks`/attempt 上下文） |
| 隔离根        | `mkdtemp` 目录；同时注入 `MODE_DATA_BASE_DIR`、`MODE_HOME`、`HOME`、`USERPROFILE`                                              |
| 会话参数      | `oracle-mock / mock-1`，`reasoningLevel:"disabled"`，`thoughtLevel:"disabled"`，`titleGenerationEnabled:false`                 |
| 录制脚本      | `probe-c3-goal.mts` / `probe-c3-retry.mts` / `probe-c3-background.mts` / `probe-c3-subagents.mts`                              |
| 夹具          | `packages/desktop/tests/agent-runtime-oracle/fixtures/c3/*.json`                                                               |

复现命令（从仓库根目录执行）：

```bash
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c3-goal.mts
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c3-retry.mts
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c3-background.mts
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c3-subagents.mts
pnpm exec oxfmt packages/desktop/tests/agent-runtime-oracle/fixtures/c3/
```

- 所有 `expect` 由脚本从本次 oracle 运行的事件/应答/DB 行投影后写入，不含手写期望值。
- 连续两次运行四个探针并用 oxfmt 格式化后，`fixtures/c3/` 下全部 24 个夹具逐字节相同
  （本机 2026-11-18 验证）。
  退避间隔、目标计时、PID、分页游标等运行相关值在 `probe-c3-support.mts` 折叠成
  `<retry-delay>`、`<time>`、`<pid>`、`<cursor>`；原始数值只写在探针 stdout 的摘要里，不写进夹具。
- **身份别名**：目标 id 在夹具内按首次出现映射为 `target-1`、`target-2`（同一夹具内
  同一个目标始终同一别名；replace/覆盖产生的新目标得到新别名，因此「id 是否变化」
  可验证）。子会话 id 按 `childSessionIds` 的创建顺序映射为 `child-1`、`child-2`；
  `childSessionId`/`childSessionIds` 与列表项共用同一映射。原始随机 id 不写进夹具。
- 隔离断言（每个夹具的 `expect.isolation`，两条都必须为真）：

| 断言                                          | 观察值                                                                   |
| --------------------------------------------- | ------------------------------------------------------------------------ |
| `<隔离根>/.mode/.mode-root.json` 存在         | `true`                                                                   |
| `<隔离根>/.mode/cli/log` 存在（CLI 布局哨兵） | `true`                                                                   |
| stdout 解析失败行                             | `0`                                                                      |
| mock provider 收到的路径                      | 全部为 `/v1/messages`（标题生成关闭；goal verifier 也是 `/v1/messages`） |

驱动仍拒绝把数据根指向真实 `~/.mode`（含旧根 `~/.zcodium`、`~/.zcode`），并覆盖
`MODE_PERSONAL_PROVIDER_CONFIG_FILE` / `MODE_BUILTIN_PROVIDER_CONFIG_FILE`。本文没有读取
`apps/mode-cli/**`、`packages/services/**`、`packages/client/**` 的实现体。

## 1. Goal（`session/goal`）

### 1.1 参数、应答与 action 语义

参数（线上 `.strict()`，协议镜像 `modeSessionGoalParamsSchema`）：

```json
{
  "sessionId": "sess_…",
  "inputId": "…",
  "action": "show|set|replace|pause|resume|clear",
  "objective": "…",
  "expectedRevision": 5
}
```

- `inputId` / `objective` / `expectedRevision` 可选；未知键直接 `-32602`。实测传
  `tokenBudget:100` 得到 `Unrecognized key: "tokenBudget"`，因此**当前协议面没有预算设置入口**。
- 应答恒为 `{response, snapshot, startedTurn}` 三键；`startedTurn` 即使为 `false` 也出现。
  `snapshot` 是完整会话快照（与 `session/read` 同源），不是精简的 target。

| action                   | 观察 `response`                                                             | `startedTurn` | 启动的 turn                                                                                |
| ------------------------ | --------------------------------------------------------------------------- | ------------- | ------------------------------------------------------------------------------------------ |
| `show`（无 goal）        | `No goal is set. Use /goal <objective> to set one.`                         | `false`       | 无                                                                                         |
| `show`（paused）         | `Goal paused\nObjective: …\nUsage: 0 tokens / none\nTime: 6 seconds`        | `false`       | 无                                                                                         |
| `show`（complete）       | `Goal complete\nObjective: …\nUsage: 22 tokens / none\nTime: 10 seconds`    | `false`       | 无                                                                                         |
| `set`（无 goal）         | `Goal active\nObjective: …\nUsage: 0 tokens / none\nTime: 0 seconds`        | `true`        | 先 control-only turn，再 goal-continuation                                                 |
| `replace`（无 goal）     | 同 `set`（等同创建）                                                        | `true`        | 同 `set`                                                                                   |
| `replace`（paused）      | `Goal active\nObjective: <新目标>\nUsage: 0 tokens / none\nTime: 0 seconds` | `true`        | 推断：同 `set`                                                                             |
| `pause`（无 goal）       | `No goal to pause.`                                                         | `false`       | 无                                                                                         |
| `pause`（active）        | `""`（空串）                                                                | `false`       | 无；当前 continuation 被取消（provider 请求前 `model_request_count:0`；in-flight 见 §1.6） |
| `resume`（无 goal）      | `No goal to resume.`                                                        | `false`       | 无                                                                                         |
| `resume`（paused）       | `Goal resumed\nObjective: …\nUsage: … tokens / …\nTime: … seconds`          | `true`        | goal-continuation                                                                          |
| `clear`（无 goal）       | `No goal to clear.`                                                         | `false`       | 无                                                                                         |
| `clear`（idle）          | `Goal cleared.`                                                             | `false`       | 无                                                                                         |
| `set` 缺 `objective`     | `Usage: /goal <objective>`                                                  | `false`       | 无                                                                                         |
| `replace` 缺 `objective` | `Usage: /goal replace <objective>`                                          | `false`       | 无                                                                                         |

`replace` 的 `response` 与 `set` 逐字相同；落盘也只有一条 `session_target` 行（新目标覆盖旧目标）。

无 goal 时的副作用（`goal-show-set-complete.json` 的 `noGoalEvents`）：

- 首个 mutating action `pause` 触发 `session.titleUpdated`
  `{source:"first_input", previousTitle:"", title:"/goal paused"}`；
- `clear` 触发 `session.updated {action:"cleared", source:"command", previousTarget:null,
target:null}`；
- `show` / `resume` 本身没有观察到状态事件。`set` 之前的 no-goal 动作不启动 turn。

**运行中限制（重要，与「show 只读」的直觉不同）**：存在运行中的 prompt 时，
`show` / `set` / `replace` / `resume` / `clear` 都返回：

```json
{ "code": -32010, "message": "Cannot manage goals while a prompt is running" }
```

`pause` 是唯一在运行中被接受的 action。它立即把目标置为 `paused`
（`status_updated source:"command"`），并取消当前 continuation：
`goal-pause-resume-clear.json` / `goal-user-stop.json` 里 pause/stop 发生在
provider 请求之前，事件是 `status_updated(paused) → run_finished → turn.completed(cancelled)`，
没有 `model_request_started/completed`，`turn_usage.model_request_count:0`、
`cancelled_by_user:1`。在 `model_request_started` 之后 pause 的账务记录在
`goal-pause-in-flight.json`（见 §1.6）。

### 1.2 `set` / `replace` 的 turn 序列

`set`（或 `replace`）后线上事件顺序（夹具 `goal-show-set-complete.json`）：

1. `turn.started`：`{turnNumber:0, executionKind:"controlOnly", input:"<objective>", messageId}`
   —— 这是控制 turn，不发模型请求。
2. `turn.completed`：`{response:"", tokenCount:0, toolCallCount:0, resultType:"success"}`。
3. `session.updated action:"summary_updated" source:"runtime"`：`previousTarget.summaryTitle:null`，
   `target.summaryTitle:"<objective>"`。
4. `session.updated action:"set" source:"command"`：`previousTarget:null`（首次）或旧目标；
   `target.summaryTitle:null`（注意与第 3 步的值不同，raw 顺序就是 summary_updated 在前）。
5. `turn.started`：goal-continuation turn：
   - `turnNumber` 递增（首次 `1`）；
   - `inputSource:"goal-continuation"`、`inputVisibility:"model-only"`、`targetId`；
   - `input` 是 `<system-reminder>` 包裹的续跑提示，含 `<untrusted_objective>`、`Budget:`
     （`Time spent pursuing goal: N seconds`、`Tokens used: N`、`Token budget: none`、
     `Tokens remaining: unbounded`）以及 completion-audit 规则。逐字全文在夹具
     `eventSummary.turnStarts[1].input`。
6. `session.updated action:"run_started" source:"runtime"`：`target.activeInputId = inputId`
   （如 `goal_set_main`）、`activeRunStartedAtMs/activeRunLastSeenAtMs` 写入；target 全量。
7. 正常模型 turn 事件；`session.updated action:"run_finished" source:"runtime"`：累计
   `tokensUsed/timeUsedSeconds`，清空 active 字段。
8. `turn.completed`（正常成功）。
9. `session.updated`（verification `status:"started"`）→ verifier 模型请求 →
   `session.updated`（verification `status:"completed"`）。
10. 通过时 `session.updated action:"status_updated" source:"runtime"`：target `status:"complete"`。

`replace` 的目标 id 会变化：`goal-replace-revision-conflict.json` 中旧目标是
`target-1`、新目标是 `target-2`（`set` 事件 previousTarget.targetID=target-1、
target.targetID=target-2）；`session_target` 仍是单行（`session_id` 主键），
旧目标不保留。`set` 覆盖已有目标同样产生新 id（`goal-set-over-existing.json`：
`target-1 → target-2`，previousTarget 非 null）。

每个由目标驱动的模型 turn 都包在 `run_started` / `run_finished` 中；后台通知 turn
（§3）也会包在同一对事件里并计入目标 `tokensUsed`。

### 1.3 `session_target` 行形状

`db.sqlite` 的 `session_target`（PRAGMA table_info 观察，按 rowid）：

```text
session_id TEXT PK | target_id TEXT NOT NULL | objective TEXT NOT NULL
status TEXT NOT NULL CHECK(status IN ('active','paused','budget_limited','complete'))
token_budget INTEGER | tokens_used INTEGER NOT NULL DEFAULT 0
time_used_seconds INTEGER NOT NULL DEFAULT 0 | time_created INTEGER NOT NULL
time_updated INTEGER NOT NULL | summary_title TEXT | active_input_id TEXT
active_run_started_at INTEGER | active_run_last_seen_at INTEGER
```

列名/类型/notNull/PK/default 由 C3 夹具的 `db.session_target.columns`（PRAGMA table_info）
逐项录制；`CHECK(status IN …)` 的原文来自既有 C2 fresh-install 夹具
`fixtures/c2/fresh-install-ledger.json` 的 `expect.firstStart.sqliteMaster.entries`
中 `tbl_name:"session_target"` 的 `CREATE TABLE` SQL。

一条完整观察行（`goal-show-set-complete.json`，规范化后）：

```json
{
  "session_id": "<id>",
  "target_id": "target-1",
  "objective": "Ship the C3 goal contract",
  "status": "complete",
  "token_budget": null,
  "tokens_used": 22,
  "time_used_seconds": "<time>",
  "time_created": "<time>",
  "time_updated": "<time>",
  "summary_title": "Ship the C3 goal contract",
  "active_input_id": null,
  "active_run_started_at": null,
  "active_run_last_seen_at": null
}
```

- 只有 `active` / `paused` / `complete` 被观察到；`budget_limited` **未观察到**。
- `clear` 删除整行（`rowCount:0`），不保留历史目标行；`goalVerifications` /
  `goalVerificationTimeline` 仍在 runtime/快照中保留。
- `summary_title` 在首次 `set` 后由 `summary_updated` 写成与 objective 相同的字符串；
  未测试超长/多语言摘要的截断规则（见 §6）。
- `tokens_used` 只累计成功完成的 goal turn：单请求文本 turn 22；控制 turn 0；
  pause/stop 取消的 turn 为 0；2 个模型请求的 turn 52（见 §3.3 的 74 = 52 + 22）。
- `time_used_seconds` 是墙钟累计（包含进程/模型启动开销），**不与 turn 数线性对应**；
  夹具折叠为 `<time>`，实现不应拿它做精确等值断言。

### 1.4 `session/read` / 快照的 target 投影

- `session/read` 与 `session/goal` 的 snapshot 中，`session.target` 与
  `projection.target` **同时存在且逐字段相同**；无目标时显式为 `null`（不是缺键）。
- 观察到 target 对象的键：`sessionId,targetId,objective,summaryTitle,status,tokenBudget,
tokensUsed,timeUsedSeconds,activeInputId,activeRunStartedAtMs,activeRunLastSeenAtMs,
createdAt,updatedAt`。
- `runtime.goalVerifications` 是最近验证结果的窗口（loop 观察 `length:2`）；
  `runtime.goalVerificationTimeline` 是完整历史（loop 观察 40 条，`goalIteration` 1..40）。
  首条 timeline 的形状（`goal-continuation-loop.json`）：

```json
{
  "version": 1,
  "kind": "synthetic",
  "type": "goal_verification",
  "display": "separator",
  "targetId": "target-1",
  "verificationId": "<id>",
  "status": "completed",
  "goalIteration": 1,
  "anchorAssistantMessageId": "<message-id>",
  "anchorTurnId": "<turn-id>",
  "verification": { "passed": false, "reason": "not yet", "nextAction": "continue" },
  "startedAt": "<time>",
  "updatedAt": "<time>"
}
```

- `session_entry` 同时写 `type:"target_completion_verification"` 行，`data.payload` 是同一
  verification payload（started/completed/cancelled 各一行），`sequenceNumber` 指向事件 seq。
- `session/list` 行不带 `target`（C2 契约已观察），目标只出现在 read/snapshot。

### 1.5 completion verifier

每个 goal-continuation turn 结束后，运行时发起一次**非流式、无 tools** 的模型请求：

- `querySource:"target_completion_verification"`，`modelCall.operation` 以
  `goal_completion…` 开头，`transport:"http"`、`stream` 缺省（不是 SSE）。
- 请求的 `system` 3 段；messages 末尾是固定 user 提示：
  `Verify whether the active session goal is actually complete. … Return only a JSON object
with this exact shape: {"passed": boolean, "reason": string, "nextAction": string} …`
  逐字全文在 `goal-show-set-complete.json` 的 `expect.verifierRequest.messages`。
- 观察到的结果处理：
  - 返回合法 JSON 且 `passed:true` → verification `status:"completed"`，target
    `status:"complete"`（fixture 用 `{"passed":true,"reason":"goal verified","nextAction":null}`）。
  - 返回合法 JSON 且 `passed:false` → verification completed 但 target 保持 `active`，
    下一轮 goal-continuation 继续；`goalIteration` 递增（1,2,3,…）。
  - **返回非法 JSON**（mock provider 默认文本 `oracle-mock`）→ `goal-verifier-invalid-json.json`
    记录到 target `status:"complete"`、`runtime.goalVerifications`/timeline 的
    `{"passed":true,"reason":"The completion verifier did not return valid JSON."}`。
    这条 reason 是 oracle 事件里的录制值，不是手写推断；该 fallback 是线上行为，不是实现建议。
  - verifier 请求被取消（goal pause 时）→ timeline 末项 `status:"cancelled"`，
    `verification.reason:"Completion verifier request was cancelled."`（`goal-continuation-loop.json`
    的 `read.timelineLast`）。
- `goalVerifications` 的单项形状：`{passed, reason, nextAction}`。

### 1.6 停止条件

| 条件                       | 观察行为                                                                                                                                                                                    | 夹具                                                  |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------- |
| `pause`（provider 请求前） | `status_updated(command, active→paused)` 立即发出；continuation 在 provider 请求前 `turn.completed(cancelled)`；`run_finished` 清 active；`turn_usage.model_request_count:0`；不再开新 turn | `goal-pause-resume-clear.json`、`goal-user-stop.json` |
| `pause`（请求 in-flight）  | `model_request_started` 之后 pause：`model_request_failed` + `turn.completed(cancelled)`；`turn_usage.model_request_count:1`、`model_usage.status:"cancelled"`；tokens 0                    | `goal-pause-in-flight.json`                           |
| `clear`                    | `session.updated action:"cleared" source:"command"`，`target:null`，删除 `session_target` 行；只在 idle 时接受；运行中 `-32010`                                                             | `goal-pause-resume-clear.json`                        |
| 目标完成                   | verifier `passed:true` → `status_updated(runtime, active→complete)`；不再续跑                                                                                                               | `goal-show-set-complete.json`                         |
| 用户中断                   | `session/stop` + `{sessionId}` 应答 `{}`；turn `cancelled`；target `paused`；stop 在模型请求前到达时 `turn_usage.model_request_count:0`                                                     | `goal-user-stop.json`                                 |
| 预算                       | **不可触发**：协议无 `tokenBudget` 字段（传了 `-32602`），无 budget_limited 观察                                                                                                            | —                                                     |
| 轮次上限                   | 只观察到「40 个 continuation turn 后仍在继续」；上限是否 >40、具体值未知（loop 探针主动 pause，不是运行时自己停）                                                                           | `goal-continuation-loop.json`                         |
| 最大迭代/超时              | 未观察到                                                                                                                                                                                    | —                                                     |

`pause` 的 response snapshot 里 target 可能已是 `paused`，但 `sessionStatus:"running"`、
`activeInputId` 仍在——这是**取消中**的 snapshot；随后的 `read` 看到 active 字段已清空。
实现应按事件终态（`model_request_failed`（如有）/ `run_finished` /
`turn.completed(cancelled)`）收口，不能把 response snapshot 当作最终状态。

### 1.7 `expectedRevision`

冲突应答（driver 解析出的 error 帧）：

```json
{
  "code": -32009,
  "data": { "actualRevision": 5, "expectedRevision": 4 },
  "message": "Session state revision mismatch"
}
```

- `actualRevision` 等于 `session/read` 的 `runtime.stateRevision`。
- 冲突时**零副作用**：同窗口内 `newEvents:0`、`stateRevision` 不变、`session_target`
  rowCount/objective 不变（`goal-replace-revision-conflict.json` 的 `conflict.sideEffects`）。
- 传正确 revision 的 `replace` 被接受（夹具中 `expectedRevision:5` 成功并完成新目标）。

### 1.8 重启 / 冷恢复

`session/resume` 的应答键（比 create/read 多一个 `goalStats`）：

```text
messages, projection, protocol, runtime, session, settings, slashCommands,
goalStats, todos, todoGroups
```

`goalStats` 键：`timeUsedSeconds,tokensUsed,tokenBudget,contextUsed,contextWindow,
toolCallCount,iterationCount`。冷恢复后 `contextUsed:0`、`iterationCount:1`（观察到）。

两种冷恢复观察：

| 场景                               | 观察值                                                                                                                                                                                                                                      |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| paused 目标冷重启                  | target 仍 `paused`；`session/read`：`eventSeq:2`、`stateRevision:0`、`turnCount:0`、target active 字段为 null；`session/subscribe(includeSnapshot:true)`：`eventSeq:2`、`events:[]`；重启后无自动 turn；`show` 返回 `Goal paused…`          |
| active 目标在 turn 运行中杀进程    | resume 后 target 自动变 `paused`（activeInput/activeRun 清空），`eventSeq:2`、`stateRevision:0`、`turnCount:0`；无自动 turn，也不重放被中断的 turn                                                                                          |
| paused 后显式 `resume`             | `startedTurn:true`；新 goal-continuation turn；verifier 通过后 target `complete`；`read` 的 `turnCount:1`、`eventSeq:19`、`stateRevision:2`、timeline 一条 completed                                                                        |
| completed（tokensUsed 非 0）冷重启 | target 仍 `complete`、`tokensUsed:22` 前后一致；`timeUsedSeconds` 夹具折叠为 `<time>` 并记录 preserved:true；`show` 返回 `Goal complete…`；`eventSeq:2`、`stateRevision:0`、`turnCount:0`；无自动 turn（`goal-cold-restart-complete.json`） |

- 冷恢复后的目标状态来自 DB（`session_target`），不重放已消费 turn。
- 冷恢复会清空 runtime 的事件/投影计数（`eventSeq` 归到 2、`stateRevision:0`、
  `projection.turnCount:0`），但 target 的 `timeUsedSeconds/tokensUsed` 保留。

### 1.9 参数校验与错误形状

| 输入                                        | 应答                                                                                                      |
| ------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| `action:"bogus"`                            | `-32602` ZodError：`Invalid option: expected one of "show"\|"set"\|"replace"\|"pause"\|"resume"\|"clear"` |
| `tokenBudget:100`（未知键）                 | `-32602` ZodError：`Unrecognized key: "tokenBudget"`                                                      |
| `set` 缺 `objective`                        | 成功应答 `{response:"Usage: /goal <objective>", startedTurn:false}`，零状态变化                           |
| `replace` 缺 `objective`                    | 成功应答 `{response:"Usage: /goal replace <objective>", startedTurn:false}`，零状态变化                   |
| `show` 在 prompt 运行中                     | `-32010 Cannot manage goals while a prompt is running`                                                    |
| `set/replace/resume/clear` 在 prompt 运行中 | 都是 `-32010 Cannot manage goals while a prompt is running`（`pause` 例外）                               |
| `expectedRevision` 不匹配                   | `-32009`（见 §1.7）                                                                                       |

### 1.10 goal 未观察到

- `status:"budget_limited"` / `token_budget` 非 null / 预算耗尽停止；
- 自动续跑的轮次上限、最大 wall-clock、迭代上限（40 轮内未出现）；
- `summary_title` 的生成/截断/多语言规则（只观察 equals objective）；
- `show` 在 active-idle 时的独立文案（`set` 的 `Goal active…` 是同一构造，但没有单独 `show`）；
- `resume` 在 active/运行中、`clear` 在 active-idle、`replace` 在 active-idle 的行为
  （只观察到 paused/idle/complete 组合；运行中的管理动作统一 `-32010`，`pause` 例外）；
- target 完成后的冷重启；
- 多 workspace / 远程 identity 下的 goal 行为。

## 1.11 Rust C3 现状（T1，2026-11-18）

本节登记 Rust `agent-server` / `agent-core` 在 C3 T1 的实现取舍与**已知差异**；实现以
§1 的黑盒观察为基线，夹具与实际行为冲突时以夹具为准。测试入口
`crates/agent-server/tests/goal.rs`（25 个用例）与 `crates/session-store/tests/target.rs`。

已实现（与观察一致）：

- `session/goal` 六种 action、严格参数（未知键/非法 action `-32602`）；
  `{response, snapshot, startedTurn}` 三键恒在；
- 运行中限制：show/set/replace/resume/clear → `-32010`，`pause` 例外；
- `expectedRevision` 冲突 `-32009` + `{actualRevision, expectedRevision}`，零事件/零 DB 写；
- `session_target` 单行事实源（`set`/`replace` 覆盖并产生新 target id，`clear` 删行）；
  `session/read` 与快照的 `session.target` / `projection.target` 同源；
- 续跑：`set`/`replace` 先控制 turn（`executionKind:"controlOnly"`）再
  goal-continuation；`resume` 直接续跑；`run_started`/`run_finished` 包住每个目标
  turn，tokens/time 只在 `run_finished` 累计（取消不累计 tokens）；
- 停止条件：pause、clear、complete、user stop（新增 `session/stop`）；**无轮次/预算上限**；
- completion verifier：非流式、0 tools、`query_source:"target_completion_verification"`；
  合法 JSON 按值使用，非法 JSON 线上 fallback `passed:true` +
  `The completion verifier did not return valid JSON.`；
- 冷恢复：`active` → `paused` 并清空 active 字段、已入库 tokens/time/验证时间线保留、
  **不自动续跑**；`session/resume` 带 `goalStats`；
- 方法层的 mutating action 同步产出控制 turn/状态事件后才构造应答快照：`set` 快照
  `turnCount:1`/`currentTurnId`/`eventSeq` 是控制 turn 之后，pause/resume/clear 同理；
  revision 增量为首次 set/replace `+3`、替换已有目标 `+1`、pause `+1`、resume
  `+1`、clear `+1`（取消续跑终态再 `+1`）；
- 事件 seq 由 `agent_core::EventPublisher` 单点分配：`Session::emit` 分配、
  EventSink 落库后提交，方法层 goal 事件在同一发布器上分配/提交；提交按 seq 连续
  推进（缺号等待），日志/广播严格单调；
- 续跑 turn 编号用控制 turn 基数（同一目标 resume 后仍是 1），
  `foregroundExecutionId` 用会话级 continuation ordinal（每次尝试 +1），
  verifier 的 `goalIteration` 只对到达 verifier 的尝试 +1。

差异 / 未覆盖（不伪造观察值）：

| 项                        | Rust 现状                                                                                                                                                                                                                                                                                                                                                                                                                                                            | 依据                                                 |
| ------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------- |
| 提示词正文                | continuation / verifier 提示词**从头自撰**：只保留接口结构 `<system-reminder>`/`<untrusted_objective>`/`Budget:` 事实行与 verifier 的 JSON 契约；审计步骤、完成声明、证据规则等行文均为本仓库措辞。雷同自检：与夹具提示词的最长公共子串只命中 `<untrusted_objective>` 边界与 JSON 契约（59 字符），无 8 词连续重叠。                                                                                                                                                 | clean-room；task-1-brief 要求 prompt 登记为 authored |
| `session.updated` 事件族  | 只产出 goal 相关 action（`summary_updated`/`set`/`run_started`/`run_finished`/`status_updated`/`cleared`）与 verifier started/completed/failed_closed/cancelled；C1 未实现的 `messageCount`/`model_request_started`/`model_request_completed`/`model_request_failed`/`content` 仍不产出。因此 in-flight pause 缺夹具的 `model_request_failed {reason:"cancelled", errorCode:"model_request_cancelled"}`，取消证据由 `turn_usage`/`model_usage` 的 cancelled 行承担。 | C1 既有差异                                          |
| 控制 turn 的消息行        | 不写 oracle 的 assistant timeline 行；objective 写为 `user_prompt` 消息。goal 状态变更写 `system_reminder` + `source:"goal_state_change"`（正文 `Goal paused/resumed/cleared` 为自撰）。continuation prompt 写 `system_reminder` + `source:"goal-continuation"`。                                                                                                                                                                                                    | messageSummary 只观察到 role/kind/source             |
| verifier 请求形状         | system 只有 1 段（C1 的请求面），不是 oracle 的 3 段；tools 0、非流式消费、deltas 不进事件流。历史里带 continuation 的 system-reminder prompt，oracle 还注入 currentDate/meta 提醒与 objective 文本段（夹具 verifierRequest）；本实现不伪造这些段。                                                                                                                                                                                                                  | 请求面差异 / 夹具 §1.5                               |
| `model_usage`             | 只有 goal-continuation（`main_turn`）与 verifier（`target_completion_verification`）写行；C1 普通 turn 仍不写 `model_usage`。暂停/取消的续跑行 `status:"cancelled"`、tokens 0。                                                                                                                                                                                                                                                                                      | 夹具 §1.3/§1.6                                       |
| pre-request pause 计数    | `model_request_count` 在**观察到请求真的开始**（首个 delta/provider 应答）时才 +1；因此 provider 尚未产出任何内容的暂停是 0，in-flight（已产出 delta）是 1。真实网络已发出但尚无 delta 的极窄窗口也会记 0（夹具只覆盖两个端点形状）。                                                                                                                                                                                                                                | §1.6 两个形状                                        |
| `time_used_seconds`       | 墙钟秒向上取整，任何发生过 run 的段至少 1 秒；夹具折叠 `<time>`，测试只断言非负/保留/非零。                                                                                                                                                                                                                                                                                                                                                                          | §1.3                                                 |
| verifier 取消/错误        | pause/stop 取消 verifier → 事件与 timeline 都是 `cancelled` + `Completion verifier request was cancelled.`；provider 错误 → 事件与 timeline 一致地记 `failed_closed`（未观察组合）。                                                                                                                                                                                                                                                                                 | §1.5 只观察 pause 取消                               |
| valid JSON 缺 `passed`    | 与非法 JSON 同走 fallback（`passed:true` + 同一 reason）。                                                                                                                                                                                                                                                                                                                                                                                                           | 未观察组合                                           |
| `goalVerifications` 窗口  | 只保留最近 2 条 **completed** 验证；`cancelled`/`failed_closed` 不进窗口，但保留在 timeline。                                                                                                                                                                                                                                                                                                                                                                        | loop 观察 `length:2`                                 |
| `goalStats.toolCallCount` | 恒 0（未观察到独立的目标工具计数事实）。                                                                                                                                                                                                                                                                                                                                                                                                                             | §1.8                                                 |
| 目标 id                   | `target_<毫秒>_<计数>`；夹具别名 `target-1`/`target-2` 只用于可读性。`set`/`replace` 覆盖是**替换目标**语义：新 id/objective/time_created 同时覆盖，`time_created` 不保留旧值。                                                                                                                                                                                                                                                                                      | 值折叠 + §1.2                                        |
| 失败路径                  | provider 错误：发 `turn.failed`、清 active 字段并**同步 upsert 到 `session_target`**、停止续跑（target 仍 active）；配置失败（无模型/解析失败）：target 暂停、不发 turn、不伪造事件。                                                                                                                                                                                                                                                                                | §1.10 未观察                                         |
| `runtime.eventSeq` 语义   | 快照/`session/event` 的 `eventSeq` 是 `EventPublisher` 的**已分配高水位**（含已分配但尚未提交的 pause/stop 事件），而 replay 日志/live 只含**已提交**事件。把 read/resume 的 eventSeq 直接当 `afterSeq` 的消费者可能跳过随后才提交的同号事件（T2 重连/快照交接需按「committed cursor」或等待提交后再取游标）。                                                                                                                                                       | Round-3 EventPublisher 设计                          |
| 冷恢复 `eventSeq`         | resume 后 `eventSeq:0` 或 `1`（paused 目标会同步发一条 status_updated；C1/C2 已登记的物化差异；oracle 是 2）。`stateRevision:0`、`turnCount:0` 与观察一致。                                                                                                                                                                                                                                                                                                          | C2 snapshot 测试同款登记                             |
| 冷读副作用                | 无 runtime 的 `session/read` 对 `active` 目标执行 active→paused 落库（与 resume 同一冷恢复语义）。oracle 只在 resume 后观察过该迁移。                                                                                                                                                                                                                                                                                                                                | §1.8                                                 |
| 预算                      | 协议无 `tokenBudget` 字段，不产生 `budget_limited`。                                                                                                                                                                                                                                                                                                                                                                                                                 | §1.9                                                 |
| 轮次上限                  | **不实现**任何自动轮次/时间上限（只在 40 轮内观察到仍在继续）。                                                                                                                                                                                                                                                                                                                                                                                                      | §1.6                                                 |

## 2. Stream recovery 与重试

### 2.1 既有 C1/C2 夹具里的 `streamRecovery.updated`

`fixtures/c3/stream-recovery-c1-c2-shapes.json` 从 38 个 C1 根夹具机器提取（C2 夹具没有
`streamRecovery.updated`），共 **10** 个去重形状。键集合完全一致：

```json
{
  "anchorId": "<id>",
  "attemptId": "<id>",
  "kind": "tool_result | tool_error",
  "assistantMessageId": "<id>",
  "toolCallId": "<id>",
  "toolName": "Bash | Edit | Glob | Grep | Read | Write",
  "resultPartId": "<id>",
  "committedToolCallIds": "<ids>",
  "committedAt": "<time>"
}
```

`kind` × `toolName` 的组合：`tool_result`（Bash/Edit/Read/Write）与
`tool_error`（Bash/Edit/Glob/Grep/Read/Write）。这是工具结果提交锚点；`committedAt`
在规范化前是 ISO 时间字符串。C1 录制把 `committedToolCallIds` 折叠成 `<ids>` 字符串；
C3 注入流中断的四个新形状（§2.2）保留数组形态（`committedToolCallIds: []`）。

### 2.2 流中断（已提交 delta）→ stream recovery

注入：第一次请求写出一个 `text_delta` 后断开（`afterDeltas:1`），第二次同输入返回完整文本。
观察（`retry-stream-abort.json`）：

- 事件顺序：`turn.started` → `session.updated(model_request_started)` →
  `model.streaming(text_delta:"partial text ")` → `streamRecovery.updated` ×4 →
  `model.streaming(text_delta:"complete answer after recovery")` → `turn.completed(success)`。
- 四个 `streamRecovery.updated` payload（逐字字段）：

```json
{ "attemptId": "<id>", "assistantMessageId": "<id>",
  "failureKind": "provider_network_error",
  "message": "Network connection failed for the provider request.",
  "retryNumber": 1, "maxRetries": 10, "failedRequestId": "<id>",
  "_meta": { "mode": { "apiRetry": { "kind": "api_retry", "attempt": 1,
    "maxRetries": 10, "retryDelayMs": 0, "errorStatus": null,
    "error": "Network connection failed for the provider request." } } } }
{ "attemptId": "<id>", "anchorId": "<id>", "reason": "no_tool_committed",
  "committedToolCallIds": [] }
{ "attemptId": "<id>", "anchorId": "<id>", "assistantMessageId": "<id>",
  "discardedReasoningBytes": 0, "discardedTextBytes": 13,
  "discardedToolCallIds": [] }
{ "attemptId": "<id>", "anchorId": "<id>", "retryNumber": 1, "maxRetries": 10,
  "streamMode": "sse", "failedRequestId": "<id>",
  "_meta": { "mode": { "apiRetry": { "kind": "api_retry", "attempt": 1,
    "maxRetries": 10, "retryDelayMs": 0, "errorStatus": null,
    "error": "Model stream recovery retry started" } } } }
```

- `retryDelayMs` 是 **0**（立即重试），不是 HTTP 重试的 jitter 退避。
- 重试请求的 network `attempt` 仍是 **1**（不是 2）；`model_request_started` 重新发一次。
- `mode-request-failed` 的 debug 记录是 `retryable:false`、`reason:"network_error"`、
  `message:"Network connection failed for the provider request."`；流恢复与 adapter
  重试是不同路径，不能用 `retryable` 判断是否进入 stream recovery。
- 已提交 delta **不会进入最终消息**，但会出现在事件流：
  `model.streaming.text_delta` 依次为 `["partial text ", "complete answer after recovery"]`；
  `session/messages` 里有一条 `finish:"stream_recovery_discarded"`、text 为空的 assistant
  消息，随后是正常 `finish:"stop"` 的完整回复。
- 账务分层（T2 必须区分）：
  - `turn.completed.usage.modelRequestCount:1`（事件层只计成功完成的模型请求）；
  - `turn_usage.model_request_count:2`（DB 层计尝试数）、`model_retry_count:0`；
  - `model_usage` 两行，`attempt_index` 都是 0，第一行 `status:"error"`、
    `error_type:"network_error"`，第二行 `status:"completed"`。
- `turn.completed.historyRoundCount:2`（普通单请求 turn 是 1）。

### 2.3 HTTP 5xx：adapter 重试

注入：第一次 503，第二次成功（`retry-5xx-then-success.json`）：

- 事件顺序：`model_request_started(attempt:1)` → `model_request_failed` →
  `model_retry_scheduled` → `model_request_started(attempt:2)` → `model_request_completed`。
  **没有** `streamRecovery.updated`。
- `maxAttempts:11`（即 `maxRetries:10`），`statusCode:503`、`reason:"server_error"`、
  `message:"Provider returned a server error."`、`retryable:true`、
  `errorCode:"model_request_failed"`、`errorPhase:"stream"`、
  `exceptionType:"AI_APICallError"`、`streamOutputCommitted:false`。
- `model_retry_scheduled` 额外字段：`nextAttempt:2`、`delayMs`（夹具折叠 `<retry-delay>`）、
  `_meta.mode.apiRetry`：

```json
{
  "kind": "api_retry",
  "attempt": 1,
  "maxRetries": 10,
  "retryDelayMs": 1337,
  "errorStatus": 503,
  "error": "Provider returned a server error."
}
```

- 成功后 `model_request_completed._meta.mode.apiRetry:null`；`model_request_started`
  也带 `_meta.mode.apiRetry:null`（attempt 1 时）。
- 终态恰好一次：`turn.completed(success)`。
- 账务：`turn_usage.model_request_count:1`、`model_retry_count:1`；
  `model_usage` **只有一行** `attempt_index:0`、`retry_count:1`、`status:"completed"`、
  `error_type:"server_error"`、`retryable:1`、`finish_reason:"stop"`。
- 退避是带抖动的指数退避。本次运行示例（仅说明量级，夹具不写死）：第一次
  ≈1.3–1.5s、第二次 ≈2.0–2.6s、第三次 ≈7.5s；`maxAttempts` 为 11。

`retry-backoff-sequence.json` 记录连续 3 次 503 再成功：4 次 provider 请求、
3 次 `model_retry_scheduled`、`turn_usage.model_retry_count:3`，终态一次。夹具把每次
`delayMs` 折叠成 `<retry-delay>`。

### 2.4 401：非 retryable

注入第一次 401（`retry-401-non-retryable.json`）：

- 只有 1 次 `/v1/messages` 请求；没有 `model_retry_scheduled`。
- 事件：`model_request_started` → `model_request_failed(retryable:false,
statusCode:401, reason:"auth_failed", message:"Provider authentication failed.")` →
  `turn.failed` 一次。
- `turn.failed.payload`：

```json
{
  "error": {
    "type": "unknown_error",
    "attribution": {
      "source": "provider",
      "reason": "auth_failed",
      "errorPhase": "stream",
      "exceptionKind": "api_call",
      "providerId": "oracle-mock",
      "modelId": "mock-1",
      "providerKind": "anthropic",
      "transport": "sse",
      "statusCode": 401,
      "retryable": false
    },
    "code": "provider_not_configured",
    "message": "Provider authentication failed.",
    "detail": "Turn execution failed\nprovider=oracle-mock model=mock-1 request=<id> reason=auth_failed status=401 retryable=false\ninvalid api key",
    "underlyingErrorMessage": "invalid api key"
  },
  "turnPhase": "processing_input"
}
```

- 账务：`turn_usage.status:"error"`、`error_type:"unknown_error"`、
  `error_code:"UNKNOWN_ERROR"`、`model_request_count:1`、`model_retry_count:0`；
  `model_usage` 一行 `status:"error"`、`error_type:"auth_failed"`、`retry_count:0`。
- 没有 assistant 文本成功消息；会话里留下一个 `finish:null` 的 assistant 占位（DB
  消息摘要可见），无部分文本 part。

### 2.5 重试退避期间取消

注入第一次 503，在 `model_retry_scheduled` 到达后调用 `session/stop`
（`retry-cancel-during-backoff.json`）：

- `session/stop` 应答 `{}`；provider 始终只收到 **1** 次请求（没有第二次尝试）。
- 事件：`model_retry_scheduled` 后出现 `model_request_failed`：
  `{retryable:false, reason:"cancelled", message:"Model request was cancelled."}`；
  随后 `turn.completed(resultType:"cancelled")` 恰好一次。
- 账务：`turn_usage.status:"cancelled"`、`cancelled_by_user:1`、
  `model_retry_count:1`、`error_type:"turn_cancelled"`、`error_code:"TURN_CANCELLED"`；
  `model_usage.status:"cancelled"`、`retry_count:1`、`error_type:"cancelled"`。

### 2.6 重复 delta / 终态

- HTTP 重试：失败尝试没有输出，成功尝试只产生一份 delta；最终消息一份文本。
- 流中断：事件流会看到被丢弃的 partial delta，但最终消息只有恢复后的文本；
  `stream_recovery_discarded` 消息保留为历史行（text 空）。
- 所有注入场景 `turn.completed` / `turn.failed` 都恰好一次（夹具 `eventSummary.terminals`）。
- **事件层与 DB 层计数不同源**：`turn.completed.usage.modelRequestCount` 只计成功模型请求；
  `turn_usage.model_request_count` 计尝试数（stream abort 中事件 1 / DB 2），
  `model_usage` 是逐 attempt 行。T2 的“重试次数”断言必须明确选哪层，不能混用。
- `session/debug.networkEntries` 是观察 retry 的辅助读面；其
  `statusType` 取值：`model_request_started`、`model_request_completed`、
  `model_request_failed`、`model_retry_scheduled`、`model_stream_stalled`（后者未触发）。

### 2.7 Rust C3 现状（T2，2026-11-18）

T2 在 `agent-core::Session::run_turn` 内落重试/恢复循环，事件/账务按 §2 的夹具基线
实现；`provider-http` 只负责错误分类（HTTP 状态/传输/流中断），不内嵌重试。
测试入口：`crates/agent-core/tests/recovery.rs`（7 用例）与
`crates/agent-server/tests/retry.rs`（7 用例）。

已实现（与观察一致）：

- 错误分类：`ProviderError { message, kind, status, retryable, underlying_message }`；
  401/403 → auth 非 retryable；5xx → server retryable；传输/流中断 → retryable；
  非法 JSON/本地错误 → 非 retryable。`ProviderError::new` 保持旧的 message-only 语义
  （Unknown、非 retryable）。
- 事件/终态的规范文案：auth → `Provider authentication failed.`、5xx →
  `Provider returned a server error.`、网络/流中断 →
  `Network connection failed for the provider request.`；适配器的 `HTTP 503: …`
  原始文案只留在 `ProviderError.message` / `underlying_message`，不透传到事件面
  （`turn.failed.underlyingErrorMessage` 用后者）。
- HTTP 5xx/未提交 delta 的传输失败：指数退避（默认 base 1000ms、上限 30s、0–50%
  抖动；夹具把毫秒折叠为 `<retry-delay>`，不逐毫秒复刻）+ 最多 10 次重试；
  每次调度发 `session.updated(model_retry_scheduled)`，字段含 `attempt`/
  `maxAttempts:11`/`nextAttempt`/`delayMs`/`statusCode`/`reason`/规范 `message`/
  `errorCode`/`_meta.mode.apiRetry`；成功后发
  `session.updated(model_request_completed)`（`_meta.mode.apiRetry:null`）。
- 宿主补齐（`agent-server`）：重试事件补 `providerId`/`modelId`/`sessionId`/
  `turnId`/`traceId`/`querySource:"main_turn"`/同一 turn 共享的 `queryId`/
  `requestId`/`spanId`/`parentSpanId`/`modelRequestSessionType:"main"`/
  `timestamp`/`modelCall`（`agent_step` + `main` + reasoning + `stepIndex:0`）；
  recovery 收口事件的 `modelCall` 带 `callCause:"recovery"` 与
  `attributes.streamRecoveryNumber`，载荷带 `streamRecovery` 摘要
  （attemptId/anchorId/maxRetries/retryNumber/recoveredFromRequestId）。
- 流中断（已提交 delta）：4 条 `streamRecovery.updated` 与 §2.2 同序同键：
  `failureKind:"provider_network_error"`、`retryDelayMs:0`、`retryNumber` 1..、
  `maxRetries:10`、`reason:"no_tool_committed"`、`discardedTextBytes`（UTF-8 字节）、
  `discardedReasoningBytes:0`、`streamMode:"sse"`；已提交 delta 仍进事件流但不进
  最终消息；`turn.completed.historyRoundCount` 计 2。
- 401：只请求一次；`turn.failed` 恰好一次：`turnPhase:"processing_input"`、
  `error.type:"unknown_error"`、`code:"provider_not_configured"`、规范
  `error.message` + `underlyingErrorMessage`（原始 `invalid api key`）、
  `retryable` 只在 `attribution` 内、无顶层 `error.retryable`；会话留下
  `finish:null` 的空 assistant 占位（无文本 part）；`turn_usage` 记
  `error_type:"unknown_error"`/`error_code:"UNKNOWN_ERROR"`。
- 退避期间取消：不再发起下一次尝试；发
  `session.updated(model_request_failed {reason:"cancelled", retryable:false,
errorCode:"model_request_cancelled", _meta.apiRetry:null})`；
  `turn.completed(resultType:"cancelled")` 恰好一次且形状为
  `{response, tokenCount:0, toolCallCount, historyRoundCount:0, duration,
  resultType:"cancelled"}`（无 `usage`）；DB 用内部轮次事实写
  `turn_usage.model_request_count:1`、`model_retry_count:1`。
- 账务分层：事件 `turn.completed.usage.modelRequestCount` 只计成功完成的请求；
  DB `turn_usage.model_request_count` 与（成功 turn 的）`historyRoundCount` 计轮次：
  初始尝试 + `is_recovery_attempt` 的恢复尝试；HTTP adapter 重试（包括恢复周期里
  后续无 delta 的 adapter 尝试）不增轮次；`turn_usage.model_retry_count` 只计 HTTP
  adapter 重试（`model_retry_scheduled` 事件数，stream recovery 为 0）。
- apiRetry：agent-server 在提交 `session.updated`/`streamRecovery.updated` 时喂入
  `mode-api-retry::SessionApiRetryTracker`；`model.streaming` 的恢复进展（非空
  text/reasoning/tool delta）通过 crate 新增的
  `track_model_streaming_progress` 清除状态；成功/取消/错误路径清空为 `null`。
  快照用 crate 的 `with_api_retry_runtime` 投影。
- OpenAI Chat 适配器：连接在 `[DONE]`/`finish_reason` 前关闭 → `StreamAbort`
  （retryable），不再按已产出内容推断成功；有 `finish_reason` 的兼容端点仍成功。
- 重试耗尽：按夹具的 `maxRetries:10`（共 11 次尝试）停止，`turn.failed` 恰好一次；
  不另发第二个终态。

差异 / 未覆盖（不伪造观察值）：

| 项                       | Rust 现状                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | 依据                    |
| ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------- |
| 重试所在层               | HTTP 重试在 `agent-core` 的会话循环里做（brief 指定），不是 `provider-http` adapter 内部；因此 adapter 无法在每次请求前重发 `model_request_started`。                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | task-2-brief            |
| 重试事件字段             | 宿主补齐 `providerId/modelId/sessionId/turnId/traceId/querySource/queryId/requestId/spanId/parentSpanId/modelRequestSessionType/timestamp/modelCall`；**未接线**（不伪造）`providerKind`/`transport`/`baseURL`/`requestHeaderCount`/`responseHeaderCount`/`requestHeaders`/`responseHeaders`，以及 timing（`timeToFirstProviderEventMs`/`timeToFirstContentMs`/`timeToFirstTextMs`/`streamStallCount`/`durationMs`）与失败事件的 `responseHeader*`/`streamOutputCommitted`/`exceptionType`。`queryId` 同一 turn 共享，`requestId` 逐事件生成（夹具折叠为 `<id>`）；`modelCall.stepIndex` 恒 0（重试夹具均单步；多步 turn 未观察）；`querySource` 恒 `main_turn`（goal 续跑的重试未观察）。 | §2.3/§2.5               |
| 规范文案                 | 事件/`turn.failed.message` 用分类规范文案（auth/server/network/rate-limit）；适配器原文只进 `ProviderError.message` 与 `turn.failed.underlyingErrorMessage`。未观察分类（HttpStatus/Api/Malformed/Local）仍用原始 message。                                                                                                                                                                                                                                                                                                                                                                                                                                                                | §2.3/§2.4               |
| `model_request_*` 事件族 | 正常 turn 仍不产出（C1 登记缺省）。重试 turn 补发 `model_retry_scheduled` / `model_request_completed` / 取消收口的 `model_request_failed`；**每次尝试**的 `model_request_started` 与每次失败的 `model_request_failed` 未产出（取消收口事件不属于这两类）。                                                                                                                                                                                                                                                                                                                                                                                                                                 | C1 既有差异 + §2.2/§2.3 |
| 退避毫秒                 | 指数 + 抖动只保证量级/单调；夹具把 `delayMs` 折叠为 `<retry-delay>`，未逐毫秒对拍。默认 base 1000ms、上限 30s。                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            | §2.3                    |
| 429/408/403/400          | 只观察过 401/503。Rust 规则：403 auth、408 transport（retryable）、429 rate limited（retryable）、其余 4xx 非 retryable；这些分支没有夹具。                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                | §2.8 未观察到           |
| stream recovery 取消     | 未观察；Rust 在尝试边界/退避 select 响应取消，终态 cancelled 一次；恢复期间取消也补 `model_request_failed{cancelled}` 清空 apiRetry。                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | §2.8 未观察到           |
| 带工具调用的流恢复       | 未观察；已提交工具调用时发 `reason:"tool_committed"` + `committedToolCallIds`，丢弃事件清空当前步。夹具只覆盖 `no_tool_committed`。                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        | §2.8 未观察到           |
| recovery failureKind     | 只有 `provider_network_error` 是观察值；已提交 delta 的 5xx / 429 会分别发 `provider_server_error` / `provider_rate_limited`（自撰稳定串，无夹具）。                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       | §2.8 未观察到           |
| 重试耗尽形状             | 未观察；Rust 只保证 11 次尝试后 `turn.failed` 一次。DB 记 `model_request_count:1`（HTTP 重试不增轮次）、`model_retry_count:10`。                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           | §2.8 未观察到           |
| 恢复后的重试再失败       | 恢复尝试若未产出任何 delta 又失败，Rust 按「无已提交 delta」走 adapter 退避；该 adapter 尝试不增轮次（`is_recovery_attempt=false`），成功收口事件不带 `streamRecovery`/`callCause:"recovery"`。夹具未覆盖连续中断。                                                                                                                                                                                                                                                                                                                                                                                                                                                                        | §2.8 未观察到           |
| `model_usage` 行         | C1 普通 turn 仍不写 `model_usage`（§1.11 既有差异），因此夹具的「stream abort 两行（error + completed）/5xx 一行」没有逐行复刻；重试账务只在 `turn_usage` 层可断言。                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       | §1.11 + §2.2/§2.3       |
| `turn.failed` 归属       | 401 形状与夹具一致（`turnPhase:"processing_input"`、规范 message、underlyingErrorMessage、无顶层 retryable、finish:null 占位）；`providerId`/`modelId`/`providerKind`/`transport`/`exceptionKind` 仍未填（agent-core 不持有这些事实），`detail` 的 provider/model/request 段为自撰/省略。                                                                                                                                                                                                                                                                                                                                                                                                  | §2.4                    |
| `turnPhase`              | 只有首次请求的 401 夹具观察到 `processing_input`。Rust 对 `run_turn` 内所有 provider 失败沿用该值（含工具批次之后的后续请求）；「首次请求 vs 后续步」的阶段化命名**未观察**，不发明新枚举，登记为泛化差异。 | §2.4 / §2.8 |
| compact / verifier 请求  | 不在 `run_turn` 内，未接重试（未观察到这两类请求的失败重试）。                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | §2.8 未观察到           |
| 取消 turn.completed 形状 | 与夹具一致：无 `usage`、`historyRoundCount:0`、`tokenCount:0`；DB `model_request_count` 改走内部轮次事实通道（夹具取消示例为 1），不再从事件 `historyRoundCount` 取。                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | §2.5                    |

### 2.8 stream recovery / retry 未观察到

- 重试耗尽（`maxRetries:10` 全用完）的 `turn.failed` 形状与耗时（会跨多轮长退避，本次未驱动）；
- 403 / 400 / 408 / 429 的分类（只验证 401 与 503）；
- `model_stream_stalled` / idle timeout 触发的恢复；
- 工具调用（`tool_input_*` / `tool_call`）中断后的 stream recovery；
- stream recovery 重试期间取消；
- `web-remote-replayable` 的 gap 恢复（归 Host，不在 stdio 面）；
- provider 返回 `retry-after` 头时的退避覆盖。

## 3. 后台通知

### 3.1 空闲完成

`background-idle.json`：后台 Bash `sleep 3; printf 'idle-bg-done\n'` 在主 turn 结束后完成。

- 会话空闲时自动开一个新 turn：`turn.started` 的 `inputSource:"background_task"`，
  `input` 是 `<task-notification>`：

```text
<task-notification>
<task-id>exec_<uuid></task-id>
<tool-use-id><toolCallId></tool-use-id>
<output-file><dataRoot>/.mode/cli/exec/<sessionId>/<toolCallId>-stdout.log</output-file>
<status>completed</status>
<summary>Background command "idle background marker" completed (exit code 0)</summary>
</task-notification>
```

- notification turn 以 `turn.completed(resultType:"success")` 结束；事件流不关闭。
- DB：notification turn 的 user message `semantics.kind:"background_notification"`、
  `source:"background_task"`（`messageSummary` 观察）。
- 后台任务本体的事件：`session.updated` 带
  `{taskId,toolCallId,toolName:"Bash",taskKind:"bash",cancellable,command,description,
status:"running|completed",outputPath,pid,startedAt,completedAt,...}`。
  任务完成事件先于 notification turn。

### 3.2 忙时完成（模型流式等待期间）：合并为一个 notification turn

`background-busy-coalesced.json`：两个后台任务（`sleep 3`，描述 `busy one` / `busy two`）
在另一个 turn 的 **模型流式等待窗口**（`model_request_started` 之后、`turn.completed`
之前，模型响应延迟 9s）内完成。

- busy 期间**没有**立即开 notification turn。
- busy turn 结束后只开 **一个** `background_task` turn，`input` 同时包含两条
  `<task-notification>`，按完成顺序、以空行分隔。两条 summary 都保留（本夹具
  `droppedCompletedTasks:[]`）。
- 结论只覆盖这个窗口：忙时（模型流式等待）完成的通知是「合并延后」。
  **不能推广到 C1 的另一种时序**：C1 的「忙时丢弃」观察发生在**前台工具执行**
  （foreground tool execution）期间，C3 没有重录该组合，见 §5。
- C3 T3 若只实现本夹具，应按「待处理集合 + 空闲后一个 notification turn 合并投递」；
  工具执行中的忙时行为需等 §5 的补测。
- 未观察到通知在 busy turn 结束前被消费；也未观察到同一任务重复通知。

### 3.3 与 goal 续跑的交互

`background-goal-interaction.json`：goal-continuation turn 内启动后台 Bash，
任务在该 turn 的第二次模型请求期间完成。

观察顺序：

1. continuation turn 的 `run_started` / 模型请求 / 后台任务完成 / `run_finished` / `turn.completed`；
2. `turn.started(inputSource:"background_task", targetId:null)`；
3. notification turn 的 `run_started` / `run_finished`（target 仍 active）；
4. 之后才发起 verifier（`target_completion_verification`）；
5. verifier 通过 → `status_updated(runtime, complete)`。

- notification turn 计入 goal：最终 `target.tokensUsed:74`（continuation turn 52 +
  notification turn 22），`timeUsedSeconds` 同样包含它。
- 因此实现顺序应是「先处理后台通知 turn，再做 completion verification」；不能把
  notification turn 当成 goal 之外的旁路，也不能让它跳过 `run_started/run_finished`。

### 3.4 通知文本

| 场景            | 观察文本要点                                                                                                                                                                                                                                                                                                                                                   |
| --------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 后台 Bash 成功  | `<status>completed</status>`、`<summary>Background command "<description>" completed (exit code 0)</summary>`；`<description>` 来自工具入参。                                                                                                                                                                                                                  |
| 后台 Agent 成功 | `<task-id>agent_<uuid></task-id>`、`<output-file><dataRoot>/.mode/cli/agents/<sessionId>/agent_<uuid>/output.txt</output-file>`、`<summary>Agent general-purpose task "<description>" completed.</summary>`、`<result><子代理最终文本></result>`、`<usage><subagent_tokens>22</subagent_tokens><tool_uses>0</tool_uses><duration_ms>…</duration_ms></usage>`。 |

- 后台 Agent 的 notification turn `turn.completed` 带
  `backgroundSubagentResultConsumed:true`（foreground Agent 没有该字段）。
- 后台 Bash 的 notification turn 未观察到 `backgroundSubagentResultConsumed` 键。
- 失败退出码 / killed 的 Bash 通知文本 C1 已观察（`failed with exit code 7`、
  `was stopped`），C3 只重录成功路径；见 `tauri-agent-runtime.md §3.3`。

### 3.5 Rust C3 现状（T3，2026-11-18）

T3 在 `agent-server` 的 per-session worker 上接线后台通知，行为基线是 §3.1–§3.3 的
三个夹具；测试入口 `crates/agent-server/tests/background.rs`。

已实现（与观察一致）：

- **空闲完成 → 一个通知 turn**：`TaskRegistry` 在任务终态时投递通知；worker 空闲时
  由注册表完成信号唤醒（仅唤醒，不新增队列/定时器），取走当前全部待处理通知后
  开**一个** `background_task` turn。`turn.started` 带 `inputSource:"background_task"`、
  `inputVisibility:"model-only"`、`inputId`、`foregroundExecutionId`（与普通输入同一
  `runtime_command_<n>` 计数）、`originMeta:{backgroundSource:"bash",title,workId}`、
  `backgroundSource:"bash"`；`input` 是 `<task-notification>`，多条之间用空行分隔
  （§3.2 的合并形状）。DB user 行 `semantics.kind:"background_notification"`、
  `source:"background_task"`。
- **忙时（模型流式等待窗口）完成 → 合并延后**：完成通知留在 `TaskRegistry` 的
  既有通道，当前 turn 结束后由 worker 一次取走并开一个通知 turn；不丢、不重复。
  `turn.started` 的 `turnNumber` 用普通 turn 序号（`goal` 交互见下）。
- **前台工具执行期间完成 → 丢弃**：worker 在工具 `started` 与 `batch` 之间标记
  「前台工具执行窗口」，该窗口内到达的通知不入队（C1 §3.5 的丢弃观察；C3 未重录
  该组合，见 §3.6）。**例外**：`TaskStop` 自己产生的 killed 通知走控制投递通道，
  跳过该判据（否则停止任务后永远收不到 C1 观察到的 `<status>killed</status>`
  notification turn）。
- **执行序号**：`foregroundExecutionId` 的数字部分由显式执行序号给出，不再从
  `turnNumber` 推导。非 goal 通知 turn 用「已完成 turn 数 + 1」；goal 路径里
  continuation 用续跑序号，通知 turn 递增同一个 goal 执行计数器（`rc(N+1)`），
  completion verifier 回指通知 turn（无通知时回指续跑 turn），下一轮 continuation
  继续 `rc(N+2)`，保持单调（夹具 `background-goal-interaction.json` 的
  `runtime_command_2`）。
- **goal 续跑交互**：续跑 turn 结束、verifier 之前，先投递 pending 通知为一个
  `background_task` turn（`targetId:null`，但包在目标 `run_started`/`run_finished`
  里，tokens/time 计入目标）；verifier 仍在其后运行。
- **终态**：通知 turn 与普通 turn 同口径，`turn.completed` / `turn.failed` 恰好一次；
  取消不写半截 assistant 行；`apiRetry` 在任何 `turn.completed` / `turn.failed`
  提交时清空。
- **取消占位**：turn 取消且**已观察到模型请求**（`TurnError::progress.model_requests>0`
  同源的内部轮次事实）时，补一条 `finish:null`、无 part 的 assistant 占位行
  （§2.5、§1.6 的 `retry-cancel-during-backoff` / `goal-pause-in-flight` 形状）；
  请求前取消不写占位。占位在 `turn.completed(cancelled)` 发布前落库。

差异 / 未覆盖（不伪造观察值）：

| 项 | Rust 现状 | 依据 |
| --- | --- | --- |
| 任务状态事件 | 不产出任务 `session.updated {taskId,…,status:"running"/"completed"}`（C1 事件族缺省）；通知 turn 本身完整。 | §3.1 |
| 前台工具执行窗口的丢弃 | 用「`tool.updated started` 到 `batch`」近似 C1 的时序；C3 只录到模型流式窗口的合并，二者是否同一判据**未验证**（§5 已登记）。 | C1 §3.5 / §3.2 |
| `originMeta` | `backgroundSource` 恒 `"bash"`（当前只有 Bash 后台任务）；`title` 是多条描述的 `" · "` 连接；`workId` 取首批第一条 task id（夹具折叠为 `<id>`，无法区分具体取哪条）。 | §3.2 |
| 通知 turn 计数 | 非 goal 路径按普通输入计 `turnCount`，并补两次 revision（起点 + 终态），与 `background-idle`/`background-busy-coalesced` 的 `stateRevision` 一致；goal 路径的 revision 仍按 T1 规则，未逐值对齐 §3.3。 | §3.1/§3.2/§3.3 |
| 通知 turn 取消 | 未观察；实现与普通 turn 同口径（`session/stop` 取消 token、终态 cancelled 一次）。 | §3.6 |
| 通知 turn provider 失败 | 未观察；实现发唯一 `turn.failed`、清 active 目标字段并停止续跑（与 goal 续跑失败同口径）。 | §3.6 |
| verifier 期间完成 | 未观察；通知留在注册表，verifier/目标收口后由 worker 按空闲通知投递（可能产生 `targetId:null` 的通知 turn）。 | §3.6 |
| 3+ pending 合并上限 | 未观察；实现无上限、按完成顺序一次投递。 | §3.6 |

### 3.6 后台通知未观察到

- C3 时序下的失败/killed 通知（依赖 C1 观察）；
- 未提供 `description` 时的 summary 文案；
- 3 个以上任务同时 pending 的合并上限/顺序；
- 通知 turn 运行中再次完成任务的交互；
- 通知 turn 与 retry/stream recovery 的组合；
- notification turn 被 `session/stop` 取消时的目标/账务；
- 前台工具执行期间的忙时完成（C1 观察到丢弃；C3 只录了模型流式等待窗口的合并
  延后，二者未统一；Rust 用工具窗口近似丢弃，属待补测语义）。

## 4. Subagents 只读面（`session/subagents`）

### 4.1 边界与空数据

- 首 turn 之前（session 刚 create/subscribe，尚未产生 turn）调用
  `session/subagents` 返回 `-32004 Session not found: <sessionId>`——即使该会话
  在 `session/read` 中可见。完成至少一个 turn 后同一会话返回正常空数据。
- 不存在的 session：`-32004 Session not found: sess_missing`。
- 有 turn 后空数据（`endedLimit` 传 20 或省略相同）：

```json
{ "revision": 4, "childSessionIds": [], "running": [], "ended": { "total": 0, "items": [] } }
```

- `endedLimit` 的默认 20 来自协议 schema（`modeSessionSubagentsParamsSchema` 的
  `.default(20)` 镜像），不是本次行为观察；空数据下「省略」与「传 20」返回相同结果，
  无法用这个夹具区分默认值。`endedLimit:0` 与 `-1` 都是 `-32602`：
  `Too small: expected number to be >0`。
- 无更多页时 `ended.nextCursor` 键省略（不是 null）。

### 4.2 running / ended 形状

前台 `Agent`（`subagents-foreground.json`，`general-purpose`，延迟 3s 完成）：

```json
{
  "revision": 3,
  "childSessionIds": ["child-1"],
  "running": [
    {
      "childSessionId": "child-1",
      "agentId": "agent_<uuid>",
      "toolCallId": "toolu_mock_0_0",
      "subagentType": "general-purpose",
      "title": "child task",
      "startedAt": 1794965762749,
      "status": "running"
    }
  ],
  "ended": { "total": 0, "items": [] }
}
```

完成后：

```json
{
  "revision": 4,
  "childSessionIds": ["child-1"],
  "running": [],
  "ended": {
    "total": 1,
    "items": [
      {
        "childSessionId": "child-1",
        "agentId": "agent_<uuid>",
        "toolCallId": "toolu_mock_0_0",
        "subagentType": "general-purpose",
        "title": "child task",
        "startedAt": 1794965762749,
        "status": "success",
        "summary": "child result text",
        "endedAt": 1794965766843
      }
    ]
  }
}
```

- running 项没有 `summary` / `endedAt`；ended 项有 `summary`（子代理最终文本）与
  `endedAt`。
- `title` 来自 `Agent` 入参的 `description`（不是子会话标题）。
- `status` 只观察到 running / success；`waiting/blocked/failed/cancelled/lost` 未观察。
- `revision` 随子代理状态变化递增（3→4），可视为投影版本。

### 4.3 生命周期事件与工具结果

`Agent` 工具调用产生：

- `tool.updated scheduled`：`canRunParallel:true`、`parallelGroupIndex:0`；
- `tool.updated started`：`readOnly:true`、`sideEffectScope:"session"`；
- `session.updated` running：

```json
{
  "agentId": "agent_<uuid>",
  "agentType": "general-purpose",
  "childSessionId": "child-1",
  "description": "child task",
  "prompt": "child task prompt: inspect the workspace",
  "parentToolCallId": "toolCallId",
  "status": "running",
  "allowedTools": ["*"]
}
```

- `session.updated` completed：

```json
{
  "agentId": "agent_<uuid>",
  "agentType": "general-purpose",
  "childSessionId": "child-1",
  "parentToolCallId": "toolCallId",
  "status": "completed",
  "totalDurationMs": 3105,
  "totalToolUseCount": 0,
  "totalTokens": 22
}
```

- `tool.updated result` content 逐字模板：

```text
child result text
agentId: agent_<uuid> (use SendMessage with to: 'agent_<uuid>' to continue this agent)
<usage>subagent_tokens: 22
tool_uses: 0
duration_ms: 3105</usage>
```

结果字段：`budgetStrategy:"artifact"`、`truncated:false`、
`originalBytes/returnedBytes` 为正文 UTF-8 字节数（230 for the observed case）。

- `Explore` 子代理的 `allowedTools` 为具体列表（observed `Bash,Read,WebFetch,WebSearch,TodoWrite`），
  `general-purpose` 为 `["*"]`。

### 4.4 分页

`subagents-pagination.json`（两个**串行创建**的 Agent，`endedLimit:1`）：

- `ended.total:2`，items 按 `endedAt` 倒序（后完成的 child two 先返回）；
- `nextCursor` 是不透明 base64；解码后形状 `{childSessionId, endedAt}`；
- 用 `endedCursor` 请求第二页返回剩余 child one，且没有 `nextCursor`；
- `childSessionIds` 保持创建（tool call）顺序：`["child-1","child-2"]`；ended 第一页是后完成的 `child-2`，第二页是 `child-1`，与创建顺序无关；
- `endedLimit:1` 的响应仍带完整 `childSessionIds`（两个）。

### 4.5 DB 投影

- `session` 表出现父会话与子会话两行：
  - 子会话 `task_type:"subagent_child"`、`parent_id:"<父 sessionId>"`；
  - 子会话 `title` 是 prompt（不是 description），例如
    `child task prompt: inspect the workspace`；
  - `slug` 与 `id` 相同；`directory` 与父会话相同。
- **`session_task_link` 在 subagent 路径下为空**（前台/后台/分页三种观察都是 0 行）。
  子代理列表不是从该表投影；C3 T4 不要假设该表有 subagent 行。
- 子会话消息与父会话消息写在同一 `message` 表（`sequence` 是会话内序号，会重复）。
- notification turn 的消息语义：`kind:"background_notification"`、`source:"background_task"`。

### 4.6 子会话读面

- 子代理结束后直接 `session/read({sessionId: childSessionId})` 返回
  `-32004 Session is not active: child-1`。
  子会话读面要求该会话仍是活动 runtime；C3 的列表投影不依赖它。

### 4.7 subagents 未观察到

- `waiting` / `blocked` / `failed` / `cancelled` / `lost` 状态；
- 子代理失败/缺失字段时的 fail-soft 形状；
- `endedCursor` 非法/过期游标的错误；
- `running` 多项并发时的排序稳定性（并行 Agent 的 running/completed 事件到达顺序
  不稳定；夹具按 description 排序后记录）；
- 子代理取消（TaskStop / session/stop）后的列表项；
- `background_subagent_result_consumed` 在 `turn.failed` 上的取值（只观察到 success）。

## 5. 未观察到 / 不可触发汇总

| 项                                             | 状态                                                           |
| ---------------------------------------------- | -------------------------------------------------------------- |
| goal `budget_limited` / 预算耗尽               | 当前协议无 `tokenBudget` 字段；传未知键 `-32602`               |
| goal 轮次/迭代上限                             | 只验证 40 个 continuation turn 未停；上限是否 >40/具体值未知   |
| `summary_title` 生成/截断规则                  | 只观察 equals objective                                        |
| `show`(active-idle) 独立文案                   | 未单独捕获；`set` 的 `Goal active…` 是同一构造                 |
| 重试耗尽（10 次）                              | 未驱动；只观察到 `maxAttempts:11`/`maxRetries:10`              |
| 403/400/429 分类                               | 未驱动                                                         |
| `model_stream_stalled` / idle timeout          | 未触发（耗时不可控）                                           |
| 工具调用中途断流的 recovery                    | 只注入文本 delta 中断                                          |
| stream recovery 期间取消                       | 未观察（retryDelay 0，窗口太短）                               |
| `retry-after` 覆盖退避                         | 未观察                                                         |
| 后台 Bash 失败/killed 通知（C3 时序）          | C1 已观察，C3 未重录                                           |
| 前台工具执行期间的忙时完成（C1 时序）          | C1 观察到丢弃；C3 只录了模型流式等待窗口的合并延后，二者未统一（Rust 以工具窗口近似丢弃，见 §3.5/§3.6） |
| 3+ pending 通知的合并顺序                      | 未观察                                                         |
| notification turn 与 retry/goal 的更多组合     | 只观察 §3.3 一种                                               |
| subagent waiting/blocked/failed/cancelled/lost | 未观察                                                         |
| `session_task_link` 的 subagent 行             | 三种路径都是空表                                               |
| `endedLimit` 默认 20 的运行时行为              | 20 来自协议 schema；空数据下省略与 20 不可区分；>20 截断未观察 |

## 6. 夹具与复现

| 夹具                                  | 记录内容                                                                                                                    |
| ------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| `goal-show-set-complete.json`         | show(无 goal)/pause/resume/clear(无 goal)、set、control turn、continuation、verifier 请求、target 行/turn_usage/model_usage |
| `goal-pause-resume-clear.json`        | provider 请求前 pause（取消、model_request_count:0）、resume 完成、clear、target 行删除、read 投影                          |
| `goal-pause-in-flight.json`           | `model_request_started` 后 pause：in-flight cancelled、model_request_count:1、账务                                          |
| `goal-set-over-existing.json`         | 对 paused 目标再次 set：previousTarget 非 null、target-1→target-2、覆盖后完成                                               |
| `goal-verifier-invalid-json.json`     | verifier 默认非 JSON 文本的 fallback：target complete、reason、timeline/DB                                                  |
| `goal-replace-revision-conflict.json` | -32009 冲突与零副作用、正确 revision 的 replace、运行中 set/replace/resume/clear 的 -32010、参数错误                        |
| `goal-user-stop.json`                 | `session/stop` 取消续跑、target paused、turn_usage cancelled                                                                |
| `goal-cold-restart-paused.json`       | paused 冷重启、resume 返回 goalStats、无自动 turn、显式 resume 后完成                                                       |
| `goal-cold-restart-active.json`       | active 运行中杀进程、重启转 paused、active 字段清空                                                                         |
| `goal-cold-restart-complete.json`     | completed（tokensUsed 22）冷重启：状态/账务保留、无自动 turn、show complete                                                 |
| `goal-continuation-loop.json`         | verifier false 的 40 轮续跑（未观察到上限）、pause 中断、timeline 窗口                                                      |
| `retry-5xx-then-success.json`         | 503→retry_scheduled→成功、apiRetry、network 事件、账务                                                                      |
| `retry-backoff-sequence.json`         | 3 次 503 的退避序列（delay 折叠）                                                                                           |
| `retry-stream-abort.json`             | 已提交 delta 后的 4 种 streamRecovery 形状、丢弃行、最终文本                                                                |
| `retry-401-non-retryable.json`        | 401 非 retryable、turn.failed 形状、账务                                                                                    |
| `retry-cancel-during-backoff.json`    | retry_scheduled 后 stop、无第二次请求、cancelled 终态                                                                       |
| `stream-recovery-c1-c2-shapes.json`   | 38 个 C1 夹具里 10 个去重 streamRecovery 形状                                                                               |
| `background-idle.json`                | 空闲后台完成、`background_task` turn、通知文本、DB 消息语义                                                                 |
| `background-busy-coalesced.json`      | 两个任务忙时完成合并为一个 notification turn                                                                                |
| `background-goal-interaction.json`    | notification turn 在 verifier 前、计入 goal tokensUsed                                                                      |
| `subagents-empty.json`                | 首 turn 前 -32004、空列表、endedLimit 边界                                                                                  |
| `subagents-foreground.json`           | 前台 Agent 的 running/ended、生命周期事件、工具结果、子会话 DB 行                                                           |
| `subagents-background.json`           | 后台 Agent 通知、`backgroundSubagentResultConsumed:true`                                                                    |
| `subagents-pagination.json`           | endedLimit/endedCursor、游标解码形状、childSessionIds 顺序                                                                  |

回放/对拍注意：

- 夹具的 `expect` 已把目标 id、消息/part id、时间、退避、PID、游标折叠成 token；
  比较时不要重新规范化原始事件（C3 的事件在录制时走 `normalizeC3Deep`）。
- `goalVerificationTimeline` 在 loop 夹具里只保留 count + first/last（完整 40 条会在
  `goal-continuation-loop.json` 的 `db.turn_usage` 等大数组里重复；实现对照以字段形状为准）。
- 分页夹具用串行创建消除并行子代理到达顺序的不确定性；lifecycle 事件仍按 description 排序记录。
- C1/C2 夹具与探针未修改；`pnpm test:agent-runtime-oracle` 的
  `session-stream` 目录覆盖失败是基线已有问题（该根夹具不在 `FIXTURE_CASES`），
  C1 回放 39/39、C2 回放 11/11 仍通过。

## 7. 对 C3 实现的直接约束

1. **goal 管理在 prompt 运行中拒绝**（除 `pause`）：`-32010`；`show` 也不例外。
2. **pause 立即置 paused 并取消当前 continuation**：provider 请求前取消时
   `model_request_count:0`；in-flight 取消时 `model_request_count:1`、`model_usage`
   记 cancelled、tokens 0。不要把 response snapshot 当最终状态，也不要重复发终态。
3. **续跑 turn 使用 `run_started/run_finished` 包裹**，target 的 tokens/time 只在
   `run_finished` 累计；控制 turn（`executionKind:"controlOnly"`）token 为 0。
4. **verifier 是独立非流式请求**（`target_completion_verification`，tools 0）；
   通过才 `complete`；非法 JSON 的线上 fallback 是 passed=true
   （`goal-verifier-invalid-json.json` 录制；若实现不复制，必须在差异表登记）。
5. **`session_target` 是唯一事实源**；`clear` 删行；`session/read` 与快照的
   `session.target` / `projection.target` 同源。
6. **冷恢复不自动续跑**：active 转 paused；只有显式 `resume` 才启动新 turn；
   target 的 tokens/time 从 DB 恢复，runtime/projection 计数归零。
7. **HTTP 5xx 走 adapter 重试**（`model_retry_scheduled` + `apiRetry`），
   **流中断走 streamRecovery**（4 条 `streamRecovery.updated`，retryDelay 0，
   attempt 仍为 1）；两条路径的账务字段不同（§2.2/§2.3），不能合并成一种计数。
   Rust T2 把两条路径的重试循环放在 `agent-core`（brief 指定），事件/账务仍按此
   区分；未产出的 `model_request_*` 子事件登记在 §2.7。
8. **已完成 delta 不进入最终消息**；保留 `stream_recovery_discarded` 历史行。
9. **后台通知在模型流式等待窗口内忙时完成会合并延后**：一个 notification turn 可含
   多条 `<task-notification>`；notification turn 计入 goal，并在 verifier 之前执行。
   前台工具执行期间完成的 C1 时序未重录，不能据此推广。
10. **subagents 只读面**：空表/首 turn 前 -32004；`session_task_link` 不承载 subagent
    行；ended 按 endedAt 倒序分页；后台结果消费用
    `turn.completed.backgroundSubagentResultConsumed:true` 标记。
