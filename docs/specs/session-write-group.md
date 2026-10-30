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
