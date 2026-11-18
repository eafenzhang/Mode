# Spec：C3 流恢复、目标续跑与故障收口（LT-1 C3）

状态：**方向已定（2026-11-18，用户「继续 C3」）**；实现前由 C3-T0 补齐黑盒观察。
关联：`docs/specs/agent-core-parity.md`（LT-1 总 spec §5.8）、`docs/specs/mode-api-retry.md`、
`docs/specs/tauri-agent-runtime.md`（对拍/白名单）、`docs/specs/agent-c2-context-memory.md`、
`docs/specs/project-memory-service.md`、`docs/specs/clean-room-policy.md`。

## 1. 目标与范围

C3 在 C1/C2 的 Rust `agent-server` 上补齐三件事，并保持「终态恰好一次」：

1. **流恢复（stream recovery）**：模型流中断/重试时的锚点、续传语义与
   `streamRecovery.updated` 事件；
2. **目标续跑（goal/target）**：`session/goal` 的 show/set/replace/pause/resume/clear，
   续跑 turn、停止条件与 `session_target` 单一事实源；
3. **故障收口与后台通知**：工具/provider/超时/取消的终态各恰好一次；后台任务完成时
   空闲开新 turn、忙时按 oracle 行为丢弃；子代理**只读基础面**（列表/结果消费标记，
   不实现子代理运行时）。

**不在 C3**：MCP/Skills/Plugins/Workflows/附件/v4 会话帧（C4）；子代理/工作流的实际
执行（C4）；真实端点 smoke（人工凭据，C2 已留脚本）；官方平台/遥测（永久下线）。

## 2. 流恢复与重试

### 2.1 事实来源

- `streamRecovery.updated` 的 payload 形状、发出时机与去重规则以 C1/C2 夹具与 C3-T0
  观察为准；未观察到的字段不伪造。
- `mode-api-retry` crate 已是重试状态的唯一归一化/投影实现（`SessionApiRetryTracker`、
  `DeferredDraftRegistry`）；C3 只消费其公开 API，不复制规则。
- oracle 的重试次数/退避/错误分类由 T0 观察（mock provider 注入 5xx/流中断）；未观察到
  的策略不启用，登记为差异。

### 2.2 语义

- 一次模型请求失败后：retryable（网络/5xx/流中断）→ 按观察到的次数与退避重试；
  非 retryable（401/403/参数错误）→ 直接终态，不重试。
- 重试期间：
  - 已提交的 delta/part 不重复发送；重试从最后锚点继续（或按观察到的重放方式）；
  - `streamRecovery.updated` 携带锚点/apiRetry 状态；状态由 `mode-api-retry` 投影；
  - 恢复进展（text/reasoning/tool delta）按 `mode-api-retry` 的规则清除重试态。
- 重试耗尽：`turn.failed` 恰好一次，错误可诊断（状态码/类型/尝试次数）。
- `desktop-continuous` 的实时流与 `web-remote-replayable` 的 gap 恢复语义分开：
  Rust 运行时只产生有序事件与锚点；快照/队列仍归 Host/relay（DR-T5-1）。

### 2.3 终态与幂等

- `turn.completed` / `turn.failed` 每个 turn 恰好一次；重试不产生第二个终态。
- 同一 `inputId`/`queryId` 的重试幂等；stale run 拒绝。
- 取消优先于重试：取消后不再发起新尝试，终态为 cancelled。

### 2.4 Rust T2 现状（2026-11-18）

- `ProviderError` 带 `kind`/`status`/`retryable`/`underlying_message`；
  `provider-http` 负责 HTTP 状态（401/403 auth、5xx server、408 transport、
  429 rate limited）、传输失败与流中断分类；适配器不内嵌重试。OpenAI Chat 在
  `[DONE]`/`finish_reason` 前断流也归 `StreamAbort`。
- 重试循环在 `agent-core::Session::run_turn`：未提交 delta 的 retryable 失败走
  指数退避 + `session.updated(model_retry_scheduled)`；已提交 delta 的流中断走
  4 条 `streamRecovery.updated`（`retryDelayMs:0`）并立即重试。次数用夹具的
  `maxRetries:10` / `maxAttempts:11`；耗尽形状未观察，只保证 `turn.failed` 一次。
- 事件/终态用规范文案（auth/server/network），适配器原文只进
  `underlyingErrorMessage`；agent-server 补齐 `providerId`/`modelId`/session/turn/
  query/modelCall 等宿主事实（未接线的 providerKind/transport/baseURL/header 统计
  登记在契约 §2.7）。
