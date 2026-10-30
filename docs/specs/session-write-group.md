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
