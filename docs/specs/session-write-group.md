# 会话写路径组（create / close / setter）

> 范围：`crates/agent-protocol::{session_create,session_write}` 与 `crates/agent-process`
> 的 facade 方法。对应 TS `modeAgentService` 的 `createSession` / `closeSession` /
> `setModel` / `setThoughtLevel` / `setMode`。

## 0. `session/create`

请求字段（`modeSessionCreateParamsSchema`，全部 `.strict()`）：

```
{sessionId?, workspace, parentSessionId?, mode?, model?, persistence?, thoughtLevel?,
 titleGenerationEnabled?, mcpServers?, toolAllowlist?, toolDenylist?, importedHistory?,
 offPeakToolEnabled?, dynamicWorkflowEnabled?}
```

- `workspace` 用严格的 `SessionWorkspaceRef`（四键）；`buildWorkspaceRef` 规则不变。
- `mcpServers` 是 stdio / http(sse) 的联合：stdio 必填 `command/args/env`；http 必填
  `type/url/headers`，可选 `oauth`（`client_credentials` 或 `authorization_code`）。
- `importedHistory` 按 `source` 分 `claudeCode` / `remoteMirror` / `sharedContext`；
  `sharedContext` 带 `provenance`（三个 sha256、formatterVersion=1、installedArtifacts）。
- `offPeakToolEnabled` / `dynamicWorkflowEnabled` 只在 **true** 时下发，`false` 与缺省等价
  （CLI 缺省不注册工具簇）。
- 结果：完整 `modeSessionStateSnapshotSchema`。

**与 TS 的差异**：TS 在 `getClient` 前先 `ensureAccountProviderConfigSynced` 并解析
Off-Peak / 动态工作流灰度；本 facade 接收已解析的布尔值，provider 同步留给服务层。

## 0b. `session/send`

请求（`modeSessionSendParamsSchema`，`.strict()` + `superRefine`）：

```
{sessionId, modelSelection?, modelExecution?, inputId?, queryId?, content, attachments?,
 browserAmbientContext?, expectedRevision?, expectedProviderRevision?, automationId?,
 offPeakTaskId?, offPeakRunType?, botDeliveryTarget?, toolDenylist?}
```

- `attachments` 是开放对象数组（`record<string, unknown>`）；`browserAmbientContext`
  是 `{tabCount(1..100), currentUrl?}`；`botDeliveryTarget` 是
  `{provider(6 值), botId, providerUserId, chatType}`。
- `modelExecution` 是单次执行约束：`selectionScope=execution` 必填，可选
  `memoryExtraction=skip`、`requestAuth{apiKey?,headers?}`、`subagents{foregroundModel=submission,background=deny}`。
- `superRefine` 三条约束在发出前校验：`automationId`/`offPeakTaskId` 互斥、
  `offPeakRunType` 依赖 `offPeakTaskId`、`modelExecution` 依赖 `modelSelection`；
  违规返回 `ControlError::InvalidParams`（不拉起 Agent）。
- 结果 `{sessionId, accepted, stateRevision}`，`accepted` 在协议里是 `z.literal(true)`；
  facade 对 `false` 报 `invalid session/send result: accepted must be true`。

## 1. 其余方法

| 方法 | 协议方法 | params | result |
| --- | --- | --- | --- |
| `close_session` | `session/close` | `{sessionId, expectedPersistence?}` | `{closed?}`（缺省按成功 `true`） |
| `set_session_model` | `session/setModel` | `{sessionId, model, expectedRevision?, persistAsWorkspaceLastUsed?}` | 完整状态快照 |
| `set_session_thought_level` | `session/setThoughtLevel` | `{sessionId, thoughtLevel?, expectedRevision?, persistAsWorkspaceLastUsed?}` | 完整状态快照 |
| `set_session_mode` | `session/setMode` | `{sessionId, mode, expectedRevision?}` | 完整状态快照 |

- `persistAsWorkspaceLastUsed` 在 schema 里是 `z.boolean().default(true)`，但请求侧语义是
  「undefined 省略」：Rust 镜像用 `Option<bool>`，缺省时不写字段。
- `thoughtLevel` 缺省同样省略键。
- 三个 setter 的结果复用 `agent_protocol::snapshot::SessionStateSnapshot` 严格解析。

## 2. 与 TS 的差异

TS 的 `getClient` 会先读 model-selection readiness，未就绪时抛 `provider_not_ready` 且不拉起
Agent。本 facade 只做 `get_or_spawn`；readiness 门禁属于服务层编排，接入频道时补齐。

## 2b. 反向请求：`session/requestRuntimePreferences`