- 取消优先：退避与取消竞争，取消后不再发起尝试；退避取消补
  `model_request_failed {reason:"cancelled", retryable:false}`，终态
  `turn.completed(cancelled)` 一次（无 `usage`、`historyRoundCount:0`）；DB
  `model_request_count` 用内部轮次事实（示例 1）。
- 账务分层：事件 `modelRequestCount` 只计成功请求；DB
  `turn_usage.model_request_count` / `historyRoundCount` 计轮次（`is_recovery_attempt`
  的恢复尝试 +1；HTTP adapter 重试不增，包括恢复周期内的后续 adapter 尝试）；
  `turn_usage.model_retry_count` 只计 adapter 重试。
- apiRetry 由 `mode-api-retry::SessionApiRetryTracker` 投影（agent-server 喂事件、
  快照用 `with_api_retry_runtime`）；恢复进展 delta 通过 crate 新增的
  `track_model_streaming_progress` 清空，成功/取消/错误也清空。
- 401 终态与夹具一致（`turnPhase:"processing_input"`、规范 message、
  `underlyingErrorMessage`、无顶层 retryable、`finish:null` 占位）。
- 已知差异逐项登记在 `agent-c3-contract.md` §2.7（`model_request_*` 事件族仍缺、
  退避毫秒不逐一对拍、工具流恢复/连续中断/耗尽形状未观察、普通 turn 不写
  `model_usage`、compact/verifier 未接重试等）。

## 3. 目标续跑（Goal / Target）

### 3.1 方法契约

- `session/goal`：参数 `{sessionId, inputId?, action, objective?, expectedRevision?}`；
  结果 `{response, snapshot, startedTurn?}`（复用 `agent_protocol::session_write` 的严格类型）。
- action：`show`（只读）、`set`（创建目标）、`replace`（替换目标）、`pause`、`resume`、
  `clear`；`objective` 只对 set/replace 等需要目标的动作必填。
- `expectedRevision` 冲突 → 显式错误，不改变状态、不启动 turn。

### 3.2 状态与事件

- `session_target` 是目标的唯一事实源（objective/status/summary_title/token_budget/
  tokens_used/time_used_seconds 等字段）；`session/read` 的 `session.target` 从它投影。
- 目标续跑：set/replace/resume 后按 oracle 观察决定是否自动开 turn（`startedTurn`）；
  pause/clear 停止续跑；用户中断、达到预算/轮次上限、目标完成时停止。
- 事件：`session.updated`/`goal` 相关事件按 T0 观察产出；未观察到的字段/事件不伪造。
- 续跑 turn 与普通 turn 共用同一 worker/CommandInbox；不新增第二队列。

### 3.3 停止条件

- pause/clear/用户取消/目标完成/预算耗尽/最大轮次（以观察为准）；每个条件有测试；
- 重启/冷恢复后 `session_target` 可继续（状态从 DB 重建，不重放已消费 turn）。

### 3.4 Rust T1 现状（2026-11-18）

- `agent-core::goal`：目标状态/响应文案、续跑停止判定、verifier 响应解析与两条
  **从头自撰**提示词（仅保留 `<untrusted_objective>`/Budget/JSON 等接口结构；
  与夹具最长公共子串只命中这些结构片段）。
- `session_store::TargetRecord` + `get_target/upsert_target/delete_target`：
  `session_target` 单行读写，`clear` 删行。
- `session/goal`：六 action、严格参数、`-32009`/`-32010`、`startedTurn`；`pause`
  立即置 paused 并取消 pending/in-flight 续跑；`session/stop` 同时落地。
- 方法层：`set/replace` 同步产出 title/控制 turn/`summary_updated`/`set` 后再返回
  应答快照；pause/resume/clear 的 status/cleared 事件也同步进入快照。worker 只跑
  续跑循环；每轮 `run_started` → turn → `run_finished` → verifier；无轮次/预算上限。
- 序号：continuation turn 编号沿用控制 turn 基数；`foregroundExecutionId` 是会话级
  continuation ordinal；verifier `goalIteration` 只对到达 verifier 的尝试递增。
- 账务：tokens/time 只在 `run_finished` 累计（取消不累计 tokens）；
  verifier 独立 `model_usage(query_source:"target_completion_verification")`。
- 冷恢复：`active` → `paused` + 清 active 字段，tokens/time/验证时间线保留，
  无 idle turn；`session/resume` 带 `goalStats`。
- 已知差异逐项登记在 `agent-c3-contract.md` §1.11（提示词正文、C1 未产出的
  `session.updated` 子事件、控制 turn 消息行、verifier 请求分段、pre-request 计数边界、
  `eventSeq` 物化差异、失败路径等）。

