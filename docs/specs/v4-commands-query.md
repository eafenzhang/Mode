# Spec：`v4/commands/query`（Rust 协议镜像）

状态：2026-11-01 首版。真源：`packages/shared/src/mode-protocol-v4/command.ts` 的
`commandsQueryParamsSchema` / `commandQueryItemSchema` / `commandsQueryResultSchema`，以及
`apps/mode-cli/packages/bootstrap/src/mode-protocol-v4/persistent-command-index.ts` 的查询语义
（重连后按 `commandId` 与 CLI 权威事实对账）。

## 1. 范围

| Rust                                    | 说明                                                                                            |
| --------------------------------------- | ----------------------------------------------------------------------------------------------- |
| `agent_protocol::v4::CommandKey`        | `sessionId` required-nullable + `commandId`；手写 `Deserialize` 区分「缺失」与 `null`           |
| `CommandsQueryParams`                   | `commands` 1..=64；`clock` 仅允许字面量 `true`，且此时所有 key 必须是全局（`sessionId = null`） |
| `CommandQueryOutcome`                   | 结果联合：`commandAckSchema` 或字面量 `"unknown"`（`untagged`）                                 |
| `CommandsQueryResult`                   | `results` 1..=64 + 可选 `localTtftClock`（`instanceId` 1..=128，时间有限非负）                  |
| `AgentControlClient::commands_query_v4` | 发送前 `validate`；解析后再次 `validate`；不改写返回结果                                        |

常量：`V4_COMMANDS_QUERY_METHOD = "v4/commands/query"`。

## 2. 边界与差异

1. **required-nullable**：`z.object({ sessionId: z.string().nullable(), ... }).strict()` 要求键必须
   出现。serde 对裸 `Option` 会把缺失当成 `None`，所以 `CommandKey` 手写 visitor；未知键与重复键
   直接报错（等价 `.strict()`）。
2. **clock 探针**：协议 `superRefine` 禁止 `clock: true` 时查询会话命令；本地在发请求前拒绝
   （`ControlError::InvalidParams`），与其它 `superRefine` 镜像一致。
3. **ACK 内部**：`commandAckSchema.result` 仍是按命令类型的判别联合，本镜像沿用
   `CommandAck.result: Value`（见 `session-write-group.md` §2g），查询不收紧该字段。
4. **夹具**：`commands/query` 只在重连对账时由 host 发起，现有 oracle 录制脚本（一次
   session 往返）不会触发它，因此没有共享录制夹具；协议形状用字面量 JSON 的 4 条单测覆盖，
   控制面用 `agent-process-fixture` 的 `v4/commands/query` 分支验证请求帧与结果解析。

## 3. 验收

- `cargo test -p agent-protocol --test v4_commands_query`：required-nullable、未知键、
  clock 约束、ACK/unknown 联合、空结果与非法 clock（4 条）。
- `cargo test -p agent-process --test v4_queries commands_query`：请求帧逐字段比对、
  clock 结果解析、非法参数在 spawn 前拒绝、Agent 返回空结果时 `InvalidResult`（3 条）。
