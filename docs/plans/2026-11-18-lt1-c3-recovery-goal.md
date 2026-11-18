# LT-1 C3：流恢复、目标续跑与故障收口 Implementation Plan

> **For agentic workers:** 本文件是 C3 检查点的长任务计划。每个 Task 开工前按
> `writing-plans` 补齐该 Task 的逐步骤细节；步骤用 `- [ ]` 跟踪。
>
> 前置：C1 + C2 已合并到 `main`（merge `de7ec00e2` / `468da1573`）。C3 在
> `feat/lt1-c3-recovery-goal` 上开发。

**Goal:** 在 Rust `agent-server` 上补齐流恢复/重试、goal/target 续跑、后台通知与
终态收口，并交付子代理只读基础面；保持 C1/C2 回放不回退。

**Architecture:** `agent-core` 负责 turn 内重试/恢复与终态；`agent-server` 负责
`session/goal`、`session/subagents`、后台通知与持久化；`mode-api-retry` 是重试状态的
唯一投影；`session_target` 是目标唯一事实源。

**Tech Stack:** Rust 1.99、tokio、serde/serde_json、rusqlite、`mode-api-retry`、
`agent-protocol`、`session-store`。

**Spec:** `docs/specs/agent-c3-recovery-goal.md`；总 spec `docs/specs/agent-core-parity.md`。

## Global Constraints

- clean-room：不读 `apps/mode-cli/**`、`packages/services/**`、`packages/client/**`
  实现体；只读协议类型、测试断言、specs、公开资料与黑盒观察。
- 观察优先：goal/stream recovery/retry/后台通知/subagents 的形状只来自 T0 录制；
  未观察到的字段/策略不伪造，登记为差异。
- 终态恰好一次：retry/取消/失败不产生第二个终态；已提交 part 不重复。
- 单一所有者：`session_target` 是目标事实源；重试状态由 `mode-api-retry` 投影；
  后台通知不偷偷排队（按 oracle）。
- 回放不回退：C1 38/38 + C2 9/9 保持；C3 新夹具另行计数。
- 门禁：`cargo test`（touch crates）、`cargo clippy --all-targets -- -D warnings`、
  `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`、`pnpm check:sqlite-schema`。
- 不推送：agent 只做本地提交。

## Task 总览

| ID  | 任务               | 规模 | 依赖                   | 主要落点                                                          |
| --- | ------------------ | ---- | ---------------------- | ----------------------------------------------------------------- |
| T0  | C3 黑盒契约捕获    | M    | 无                     | `docs/specs/agent-c3-contract.md`、probes/fixtures                |
| T1  | Goal/target 续跑   | L    | T0                     | `agent-core` goal、`methods/goal.rs`、`session_target`            |
| T2  | 流恢复与重试       | L    | T0、T1（同改 runtime） | `agent-core` recovery、`mode-api-retry`、`provider-http` 错误分类 |
| T3  | 后台通知与终态收口 | M    | T0、T2                 | `runtime.rs`、`agent-tools::TaskRegistry`                         |
| T4  | 子代理只读基础面   | M    | T0                     | `methods/subagents.rs`、`session_task_link`                       |
| T5  | 对拍、smoke 与文档 | M    | 全部                   | replay harness、`tauri-agent-runtime.md` §C3                      |

**波次**：Wave 0 = T0；Wave 1 = T1；Wave 2 = T2；Wave 3 = T3 + T4（T3/T4 不同文件，
但 T3 改 runtime.rs，T4 读面为主，顺序 T3→T4 更稳）；Wave 4 = T5。

---

### T0: C3 黑盒契约捕获

**Files:**

- Create: `docs/specs/agent-c3-contract.md`
- Create: `packages/desktop/tests/agent-runtime-oracle/fixtures/c3/*.json`
- Create: `packages/desktop/tests/agent-runtime-oracle/probe-c3-*.mts`

**观察项（隔离数据根，真实 oracle，机器录制）**：

1. **goal**：`session/goal` 六个 action 的参数/应答（`response`/`snapshot`/`startedTurn`）、
   事件序列、`session_target` 行、续跑 turn（自动/手动）、pause/resume/clear、预算/
   轮次/完成停止条件、`expectedRevision` 冲突。
2. **stream recovery**：从既有 C1/C2 夹具提取 `streamRecovery.updated` 形状；
   用 mock provider 注入流中断/5xx，观察重试次数、退避、事件顺序、`apiRetry` 状态、
   终态；非 retryable（401/403）行为。
3. **后台通知**：后台 Bash 完成时空闲/忙时的 turn 行为（C1 观察为忙时丢弃，复核）。
4. **subagents 只读**：`session/subagents` 的 running/ended 形状、字段、边界（无数据）。
5. **goal 与后台通知/重试的交互**：至少观察一种组合（如 goal 续跑中任务完成）。
6. 未观察到/无法触发的项显式登记。

