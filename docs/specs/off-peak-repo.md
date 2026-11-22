# Spec：闲时任务仓库（`crates/off-peak-repo`）

状态：2026-10-29 实现（W3-3 任务域：`OffPeakTaskRepo` 全量 Rust 化）。
行为真源（clean-room：只读本仓库 TS）：

- `packages/services/src/session/offPeakTaskRepo.ts`
- `packages/shared/src/off-peak-types.ts`
- `packages/shared/src/model-selection.ts`（`modelSelectionSchema`）
- `packages/shared/src/task-realtime-core.ts`（`resolveWorkspaceKey`）

数据库 schema 与冻结迁移由 `crates/tasks-db` 提供；本 crate 不复制 DDL。

## 范围

| Rust 方法 | 对应 TS | 关键语义 |
| --- | --- | --- |
| `ensure_ready` | `ensureReady` / `initialize` | 建目录、打开连接、`busy_timeout` / `WAL` / `synchronous=NORMAL`（**不设** `foreign_keys`，与 TS 一致）；未 prepared 时跑迁移并把遗留 `awaiting_approval` 改回 `running` |
| `close` | `close` | 释放连接；`throwOnError` 语义不提供 |
| `create` | `create` | 建行即 `queued`；id 缺省 `offpeak-<uuid v4>`；`schedulable` 缺省 0；`model_selection` 严格编码后落库 |
| `list` | `list` | 有 `tasks` 表时 LEFT JOIN 取绑定会话标题；空标题不输出 `sessionTitle`；`ORDER BY created_at DESC` |
| `get` | `get` | 单行读取，不带联查标题 |
| `invalidate_model_selection` | `invalidateModelSelection` | 事务内比较；另一进程已修复则原样返回；否则清 `model_selection`、撤销 `schedulable`、保留旧 `model/thought_level` |
| `delete` | `delete` | 任何状态可删 |
| `mark_history_deleted` | `markHistoryDeleted` | 未启动或已隐藏时幂等返回；只写 `history_deleted_at` |
| `count_non_terminal` / `count_active` | 同名 | 非终态计数 / `running` 计数 |
| `has_active_bound_task` | `hasActiveBoundTask` | `(workspace_key, session_id)` 的非终态绑定 |
| `update_editable_fields` | `updateEditableFields` | 仅 `queued`/`paused`；`Clear`（TS `null`）直接拒绝；缺有效 Selection 抛错；只改 `model_selection`，不动旧列 |
| `update_scheduling_snapshot` | `updateSchedulingSnapshot` | 仅覆盖显式传入字段；`server_ticket_id`/`registered_at` 用 `??` 语义回退 |
| `claim_due` | `claimDue` | 先回收超时认领（`OFF_PEAK_CLAIM_STALE_MS = 10min`），再按 `queued_at, created_at` FIFO 逐条 `claim_running 0→1`；无有效 Selection 的行跳过 |
| `mark_running` | `markRunning` | 仅 `queued→running`；`COALESCE` 保留首跑字段，`started_at` 只在首段写入 |
| `mark_terminal` | `markTerminal` | 终态不可逆出；`files_changed` 保留旧值；带 `dispatch_error` 时 `attempt_count + 1` 并记 `last_error` |
| `set_paused` | `setPaused` | `queued ⇄ paused`；`claim_running=1` 时拒绝 |
| `release_claim` | `releaseClaim` | 仅对在途认领生效；带 error 累计尝试次数 |
| `recover_interrupted` | `recoverInterrupted` | 单事务：残留 `running → queued` + 清理超时认领，返回回收条数 |
| `requeue_for_continuation` | `requeueForContinuation` | 仅 `running→queued`，保留会话与 `started_at` |
| `list_non_terminal` | `listNonTerminal` | 按 `queued_at` 升序 |
| `mark_settled` | `markSettled` | 仅终态行；重复核销覆盖为最新 ack 时间 |
| `list_unsettled_terminal` | `listUnsettledTerminal` | 终态且未核销，按 `ended_at` 升序 |

## `ModelSelection` 编码

- 解析（读库）与 `modelSelectionSchema` 同构：顶层与 `options` 都拒绝未知键；
  `providerId` / `modelId` / `options.reasoningLevel` 必须 `trim()` 后非空；
  `options` 出现 `null` 或非对象视为无效。无效时输出 `modelSelectionIssue:
  { code: "repair-required" }`，且该行不参与认领。
- 落盘按 `providerId → modelId → options` 的插入序手工拼 JSON；
  `options` 为空对象时整个字段省略（与 `Object.keys(...).length > 0` 一致）。
- 落盘前重新 trim 并校验非空，空值返回错误。

## 输出形状

`OffPeakTask` 字段顺序与 TS 对象字面量一致：
`offPeakTaskId, serverTicketId?, title, conversationId?, sessionId?, sessionTitle?, prompt,
permissionMode, modelSelection?, modelSelectionIssue?, workspaceKey, workspacePath,
workspaceIdentity?, status, queuedAt, startedAt?, endedAt?, failureReason?, filesChanged?,
settledAt?, historyDeletedAt?, registeredAt?, schedulable, queuePosition?, nextPollAt?,
createdAt, updatedAt`。

`schedulable` 恒输出布尔；其余可空字段缺省时整个键省略。

## 初始化与进程内标记

`ensure_ready` 首次完成迁移与 `awaiting_approval` 修复后写入 crate 内的 prepared
注册表（等价 TS `initializePromise`）。TS 的 prepared/migrated 标记在
task-index / automation / off-peak 三个代表间共享；Rust 目前按 crate 独立。
差异只影响重复初始化的工作量：修复语句本身幂等，最终数据一致。

`awaiting_approval` 修复使用注入时钟（默认 `SystemTime`），与 TS 的 `Date.now()` 对齐。

## 与 TS 的差异

| 差异 | 影响 |
| --- | --- |
| prepared 标记按 crate 独立 | 首次初始化可能多跑一次幂等修复语句 |
| `create` 的默认主键由 `uuid v4` 生成 | 与 `randomUUID()` 同样不可跨进程比较；fixture 一律显式传 id |
| 错误文案 | 空 Selection / 缺行的错误为 Rust 自定义文本（TS 为 ZodError 或仓库内文案） |
| `close({throwOnError})` | Rust 返回 `()`，不区分该选项 |

## 验收

- `pnpm off-peak:fixtures` 用真实 TS `OffPeakTaskRepo` 生成 **50 步**黄金数据
  （含联查标题、legacy 无效 Selection、认领与回收、暂停/续跑、核销、删除与重开）。
- `tests/parity.rs`：逐步重放并比较方法返回 JSON 与整表快照（时间列不归一，
  因为所有写入口都显式传 `now`）。
- `tests/repo.rs`：注入时钟的 `awaiting_approval` 修复、默认主键前缀、
  超时认领回收、终态不可逆出。
- `cargo test -p off-peak-repo`、`cargo clippy -p off-peak-repo --all-targets -- -D warnings`。

## 未纳入

- `AutomationRepo`（`automations` / `automation_runs` 表与 cron 语义）仍由 TS 承担，
  是任务域剩余的最大一块。
