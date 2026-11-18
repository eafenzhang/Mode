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
  **自撰**提示词（正文不与 oracle 相同，保留结构性片段）。
- `session_store::TargetRecord` + `get_target/upsert_target/delete_target`：
  `session_target` 单行读写，`clear` 删行。
- `session/goal`：六 action、严格参数、`-32009`/`-32010`、`startedTurn`；`pause`
  立即置 paused 并取消 pending/in-flight 续跑；`session/stop` 同时落地。
- worker：`set/replace` 控制 turn → `summary_updated`/`set` → 续跑循环；`resume`
  直接续跑；每轮 `run_started` → turn → `run_finished` → verifier；无轮次/预算上限。
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
