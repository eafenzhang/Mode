# Spec：sessions-index topic（Rust 协议镜像）

状态：2026-11-01 首版。真源：`packages/shared/src/mode-protocol-v4/sessions-index.ts`、
`sessions-index-workflow-activity.ts` 与 `transport.ts` 的
`sessionsIndexTopicFrameSchema` / `createTopicWireFrameSchema`。

## 1. 范围

| Rust                                                  | 说明                                                                                                   |
| ----------------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| `sessions_index_topic` / `parse_sessions_index_topic` | `sessions-index/<workspaceId>`，workspaceId 非空                                                       |
| `SessionSummary`                                      | 侧栏列表行；`titleSource` / `workflowActivity` / pending 摘要 / `goalStatus` 均可缺席（旧 frame 兼容） |
| `SessionPendingInteractionSummary`                    | 只允许 `permission` / `userInput`；toolName 与 autoResolution 可选                                     |
| `SessionWorkflowActivity`                             | 最多 4 条 run；每条含 phases（≤32）、currentPhase、agentsWorking                                       |
| `SessionsIndexSnapshot`                               | `protocolVersion = 1` + workspaceId + logEpoch + sessions（无序）                                      |
| `SessionsIndexDelta`                                  | `session.upserted` / `session.removed`（discriminated on `op`）                                        |
| `SessionsIndexTopicFrame`                             | 与 conversation 同五件套，但没有 `ttft` / `ttftRelated`                                                |
| `SessionsIndexWireFrame`                              | complete（强类型 frame）或 fragment（只校验 topic 前缀）                                               |

## 2. 校验

- topic 必须 `sessions-index/` 前缀且后缀非空；`validate()` 在帧与 wire frame 两层都检查。
- `protocolVersion` 只接受 1。
- `workflowActivity.runs ≤ 4`；每个 run 的 `phases ≤ 32`、phase 名 1..=128、
  `alongside` 下标必须落在本 run 的 phases 数组内。
- `sessionId` / `runId` / `toolCallId` / `name` 的非空约束与 zod schema 一致。
- 内层 `z.object` 未 `.strict()`，Rust 不加 `deny_unknown_fields`（未知键按 strip 语义忽略）；
  帧信封本身是 strict，保持 `deny_unknown_fields`。
- `SessionsIndexDelta` 的字段命名用 `rename_all_fields = "camelCase"`（internally-tagged
  enum 的 variant 字段不会继承 `rename_all`）。

## 3. 传输

sessions-index 与 conversation 共用 `v4/conversation/subscribe`（`topic` 自由字符串），
没有新增 RPC 方法；本模块补 topic 载荷、帧与 wire frame。分片重组仍由消费方负责，
与 conversation topic 一致。

客户端 facade：`AgentControlClient::subscribe_sessions_index_v4` 复用同一个 subscribe RPC，
发送前校验 `sessions-index/<workspaceId>`；`SessionsIndexFrameSubscription` 从
`v4/conversation/frame` 通知里按 `subscriptionId` 过滤并反序列化为
`SessionsIndexWireFrame`。conversation 订阅与 sessions-index 订阅共用私有泛型
`subscribe_topic_frames`，避免两条链路各自实现过滤逻辑。unsubscribe/resync 复用现有
`v4/conversation/unsubscribe|resync`（同为 topic 字符串）。

## 4. 验收

`cargo test -p agent-protocol --test v4_sessions_index`（5 条）：

1. topic 构造/解析，空 workspaceId 与错误前缀拒绝；
2. snapshot 帧含 workflowActivity/pending/goalStatus 的解析，旧 frame 缺省字段仍可读；
3. 增量 upserted/removed 解析，未知 `op` 拒绝；
4. topic、protocolVersion、runs/phases/alongside 越界；
5. complete/fragment wire frame 解析与 topic 前缀校验。

`cargo test -p agent-process --test v4_transport`（sessions-index 相关 2 条）：

1. 订阅请求帧 topic 正确、ACK 无 openTiming、首帧反序列化为
   `SessionsIndexWireFrame::Complete` 且 snapshot.workspaceId/logEpoch 正确；
2. 非 `sessions-index/` topic 在 spawn 前以 `InvalidParams` 拒绝。
