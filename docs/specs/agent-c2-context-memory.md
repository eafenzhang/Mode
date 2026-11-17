# Spec：C2 上下文、记忆、压缩与 provider 扩展（LT-1 C2）

状态：**已批准方向（2026-11-17，用户「继续 C2」）**；实现前先由 C2-T0 补齐黑盒观察。
关联：`docs/specs/agent-core-parity.md`（LT-1 总 spec，C2 = §4/§5.7）、
`docs/plans/2026-11-17-lt1-c2-context-memory.md`（C2 计划）、
`docs/specs/tauri-agent-runtime.md`（对拍/白名单）、
`docs/specs/project-memory-service.md`（Host 侧只读记忆）、
`docs/specs/clean-room-policy.md`。

## 1. 目标与范围

C2 在 C1 的 Rust `agent-server` 上补齐四件事：

1. **上下文预算 v1**：模型能力（context window / max output tokens）进入装配，
   输入预算、历史裁剪、`contextUsage` 快照字段可用；
2. **compact**：`session/compact` 方法 + 预算触发的自动 compact；摘要落库、事件、
   失败语义与 oracle 一致；
3. **memory recall**：`memoryEnabled` 运行时偏好生效，项目记忆按数据根只读注入上下文；
   memory 写入边界由观察决定（见 §5.3）；
4. **provider 扩展**：Anthropic Messages 与 OpenAI Responses 的流式适配；
5. **fresh-install 迁移**（`agent-core-parity.md` §8 的 C2 承诺）：Rust 能在空数据根
   创建与 oracle 等价的 CLI 会话库（schema + 迁移账本 + checksum），只增不改；
6. **快照补齐**：`session/read`/`list` 的 `settings`/`projection`/`runtime`/`contextUsage`
   与 oracle 观察对齐（缺省项必须登记）。

**不在 C2**：MCP/Skills/Plugins/Subagents/Workflows/附件/v4 会话帧（C4）；流恢复与
goal/target（C3）；自动重试/退避（C3）；官方平台/遥测（永久下线）。

## 2. 上下文预算与装配

### 2.1 事实来源

- `ModelCapabilities { context_window, max_output_tokens }` 来自
  `ProviderConfigHub::resolve` 的模型目录能力；缺失保持 `None`，**不编默认值**，
  只在提示/预算里标注 `not reported`。
- 系统提示、模式/工作区事实、工具 schema 由本仓库编写；**不复制上游提示词**。
- 记忆/技能段只从本仓库数据与服务读取；技能实际接线属 C4，C2 只留装配插槽。

### 2.2 装配顺序（C2 定稿）

```text
系统提示（身份/工作区/模式/工具/安全/输出）
  → 项目记忆（memoryEnabled 时；只读、索引优先）
  → 工具 schema
  → 历史消息（含 compact 摘要）
  → 本轮输入
```

- 系统提示仍只有一条；记忆作为独立的 `System` 上下文段（实现可以选择并入系统提示，
  但同一内容不得重复出现两次）。
- 历史裁剪必须保持 `assistant(tool_calls)` 与对应 `tool` 结果成对；孤立的结果/调用
  不能进入请求。
- `contextUsage` 的 `used` 来自模型 usage 累计（与 oracle 观察一致），窗口来自
  `context_window`；未知窗口不伪造百分比。

### 2.3 裁剪与阈值

- 输入预算 `input_budget = context_window - max_output_tokens`；两者任一缺失时
  不自动 compact，只记录 `contextUsage` 的未知窗口。
- 保留策略：系统提示 + 最新 `N` 条消息 + 最近的 compact 摘要；从最旧的非系统消息
  开始丢弃，工具对成对丢弃。
- 触发自动 compact 的阈值由 T0 观察决定（比例/绝对值必须记录在契约文档），实现只
  使用观察到的规则；没有观察前不启用自动 compact。

### 2.4 T1 实现注记（2026-11-17）

- `ContextBudget::input_budget()` 只在两个能力都已知时返回
  `Some(context_window.saturating_sub(max_output_tokens))`；缺失任一为 `None`，
  饱和到 0，不产生负数。
- `trim_messages` 是纯函数（不修改输入、无随机性），供 T2 消费：
  - `assistant(tool_calls)` 与紧随其后的 `tool` 结果是一个原子组，缺结果、多余结果
    和孤立结果整体不进请求；
  - 从最新组向最旧保留，预算装不下更旧的组即停止；最新组始终保留（避免当前输入被
    静默丢弃）；
  - `compact_needed = 原始合法历史估算 token > 输入预算`；预算未知时恒 `false`；
  - 估算口径是本地启发式（4 字符 ≈ 1 token，向上取整），不冒充 provider usage。
  - T1 **不**在 `build_model_request` 中自动裁剪；自动 compact 仍未观察到。
