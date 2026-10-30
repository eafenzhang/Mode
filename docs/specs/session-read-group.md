# Agent 会话只读组（session read group）

> 范围：`crates/agent-protocol::session_read`（严格线格式镜像）与
> `crates/agent-process` 的控制面方法。第一组先落 `session/list`，后续按同一形状补
> `session/events`、`session/messages`、`session/subagents`、`session/read`。

## 1. 行为真源

- `packages/shared/src/mode-protocol-legacy-types.ts` 的 `modeSessionInfoSchema` /
  `modeWorkspaceRefSchema` / `modeSessionGoalSchema`
- `packages/shared/src/mode-protocol/index.ts` 的 `modeSessionListParamsSchema` /
  `modeSessionListResultSchema`
- `packages/services/src/mode-agent/modeAgentService.ts` 的 `listSessions` 与
  `getReadOnlyClient`

这些 schema 全部 `.strict()`：多一个键就要拒绝，不能静默吞掉。

## 2. 请求

`session/list` 参数（服务层构造）：

```json
{
  "workspace": { "workspacePath": "…", "workspaceKey": "…", "workspaceIdentity"?: "…", "remoteSessionId"?: "…" },
  "includeArchived": false,
  "limit"?: 50,
  "sessionIds"?: ["…"]
}
```

- `workspace` 由同一个 `buildWorkspaceRef` 构造（`workspaceKey = identity.trim() || path`）。
- `includeArchived` 恒发（调用方未给时 `false`）。
- `limit` / `sessionIds` 缺省时**省略键**，不写 `null`。

## 3. 应答

`{ "sessions": SessionInfo[] }`，`SessionInfo` 字段：

| 字段 | 形状 |
| --- | --- |
| `sessionId` | 非空字符串 |
| `workspace` | 严格 `WorkspaceRef`（四个键，可选身份字段） |
| `parentSessionId` / `traceId` | 可选字符串 |
| `sessionKind` | `interactive` / `fork` / `selection_side_chat` / `workflow_parent` / `workflow_child` / `subagent_child` / `nested_workflow_child` |
| `title` | 字符串（可为空串，与 schema 一致） |
| `titleSource` | 可选：`default` / `first_input` / `generated` / `custom` |
| `mode` | `plan` / `build` / `edit` / `yolo` / `auto` |
| `status` | `idle` / `running` / `waiting` / `paused` / `completed` / `error` |
| `model` | 可选严格 `{providerId, modelId, options?}` |
| `target` | 可选可空 Goal 对象 |
| `createdAt` / `updatedAt` | 非负整数毫秒 |
| `archivedAt` | 可选非负整数毫秒 |

Goal 的 `summaryTitle` 在 schema 里是 `.default(null)`：**省略**与显式 `null` 都必须能读，
且镜像要能原样写回（省略就写省略、null 就写 null）。`activeInputId` / `activeRun*` 同理
（`nullable().optional()`）。

## 3b. `session/events`

请求：`{ sessionId, afterSeq?, limit? }`，缺省的可选键省略。

应答：`{ events: SessionEvent[] }`。事件信封（`modeEventEnvelopeSchema`）严格：

| 字段 | 形状 |
| --- | --- |
| `eventId` / `sessionId` | 非空字符串 |
| `turnId` / `traceId` / `deliveryKind` | 可选 |
| `seq` / `timestamp` | 非负整数 |
| `type` | 协议事件类型字面量（26 种，未登记取值直接拒绝） |
| `payload` | 可选，按 `type` 解释 |

**与 TS 的差异**：TS 用 `modeSessionEventSchema`（按 `type` 的判别联合）逐个校验载荷；Rust
镜像只对 M2 用到的载荷做了强类型（`session_event::payload_as`），其余载荷保持原始 JSON。
消费方需要某个载荷时按 `type` 解码，解不开即报错，不会静默接受错类型。

## 3c. `session/subagents`

请求：`{ sessionId, endedCursor?, endedLimit }`。`endedLimit` **恒发**：TS 用
`params.endedLimit ?? 20`，所以缺省是 20 而不是省略；`endedCursor` 缺省时省略。

应答：`{ revision, childSessionIds, running[], ended{ total, items[], nextCursor? } }`。
`running` 的 `status` 是 `running` / `waiting` / `blocked`；`ended.items` 的 `status` 是
`success` / `failed` / `cancelled` / `lost`。两条 schema 都是 base 字段 + `status` 的
`.strict()` 扩展（`childSessionId`、`subagentType`、`title` 必填，其余可选）。

## 3d. `session/messages`

请求：`{ sessionId, afterMessageId?, limit? }`，缺省的可选键省略。

应答：`{ messages: [{ info, parts }] }`：

- `info` 按 `role` 判别：`user`（`messageId/sessionId/role/time/agent` 必填，
  `model/system/tools/synthetic/source/visibility/semantics/metadata` 可选）与 `assistant`
  （另有 `parentMessageId/path/cost/tokens` 必填，`finish/error/semantics/structured` 可选）。
- `parts` 按 `type` 判别，共 13 种：`text`、`reasoning`、`file`、`tool`、`step-start`、
  `step-finish`、`snapshot`、`patch`、`compaction`、`timeline`、`subagent`、`agent`、`retry`。
  `tool.state` 再按 `status` 分 `pending/running/completed/error`。
- `cost` 保留 `serde_json::Number`：线上既有整数也有小数，转 `f64` 会把 `0` 写成 `0.0`。
- 判别联合在 Rust 侧用 `#[serde(untagged)]` + 各分支 `deny_unknown_fields`（serde 不支持
  「内部标记 + deny_unknown_fields」），未知键仍被拒绝。
