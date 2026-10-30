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
- `session/list`、`session/events` 之外的方法按需补类型与录制夹具。

## 6. 验收

1. 共享夹具 `request-session-list.json` / `response-session-list.json` /
   `request-session-events.json` / `response-session-events.json` /
   `request-session-subagents.json` / `response-session-subagents.json` /
   `request-session-messages.json` / `response-session-messages.json` 同时被 TS zod 与 Rust
   严格镜像接受；Rust 侧序列化回到线上字节。
2. `sessionIds` / `limit` / `afterSeq` / `endedCursor` / `afterMessageId` 缺省时不出现键；
   `includeArchived=false` 与 `endedLimit=20` 恒出现。
3. `existing-only` 在没有活进程时返回 runtime-unavailable 错误且 spawn 次数为 0。
4. 正常路径返回的 `sessions` / `events` / `subagents` / `messages` 逐项可读；同一 workspace
   复用同一进程。
5. 事件信封、子代理结果或消息多一个键时整条结果被拒绝，消息前缀分别为
   `invalid session/events result`、`invalid session/subagents result`、
   `invalid session/messages result`。