- 记忆槽：`ContextInput.memory: Option<&MemorySection>`。记忆正文追加到唯一的系统
  消息末尾（系统提示之后、历史之前），`None`/空白与 C1 v0 逐字节一致；T3 负责读取
  并构造内容，T1 不读记忆目录。
- `runtime.contextUsage`（快照）：
  - 键为 `cache, cost, size, used`；**没有** `percent`，T1 也**不**产出 `breakdown`
    （chars 分解与 settings 对齐归 T6）；
  - `used` 是最近一次模型请求的 total tokens（不是 turn 累计，也不是投影的
    `totalTokenCount`）；
  - `size` 取 `context_window`；未知为 0，不编默认窗口；
  - `cache` 在至少有一次请求后出现：`inputTokens`/`cacheReadTokens`/
    `cacheWriteTokens` 取最近请求，`hitRateRequestCount` 与三个 `total*` 按请求累计；
    `cost` 恒为 `null`（provider 未上报费用）；
  - resume 时从已持久化 assistant 消息的 `tokens` 重建（每条带 `tokens.total` 的
    assistant 行 = 一次完成的请求；oracle 合成的 timeline 消息没有 `total`，不计入），
    投影计数器仍归 0；与 T0 的 close→resume→read 观察一致。

## 3. Compact

### 3.1 方法契约

- `session/compact`：参数 `{sessionId, inputId?, instructions?, expectedRevision?}`；
  结果 `{response, snapshot, compact?: {state: "accepted"|"already_running", inputId?, operationId?}}`
  （复用 `agent_protocol::session_write` 的严格类型）。
- 同一会话同一时刻只允许一个 compact；并发第二个返回 `already_running`。
- `expectedRevision` 冲突 → 显式错误，不启动 compact。
- compact 是**单所有者**操作：由 per-session worker 串行执行，不新建第二条写入路径。

### 3.2 行为

- 摘要由一次模型请求生成（使用会话当前模型；`instructions` 进入该请求）；
- 摘要落库：`message`（`kind`/`semantic` 以 T0 观察为准）+ `part`（`compact` 类型）
  - `session_entry`；`session.time_compacting` 按 oracle 语义更新；
- 事件：按 T0 观察产出（至少包含 compact 开始/结束/失败的可观察信号；未观察到的
  事件不伪造）；
- compact 成功后，后续 turn 的历史包含摘要 + 摘要之后的消息；之前的完整历史仍在 DB，
  但不再进入模型请求（除非用户显式回退，回退属 C3）；
- compact 失败不破坏会话：保留原历史，返回显式错误，turn 可继续；
- 自动 compact 与手动 compact 共用同一执行路径与幂等键，不产生两份摘要。

## 4. Provider 扩展

### 4.1 Anthropic Messages

- `POST {base_url}/v1/messages`，`stream:true`；请求映射：system、messages
  （user/assistant/tool_result）、tools（`input_schema`）、`max_tokens`、thinking
  （若模型支持）、headers（x-api-key / anthropic-version）。
- 流事件映射：`message_start`（usage）→ `content_block_start`（text/thinking/tool_use）
  → `content_block_delta`（text_delta/input_json_delta/thinking_delta）→
  `content_block_stop` → `message_delta`（stop_reason/usage）→ `message_stop`。
- 工具调用：`tool_use` 的 `id/name/input` 累积；`input_json_delta` 拼接后解析；
  `stop_reason=tool_use` → `"tool-calls"`。
- usage：input/output/cache_read/cache_creation；错误显式（401/403/5xx/流内 error）。
- 不做重试（C3）。

### 4.2 OpenAI Responses

- `POST {base_url}/responses`，`stream:true`；请求映射：`instructions`（系统）、
  `input`（消息/工具结果）、`tools`（function）、`max_output_tokens`。
- 流事件映射：`response.output_text.delta`、`response.function_call_arguments.delta`、
  `response.output_item.added/done`（function_call）、`response.completed`（usage）。
- `response.completed` 的 `status`/`incomplete_details` 映射为 stop reason；工具调用
  参数解析失败 → 显式 `ProviderError`。
- usage：input/output/reasoning/cached tokens（按公开 API 字段）；无重试。

### 4.3 选择

- `ResolvedProvider.api_format` 决定适配器；`openai-chat-completions` 保持 C1 行为。
- 未支持格式继续显式报错（不静默回退到 chat completions）。

## 5. Memory

### 5.1 启用条件

- `memoryEnabled` 来自 `session/requestRuntimePreferences` 的宿主应答（C1 已接默认值）；
  C2 必须使用宿主应答，而不是常量。