**验收**：契约文档列「观察值 / 复现命令 / 未观察」；fixtures 机器录制；隔离断言；
C1/C2 夹具与探针不被破坏；提交仅新增文档/探针/夹具。

---

### T1: Goal/target 续跑

**Files:**

- Create: `crates/agent-core/src/goal.rs`（目标状态/续跑判定/停止条件）
- Create: `crates/agent-server/src/methods/goal.rs`
- Modify: `crates/agent-server/src/{dispatch,runtime}.rs`、`session-store`（`session_target` 读写）
- Test: `crates/agent-server/tests/goal.rs`

**行为**：

- `session/goal` 严格参数/结果；`show` 只读；set/replace/pause/resume/clear 语义按 T0；
- `expectedRevision` 冲突零副作用；
- `session_target` 唯一事实源；`session/read` 的 `target` 从它投影；
- 续跑 turn 与普通 turn 共用 worker/CommandInbox；停止条件按观察；
- 重启/冷恢复后状态从 DB 重建，不重放已消费 turn。

**验收**：全 action 事件/DB/快照；续跑与停止条件；冲突；重启恢复；夹具形状可复现。

---

### T2: 流恢复与重试

**Files:**

- Modify: `crates/agent-core/src/{session,stream,provider}.rs`
- Create/modify: `crates/agent-core/src/recovery.rs`（若需要）
- Modify: `crates/provider-http/src/error.rs` / adapters（retryable 分类）
- Modify: `crates/agent-server/src/runtime.rs`（apiRetry 投影）
- Consume: `crates/mode-api-retry`（不复制规则）
- Test: `crates/agent-core/tests/recovery.rs`、`crates/agent-server/tests/retry.rs`

**行为**：

- retryable（网络/5xx/流中断）按 T0 观察的次数/退避重试；non-retryable 立即终态；
- 重试期间已提交 delta 不重复；`streamRecovery.updated` 按夹具形状产出；
- `apiRetry` 状态由 `mode-api-retry` 投影；恢复进展清空规则同 crate；
- 取消优先于重试；重试耗尽 `turn.failed` 恰好一次；
- desktop-continuous 只发有序事件；web-remote-replayable 的 gap 恢复仍归 Host。

**验收**：5xx/流中断/401 注入的尝试次数、退避、事件、终态一次；取消优先级；
无重复 delta；C1/C2 回放保持。

---

### T3: 后台通知与终态收口

**Files:**

- Modify: `crates/agent-core/src/session.rs`、`crates/agent-tools/src/task.rs`
- Modify: `crates/agent-server/src/runtime.rs`
- Test: `crates/agent-server/tests/background.rs`

**行为**：

- 后台完成空闲 → 开新 turn（`<task-notification>` 文本按 C1 观察）；忙时按 oracle 丢弃；
- 工具/provider/超时/取消终态各恰好一次；已提交 part 保留；
- goal 续跑/重试中后台完成的交互按 T0 观察；
- 不新增队列/定时器掩盖时序。

**验收**：空闲/忙、终态一次、与 goal/retry 交互；无半截消息。

---

### T4: 子代理只读基础面

**Files:**

- Create: `crates/agent-server/src/methods/subagents.rs`
- Modify: `dispatch.rs`、`session-store`（读取 `session_task_link`/子会话元数据）
- Test: `crates/agent-server/tests/subagents.rs`

**行为**：

- `session/subagents` running/ended 列表按协议镜像；字段/边界按 T0；
- `background_subagent_result_consumed` 等标记按观察；
- 无数据/损坏 fail-soft 空列表；不实现 spawn/child runtime（C4 登记）。

**验收**：形状/字段/边界；缺失 fail-soft；不新增执行路径。

---

### T5: 对拍、smoke 与文档

**Files:**

- Modify: `packages/desktop/tests/agent-runtime-oracle/replay-c3*.mts`（或扩展 replay-c2）
- Modify: `docs/specs/tauri-agent-runtime.md`（§C3 差异）
- Modify: `docs/specs/agent-core-parity.md`（C3 勾选）

**内容**：C3 夹具回放（goal/retry/background/subagents）；C1 38/38 + C2 9/9 保持；
真实端点 smoke（复用 agent-c2-smoke，补 goal/retry 场景，opt-in）；差异表；门禁。

**验收**：C3 夹具全绿 + 旧回放保持；smoke 有人执行并记录；差异与代码一致；门禁真实通过。

---

## 执行顺序

```text
T0 → T1 → T2 → T3 → T4 → T5
```

T1/T2/T3 都改 `runtime.rs`，串行；T4 在 T3 后接读面；T5 最后。

## 完成定义

- 所有 Task 勾选；C3 spec §7 验收记录。
- C1 38/38 + C2 9/9 保持；C3 新夹具全绿。
- goal/retry/background/subagents 测试通过；终态恰好一次。
- 门禁真实通过；未观察/未支持项登记。