- `assistant.structured`（`z.unknown().optional()`）用三态保留省略与显式 null。

## 3e. `session/read`

请求：`{ sessionId, deliveryKind?, messageLimit?, afterSeq? }`，缺省的可选键省略。

应答直接是 `modeSessionStateSnapshotSchema`（不是包装对象）：

| 字段 | 形状 |
| --- | --- |
| `protocol` | `{ name: "Mode Protocol" \| "ZCode Protocol", version: 1 }`（旧名仍被服务端接受） |
| `session` | 同 `session/list` 的 `SessionInfo` |
| `settings` | `model{current?, available[], lastUsed?}`、`thoughtLevel{enabled,current?,defaultLevel?,available[]}`、`mode{current}`、可选 `permission{mode?,rulesRevision?}` |
| `projection` | 状态/计数/上下文 + `pendingPermissions[]`、`activeToolCalls[]`、`backgroundJobs[]`、可选 `target`/`lastError` |
| `runtime` | `eventSeq/stateRevision/pendingRequestIds` + 可选 `deliveryKind`/`activeTurn*`/`apiRetry`/`contextUsage`/`goalVerifications`/`goalVerificationTimeline` |
| `messages` | 同 `session/messages` 的消息与部件 |
| `goalStats` / `todos` / `todoGroups` / `slashCommands` | 可选 |

细节：

- `pendingPermissions[].options[].kind` 是开放字符串（不是判别键），镜像保持原文；
  `decision` 是 4 值枚举，`permissionUpdates[].type` 目前只有 `addRules`。
- `lastError.attribution` 的 `statusCode` 限定 100–599；镜像用 `u16`，未做区间校验。
- 浮点字段（`contextUsage.cost.amount`、cache hit rate）用 `serde_json::Number` 保留原样。
- `runtime.apiRetry`、`contextUsage.cost`、`projection.target` 是 `nullable().optional()`，
  用三态解码。
- `projection.target` 与 `settings` 中的模型选择复用 `session/list` / `message` 的类型。
- **与 TS 的差异**：TS 在默认策略下先同步 Account Provider Config 再读；Rust facade 没有
  账号事实源，该同步留给服务层。
- `protocol.version: 1` 在镜像里是 `u64`，没有做「必须等于 1」的运行时校验；健康快照由
  夹具断言盯住。

## 3f. `session/debug`

请求：`{ sessionId }`。**existing-only**：诊断读取只观察现有进程，不拉起 Agent；
没有活进程时报 `Mode Agent runtime is not running.`。

应答：

| 字段 | 形状 |
| --- | --- |
| `rounds[]` | `{eventKey, requestId, requestIndex, recordedAt, usage{...可选 token 数}, hitRate(null), generationDurationMs(null), tokensPerSecond(null), prefill*?}` |
| `networkEntries[]` | `{eventKey, traceId, recordedAt, statusType(5 值), …可选诊断字段, requestHeaders, responseHeaders, requestHeaderCount, responseHeaderCount}` |
| `cache` | `{hitRateRequestCount, totalInputTokens, totalCacheReadTokens, hitRate(null)}` 或 `null`（键必在） |

`count = z.number().finite().nonnegative()` 允许小数，所以所有数值用 `serde_json::Number`
保留原样。`prefillDurationMs` / `prefillTokensPerSecond` 是 `nullable().optional()`，用
三态解码；`hitRate` / `generationDurationMs` 等是 `nullable()`，键必在。

## 4. 运行时策略

`getReadOnlyClient(params, runtimePolicy)` 的两种语义：

| 策略 | 行为 |
| --- | --- |
| `start-if-needed`（默认） | 复用或拉起 Agent；供读取历史/索引 |
| `existing-only` | 只复用现有进程；没有活进程时报 `Mode Agent runtime is not running.`（code `MODE_AGENT_RUNTIME_UNAVAILABLE`），**绝不 spawn** |

## 5. 不做的部分

- TS `listSessions` 会顺带把 `traceId` 记进进程内缓存；Rust facade 无该缓存，调用方按
  返回值自行保存。
- 交互偏好同步、Off-Peak 工具面同步、provider registry 预热属于 mode-agent 服务层编排，
  不在本组；接频道时由服务层在取 client 后补齐。
- `session/list`、`session/events`、`session/subagents`、`session/messages`、`session/read`、
  `session/debug` 之外的方法按需补类型与录制夹具。

## 6. 验收

1. 共享夹具 `request-session-list.json` / `response-session-list.json` /
   `request-session-events.json` / `response-session-events.json` /
   `request-session-subagents.json` / `response-session-subagents.json` /
   `request-session-messages.json` / `response-session-messages.json` /
   `request-session-read.json` / `response-session-read.json` /
   `request-session-debug.json` / `response-session-debug.json` 同时被 TS zod 与 Rust
   严格镜像接受；Rust 侧序列化回到线上字节。
2. `sessionIds` / `limit` / `afterSeq` / `endedCursor` / `afterMessageId` /
   `deliveryKind` / `messageLimit` 缺省时不出现键；`includeArchived=false` 与
   `endedLimit=20` 恒出现。
3. `existing-only` 在没有活进程时返回 runtime-unavailable 错误且 spawn 次数为 0。
4. 正常路径返回的 `sessions` / `events` / `subagents` / `messages` / 快照逐项可读；
   同一 workspace 复用同一进程。
5. 事件信封、子代理结果、消息或快照多一个键时整条结果被拒绝，消息前缀分别为
   `invalid session/events result`、`invalid session/subagents result`、
   `invalid session/messages result`、`invalid session/read result`。