- 关闭时完全不读记忆目录、不注入、不写。

### 5.2 召回（只读）

- 数据面：`{dataRoot}/cli/memories/projects/<workspaceId>/memory/<file>`，沿用
  `project-memory` crate 的安全规则（普通目录/文件、无 symlink、5 MiB、索引优先）。
- 注入内容：`MEMORY.md` 索引 + 允许的项目记忆文件；必须标记为「项目记忆（只读）」，
  不得伪装成用户指令。
- 注入失败（目录缺失/读取失败/超限）fail-soft：记录日志/诊断，不阻断 turn；
  不把错误写进模型上下文。
- `workspaceId` 解析遵循 `workspaceIdentity?.trim() || workspacePath` 的同一 key 规则；
  远程工作区的数据根归属由 Host 决定，C2 只使用传入的 data root。

### 5.3 写入边界

- T0 必须先观察 oracle 是否存在「agent 写记忆」的路径（工具/方法/模型提取）。
- 若观察到明确写入路径：按观察实现，写失败 fail-soft，不污染 turn；
- 若无写入路径：C2 只做召回，把「写记忆」登记为 C4/C2.1 缺口，**不发明** memory 工具。

## 6. Fresh-install 迁移

- `SessionStore::open` 在**空数据根**（无 `db.sqlite` 或无账本）时，C2 必须能以
  Rust 创建与 oracle 等价的 schema + 迁移账本；C1 的 fail-closed 行为改为
  「可创建时创建，不可创建时显式报错」。
- 迁移 id/checksum 必须与 oracle 创建的库逐字节一致（T0 从真实库读出 ledger 行作为
  数据契约）；只增不改；`pnpm check:sqlite-schema` 与基线一致。
- 已安装旧库（C1 读到的形式）保持可读；迁移 runner 不重写已有账本。
- 并发：两个进程同时首次启动时，SQLite 事务/`BEGIN IMMEDIATE` 保证只有一个创建；
  失败方重读账本，不重复写入。

## 7. 快照与读面

- `session/read`/`list` 的 `settings`、`projection`、`runtime`、`contextUsage` 按
  `agent-server-c1-contract.md` 与 T0 观察补齐；`runtime.contextUsage` 的 used/window
  来自真实 usage/能力。
- `settings.model.available` 由 provider 目录投影（当前模型 + 可选模型）；未知字段
  不伪造。
- 未产出的 `session.updated`/`streamRecovery.updated` 仍在差异表登记；C2 不新增
  未观察事件。

## 8. 所有权与失败语义

| 场景                              | 行为                                                      |
| --------------------------------- | --------------------------------------------------------- |
| 上下文超预算且自动 compact 未启用 | 按裁剪规则丢最旧消息；记录 contextUsage；不静默截断工具对 |
| compact 并发                      | 第二个 `already_running`，不产生第二份摘要                |
| compact 模型失败                  | 保留原历史；显式错误；会话可继续                          |
| 记忆读取失败                      | fail-soft，不注入；turn 继续                              |
| Anthropic/Responses 流中断        | 已发 delta 保留；`turn.failed` 一次；C3 再补恢复          |
| 空数据根迁移并发                  | 一个创建、另一个重读；无半截 schema                       |
| 未支持 apiFormat                  | 显式 `UnsupportedApiFormat`，不回落                       |

## 9. 验收

1. T0 契约文档与夹具可复现（compact/memory/contextUsage/fresh-install ledger）。
2. compact：手动 + 自动（若观察支持）事件/DB/恢复一致；并发/失败/冲突测试通过。
3. memory：`memoryEnabled` 开关、注入内容、fail-soft、无 symlink/超限测试通过。
4. Anthropic/Responses：本地 SSE 夹具 + 真实端点 smoke（opt-in）通过；工具调用/usage/
   错误映射有测试。
5. 空数据根 fresh install：schema/ledger 与 oracle 逐字节一致；`check:sqlite-schema` 通过。
6. 快照字段与 oracle 观察一致；未补齐项登记在 `tauri-agent-runtime.md` §C1/C2 差异表。
7. 回放 38/38 保持；新增 C2 夹具全绿；`cargo test`/clippy/typecheck/lint/architecture 通过。
8. 未执行项（真实端点、真机）如实报告。

## 10. Clean-room

- 不读 `apps/mode-cli/**`、`packages/services/**`、`packages/client/**` 的实现体；
  只读协议类型声明、测试断言、specs、公开 API 文档与黑盒观察。
- 迁移 ledger/checksum、compact/memory 事件形状只从真实库/事件录制获取；不手写期望值。
- 提示词与记忆注入文案由本仓库编写；出现非接口约定所致逐字雷同则丢弃重写。