运行时向 host 发 `{sessionId, scope}`（`runtime-materialization` / `user-execution`），host
在 `authority === "local"` 时直接回默认值：

```json
{ "askUserQuestionAutoResolutionEnabled": true,
  "nativeSearchEnhancementsEnabled": true,
  "memoryEnabled": false }
```

- `agent-process::runtime_preferences_handler(resolve)` 负责解析参数、调用注入的 scoped
  resolver 并编码结果；`default_runtime_preferences_handler()` 恒用默认值。
- 参数非法回 `-32602 Invalid session runtime preferences request params`；resolver 报错回
  `-32603 <message>`；其它方法 `NotHandled`（协议客户端回 `-32601`）。
- `AgentRuntimeRegistry::set_request_handler` 对现有进程立即生效，并应用到之后拉起的进程；
  `None` 清除（恢复默认 `-32601`）。

## 2c. 事件订阅：`session/subscribe` + `session/event`

`subscribe_session(target, sessionId, deliveryKind, afterSeq?, includeSnapshot?)`：

1. 先订阅协议客户端的通知广播，再发 `session/subscribe`（`params` 里 `deliveryKind` 必填、
   `includeSnapshot` 恒发、`afterSeq` 可选），避免 replay 与首条 live 之间丢帧。
2. 应答 `{sessionId, eventSeq, events[], snapshot?}` 严格解析；replay 事件先按
   `seq == 0 → 订阅内单调补号` 归一，再按 `(seq, timestamp)` 排序，并按 `eventId` 去重。
3. 返回 `SessionSubscription { replay, snapshot, recv() }`；`Drop` 中止转发任务。
4. live 流只接收 `method == "session/event"` 的通知，严格解析为 `SessionEvent`，只投递
   `sessionId` 匹配的事件；同样补号 + 按 `eventId` 去重（有界 2000）。

**与 TS 的差异**：

- TS 的补号/去重状态是 workspace/session 级共享（先归一再 fan-out）；Rust 目前是**订阅级**。
  单订阅者语义一致，多订阅者各自对 `seq == 0` 的旧事件独立补号。
- TS 对订阅建立失败做指数退避重试；Rust 直接把错误返回给调用方（重试由上层决定）。
- 通知广播落后（`Lagged`）时跳过被丢弃的通知继续消费；TS 无此路径。

## 2d. workspace 只读面

- `read_workspace_presentation(target)` → `workspace/readPresentation`：
  `{workspace}` 请求，结果 `{workspace, mode, slashCommands[]}` 严格解析。
  旧 client 在发请求前被换代关闭（`ClientError::Closed` / `Transport`）时重新取 client
  重试**一次**，与 TS 的 closed-transport 重试一致。
- `list_child_processes(target)` → `process/childProcesses`：请求**不带 params**，
  结果 `{processes:[{pid, serverName, mcpSource(builtin|plugin|custom), pluginName?}]}`。
  TS 的 `collectLocalRuntimeChildProcesses` 会遍历本 Host 全部 runtime（含 plugin /
  mcp-status 泳道）；本 facade 只覆盖单个 workspace runtime。

## 2e. `mcp/list`

`list_mcp_server_statuses(target, {mcpServers?, mode?})` → `mcp/list`：
`{workspace, mcpServers?, mode?}`（`mode` 缺省省略；schema 默认 `connect`）。结果
`{statuses: {serverName: snapshot}}`，snapshot 含 `status`（6 值）、`transport`（3 值）、
`toolCount`、`updatedAt`，可选 `error`/`failureKind`（19 值）/`serverRequestId`/
`protocolEra`/`authorization`。

**与 TS 的差异**：TS 使用独立的 mcp-status 泳道 client，并按旧 Agent 的
`unrecognized_keys` 对 `mode`/`mcpServers` 做兼容重试；本 facade 复用 workspace runtime，
不做该重试（错误原样上抛）。

## 2f. 工作流只读面

- `list_saved_workflows(target, scope?)` → `workflows/list`：`{workspace, scope?}`，
  结果 `{workflows[], invalid[], dir}`；条目含 `name/description/whenToUse?/args?/scope/path`，
  `args` 的声明含 `type/description?/required?/default?`（`default` 是 unknown-optional，
  三态保留 `null` 与省略）。
- `get_saved_workflow(target, name, scope?)` → `workflows/get`：联合结果——
  `ok:true` 分支带 `meta` 与脚本本体 `script`；`ok:false` 分支带
  `reason(invalid_name|not_found|parse_error|read_error)` 与可选 `detail`。