## 4. 后台通知与故障收口

- 后台任务完成 → 空闲时开新 turn（`<task-notification>` 文本按 C1 观察）；
  忙时按 oracle 行为（C1 观察：直接丢弃，不排队）；不得偷偷改成排队。
- 工具错误、provider 错误、超时、取消：每个 turn 终态恰好一次；已提交 part 保留；
  不留下半截消息（C1 规则沿用）。
- 后台任务与 goal 的交互（如 goal 续跑中后台任务完成）以 T0 观察为准；未观察则登记。

### 4.1 Rust T3 现状（2026-11-18）

- **准入判据分相位**：后台通知在 `TaskRegistry` 完成时，由运行时看「前台工具执行
  窗口」（`tool.updated started` → `batch`）决定丢弃或入队；窗口外（含模型流式等待）
  的通知留在注册表既有通道。worker 在 turn 结束/空闲时一次取走全部通知，只开
  **一个**合并后的 `background_task` turn（多条 `<task-notification>` 之间空行分隔）。
  空闲 worker 由注册表完成信号唤醒，不新增第二队列/定时器。
- **goal 交互**：续跑 turn 成功后、completion verifier 之前投递 pending 通知；
  通知 turn 的 `targetId:null`，但包在目标 `run_started`/`run_finished` 中并计入
  target 的 tokens/time（夹具 §3.3 的 52+22=74）。
- **终态**：通知 turn 与普通 turn 共用终态收口（成功/取消/provider 失败各恰好一次）；
  工具/provider/超时/取消不写半截 assistant 行。取消且已观察到模型请求时补
  `finish:null`、无 part 的 assistant 占位（`retry-cancel-during-backoff` /
  `goal-pause-in-flight`），请求前取消不补。
- **apiRetry**：任何 `turn.completed` / `turn.failed` 提交时清空
  `SessionApiRetryTracker`（goal 续跑失败不再把重试态泄漏到随后快照）。
- 已知差异逐项登记在 `agent-c3-contract.md` §3.5：任务状态 `session.updated` 事件
  未产出、前台工具窗口丢弃是 C1 语义的近似、`originMeta` 取值折叠、通知 turn 的
  revision/取消/失败/verifier 期间完成等未观察组合。

## 5. 子代理基础面

- 只读：`session/subagents` 列表（running/ended，形状按 `agent-protocol::session_read`
  镜像）从会话/任务库投影；`background_subagent_result_consumed` 等标记按观察实现。
- **不实现**子代理运行时（spawn/child runtime/coordinator）——归 C4；不发明子代理工具。
- 失败/缺失数据 fail-soft（列表为空/字段缺省），不影响主 turn。

## 6. 所有权与失败语义

| 场景               | 行为                                                           |
| ------------------ | -------------------------------------------------------------- |
| retryable 错误     | 按观察次数/退避重试；每次尝试可观察；耗尽后 `turn.failed` 一次 |
| non-retryable      | 立即 `turn.failed`，不重试                                     |
| 取消中             | 不再尝试；终态 cancelled 一次                                  |
| goal revision 冲突 | 显式错误，零事件/零 DB 写                                      |
| goal 预算/轮次耗尽 | 停止续跑，目标状态与 `session_target` 一致                     |
| 后台任务忙时完成   | 丢弃（oracle 观察），登记；不排队                              |
| 子代理数据缺失     | 空列表/fail-soft，不阻断                                       |

## 7. 验收

1. T0 契约文档 + 夹具（goal 全 action、stream recovery/retry、后台通知、subagents 列表）。
2. goal：全 action 的事件/DB/快照；续跑与停止条件；revision 冲突零副作用；重启恢复。
3. 流恢复/重试：注入 5xx/流中断，断言尝试次数/退避/事件/终态一次；非 retryable 不重试；
   取消优先级；无重复 delta。
4. 后台通知：空闲开 turn / 忙时丢弃；终态一次。
5. `session/subagents` 只读列表形状与缺失 fail-soft。
6. 回放：C1 38/38 + C2 9/9 保持；C3 新夹具全绿；`cargo test`/clippy/typecheck/lint/
   architecture/check:sqlite-schema 通过；未执行项如实报告。

## 8. Clean-room

- 不读 `apps/mode-cli/**`、`packages/services/**`、`packages/client/**` 实现体；只读
  协议类型、测试断言、specs、公开资料与黑盒观察。
- 重试/恢复/目标事件形状只从真实录制获取；不手写期望值；不复制上游提示词。