- `list_saved_workflow_runs(target, name?, limit, scope?)` → `workflows/runs`：
  `limit` 必须 1..=50（违规在发出前报 `InvalidParams`，不拉起 Agent）；
  run 含状态/stopReason/时间/`spentTokens`/args/cwd/artifacts（≤8，`version` 为 JSON 数字）。
- `update_saved_workflow_meta(target, name, meta, scope?)` → `workflows/updateMeta`：
  `{workspace, name, meta, scope?}`，结果 `ok:true{path}` 或 Failure。
- `delete_saved_workflow(target, name, scope?)` → `workflows/delete`：同上联合。
- `move_saved_workflow(target, name)` → `workflows/move`：`{workspace, name}`（workspace 既是
  载体也是目标项目），成功 `{ok:true,from,to}`；失败 `ok:false` 带
  `reason(invalid_name|not_found|target_exists|read_error|write_error)` 与可选 `path`/`detail`。

**与 TS 的差异**：全局档（`scope: global`）的载体运行时由 TS 自行挑选（可能回落到 plugin
管理面进程）；本 facade 直接使用调用方给的 workspace target 作为载体。

## 2g. v4 命令最小闭环：`resolveInteraction`

`resolve_interaction_v4(target, sessionId, interactionId, answer)` → `v4/command`：

```json
{ "commandId": "…", "clientId": "…", "sessionId": "…", "type": "resolveInteraction",
  "payload": { "interactionId": "…", "answer": { "optionId"?, "freeText"?, "action"?, "content"? } },
  "issuedAt": 1700000000000 }
```

- ACK 六态：`accepted` / `duplicate` / `noop` 视为幂等成功；`rejected` / `stale` / `failed`
  返回 `CommandRejected`，消息形如
  `v4 command resolveInteraction rejected (reasonCode): message`（与 TS `assertV4CommandAckOk` 对齐）。
- ACK 的 `result` 是按命令类型的判别联合；本镜像只发送 `resolveInteraction`（该命令不产生
  result），所以 `result` 保持原始 JSON。
- **与 TS 的差异**：`commandId` / `clientId` 只保证非空且按提交端稳定，不是 UUID v7；
  `helloConversationV4` / 信任连接与订阅尚未迁移（`resolveInteraction` 不依赖它们）。

## 2h. 插件只读面

- `list_plugins(target, configScope?)` → `plugins/list`：`{workspace, configScope?}`，
  结果 `{plugins[], diagnostics[]}`。插件条目含 `id/name/enabled/source/marketplace/
  skillRootCount/commandRootCount/mcpServerNames/rootPath` 等必填项，以及
  `components[]`（agent/command/skill/hook/mcp 分组）、`hookDetails[]`（含
  `shell: true|string`、`timeout` 数字）、`userConfig`/`configuredOptions`/`*Source` 等可选面。
- `describe_plugin(target, marketplace, pluginName)` → `plugins/describe`：
  结果 `{components[], diagnostics?, metadata?}`。
- `get_plugins_overview(target, configScope?)` → `plugins/overview`：市场摘要
  （`source` 开放对象、`featured`、可选 `refreshFailure`）、可安装/已安装/可恢复内置摘要
  （`listing` 全部可选面、`updateStatus` 三值）、诊断、可选 `officialMarketplaceEnabled` 与
  必填 `capability{supported,reason?}`。

**与 TS 的差异**：TS 使用独立的 plugin 管理进程（合成 workspace）与更长超时；本 facade
复用 workspace runtime、使用协议默认超时。

## 2i. v4 会话订阅：`v4/conversation/subscribe` + wire frame

`subscribe_conversation_v4(target, params)`：

- 请求：`{topic, base?{logEpoch,seq}, visibility?, connectionId, clientMode, workspace?,
  legacyTaskIds?, resumeThoughtLevel?, workflowRunDeltas?}`；`legacyTaskIds` 上限 200
  （越界返回 `InvalidParams`，不拉起 Agent）。
- 应答严格只含 ACK：`{subscriptionId, mode(snapshot|resume), logEpoch, openTiming?}`；
  首帧（snapshot 或 resume）随后以 `v4/conversation/frame` 通知发出。
- `ConversationFrameSubscription`：`ack` + 已按 `subscriptionId` 过滤的 wire frame 流；
  `frame` 支持 `complete`（内嵌会话帧原文）与 `fragment`（base64 分片 + crc32 校验）；
  `Drop` 中止转发任务。
- **范围**：wire 外壳严格镜像；`frame` 里的会话快照/增量（`conversationSnapshotSchema` /
  `conversationDeltaSchema`）体量巨大，暂按原始 JSON 透传，等独立切片再建模。
- 通知广播 `Lagged` 时跳过被丢弃的通知继续消费。

**与 TS 的差异**：TS 在订阅前同步 Account Provider Config 并读取 task-index 的 thought
hint；本 facade 由调用方在 `params` 里给出 `resumeThoughtLevel`。信任连接上下文
（`connectionId` / `clientMode`）也由调用方提供，不在本层推断。

## 2j. v4 连接握手

- `build_host_hello(connectionId)`：与 TS `helloConversationV4` 同形——
  `{kind:"hello", protocolVersion:3, connectionId, clientMode:"desktop-continuous",
  deliveryProfile:"continuous", serverTime, capabilities{nativeDialogs,localTerminal,
  binaryFrames,compression,workspaceHookReview,independentPlanState,workflowRunDeltas},
  auth:{}}`；`validate()` 强制 clientMode 与 deliveryProfile 匹配。
- `validate_client_hello(value)`：严格解析 `clientHello`（未知键拒绝），对应 TS
  `initializeConversationV4`。

**与 TS 的差异**：TS 用 `host-${randomUUID()}` 生成 connectionId 并维护连接级信任；
Rust 只提供形状与校验，connectionId 由频道层/调用方生成。

## 2k. v4 会话行（第一批）

`agent-protocol::v4_rows` 镜像 `rows.ts` 的前四种行：

- `RowBase`（`rowId/turnId/entityId?/productTurnId?/visibility?/createdAt/createdAtSeq/actions?`）
  通过 `#[serde(flatten)]` 复用到各行。**注意**：这些 row schema 在 zod 里是普通
  `z.object()`（不带 `.strict()`），未知键会被剥离；Rust 侧同样忽略未知键，不用
  `deny_unknown_fields`。
- `turnHeader`：origin/executionKind/state、工时与 `workSegments`、`fileChanges`、
  `originMeta`（后台结果归属，`workflowNotification` 暂按原始 JSON）与 `workflowLaunch`
  （`display`/`amend` 暂按原始 JSON）。
- `userInput`：origin、epilogueStart、guided、command 归因、附件引用。
- `assistantText` / `reasoning`：`state` 枚举、assistantResponseId、feedback / durationMs。
- `toolCall`：状态六值、`inputText`/`input`、`cuaApp`、`output{text,display?,truncated?}`、
  `progress`、`outputPreview`、`error{code,message}`、`approvalInteractionId`、
  `backgrounded`/`workId` 与起止时间。`display` 在 TS 里是 `.catch(undefined)`，
  这里保持原始 JSON。
- `artifact`：版本/逻辑键/展示名/类型（8 值）/mime/字节数/sha256/ref/state。
- `subagent`：父 toolCall、类型、状态四值、summary、childSessionId、后台归属。
- `hookInvocation`：hook 事件名（7 值）、lane（assistantWork/toolBefore/toolAfter）、
  执行投影（hookRunId/hookIndex/didExecute/state/outcome/blockReason/起止与耗时/sourceKind）。
- `timelineMarker`：9 种 payload——compact（origin/status/tokens/summaryRef）、
  forkNotice/forkCreated、modelChange（有来源 / 显式 ∅→X 两种形态）、goalSet、goalVerify、
  retryNotice、checkpointRestored。
- `ConversationRow`：上述 9 种行的 untagged 判别联合（各分支自带 `kind` 字面量）。
- 行动作 `canFork/canEdit/canRetry/canRewindFiles` 用 `LiteralTrue` 保持「只发 true」的形状。

**未做**：`conversationSnapshot` / `conversationDelta` 与工具 display 的强类型
（`toolCall.display` 仍按原始 JSON）。

## 2l. v4 会话快照（`conversationSnapshot`）

`agent-protocol::v4_snapshot` 镜像 `snapshot.ts` 的 A 区与 rows 窗口：

- `control`（phase/stopState/stopTargetKind/activeWorks/lastError/apiRetry）、
  `availability`（8 个动作的 allowed/denied 联合）、`inputRouting`、`meta`、
  `sharedContextImport`（V2 与 legacy 两形态）、`config`（会话模型/思考/协作模式）、
  `modelTransition`、`usage`（上下文窗口 + 累计）、`queue`（含 intent 全字段与队列态
  `dispatch`、附件、provenance）、`pendingInteractions`（permission / userInput /
  workspaceHookReview 三种 payload）、`pendingCommands`、`backgroundWorks`、`subagents`、
  `goal`/`plan`、`workspaceHookAdmission`、`rows`（`ConversationRow` 窗口 + 全序计数）。
- `goal` 的 `summaryTitle` 是 `nullable().default(null)`，用三态保留显式 null；
  `whitespace`/默认值字段按 zod 默认补齐（fixture 显式给出以避免序列化差异）。
- `workspaceHookReview` payload 只镜像形状；zod `superRefine`（kind 与 payload 一致、
  deadline、hookCount 等）暂未在 Rust 侧强制，留待消费方校验。

## 2m. v4 工作流运行态（`workflowRuns`）

`agent-protocol::v4_workflow_runs` 镜像 `workflow-runs.ts`：

- `WorkflowRunState`：状态五值、stopReason、lineage（resumedFrom/supersededBy）、usage
  （含 `nodesUnlisted*`）、可恢复位、结果/错误预览、actors/nodes、reports、
  pendingQuestions、concurrency（cap/ceiling/limit/cooldown）、concurrencyCeiling、
  subagentModel、artifacts（产物摘要）、phases/currentPhase/phaseNames/phaseAlongside、
  unlistedByPhase、truncated 与 lastEventSequence。
- `WorkflowRunActor`/`WorkflowRunNode`：站点身份（siteId/ordinal）、三态/七相位、
  instructionsHead、turn/toolCalls/lastTool、出生阶段 phaseName。
- 「零条即缺席」的集合（reports/pendingQuestions/artifacts/phases/phaseNames/
  phaseAlongside/unlistedByPhase）用 `Option` 保持键缺席而非空数组。
- `ConversationSnapshot.workflow_runs` 已从原始 JSON 收紧为 `WorkflowRunsState`。

**未做**：`state.updated` 补丁与 `conversationDelta`。

## 3. 验收

1. 共享夹具覆盖六个请求与四个应答：`request/response-session-close`、
   `request/response-session-set-model`、`request/response-session-set-thought-level`、
   `request/response-session-set-mode`，带 MCP/导入历史的 `request-session-create-import`，
   以及带执行约束/附件/Off-Peak/Bot 的 `request-session-send-execution`；TS zod 与 Rust
   镜像同时接受，Rust 序列化回到线上字节。
2. `expectedRevision` / `persistAsWorkspaceLastUsed` / `thoughtLevel` 缺省时不出键。
3. `create` 的全部可选字段缺省时不出键；灰度布尔只在 true 时出现。
4. `closed` 缺省时返回 `true`。
5. 非法结果分别报 `invalid session/close result`、`invalid session/create result` 与
   `invalid session/<setter> result`。
6. 订阅：replay 的 `seq == 0` 事件被补成单调序号并排序；重复 `eventId` 与其它 session
   的 live 事件不投递；`session/subscribe` 参数逐字段符合 schema。
7. workspace 只读面：展示结果的 workspace/mode/slashCommands 可读；子进程请求不带 params
   且结果按 schema 解析；两类非法结果有独立错误前缀。
8. `mcp/list`：`mode` 缺省省略；状态快照按 schema 解析，非法结果前缀
   `invalid mcp/list result`。
9. 工作流：list 的 args 声明与 `default:null` 原样保留；get 的成功/失败分支都能解析；
   runs 的 `limit` 越界不拉起 Agent；updateMeta/delete/move 的参数与联合结果逐项可读，
   非法结果有独立前缀。
10. v4 回执：信封的 type/payload/sessionId 与稳定 clientId 可断言；`accepted` 返回 ACK，
    `rejected` 映射为 `CommandRejected` 并携带状态与 reasonCode。
11. 插件只读：list 的组件分组/hook 详情/诊断可读；describe 的组件与 metadata 可读；
    overview 的市场/可安装/已安装摘要与 capability 可读；三类非法结果有独立前缀。
12. v4 会话订阅：ACK 的 subscriptionId/mode/openTiming 可断言；`complete` 与 `fragment`
    帧按 subscriptionId 过滤后到达；`legacyTaskIds` 超限不拉起 Agent。
13. v4 握手：host hello 的能力位与 TS 一致并 JSON round-trip；clientHello 未知键被拒绝。
14. v4 行（第一批）：四种行 JSON round-trip；未知键按 zod 的 strip 语义被忽略。
15. v4 行（第二批）：toolCall（output/progress/outputPreview）、artifact、subagent 各自
    round-trip。
16. v4 行（第三批）：hookInvocation 与 execution 投影、timelineMarker 的 9 种 payload、
    `ConversationRow` 联合对 9 种 kind 的解析。
17. v4 快照：完整 A 区 + rows 窗口 JSON round-trip；`goal.summaryTitle: null` 的三态与
    `workspaceHookAdmission` 的显式 null 保留。
18. v4 工作流运行态：完整 run（actors/nodes/reports/pendingQuestions/concurrency/phases/
    artifacts）与空状态各自 round-trip。
