# Spec：定时任务仓库（`crates/automation-repo`）

状态：2026-10-29 实现（W3-3 任务域：`AutomationRepo` 全量 Rust 化，37 个公开方法）。
行为真源（clean-room：只读本仓库 TS）：

- `packages/services/src/session/automationRepo.ts`
- `packages/shared/src/automation-types.ts`
- `packages/shared/src/bots.ts`（`modeAutomationBotDeliveryTargetSchema`）
- `packages/shared/src/model-selection.ts`
- `packages/shared/src/mode-task-types-core.ts`（`modeTaskModeSchema`）
- `packages/shared/src/task-realtime-core.ts`（`resolveWorkspaceKey`）

数据库 schema 与冻结迁移由 `crates/tasks-db` 提供（`automations` 34 列、`automation_runs`
13 列）；本 crate 不复制 DDL。模型选择解析与严格编码复用 `crates/model-selection`，
workspace key 复用 `crates/workspace-identity`。

## 范围

| Rust 方法 | 对应 TS | 关键语义 |
| --- | --- | --- |
| `ensure_ready` | `ensureReady` / `initialize` | 建目录、打开连接、`busy_timeout` / `WAL` / `synchronous=NORMAL`；未 prepared 时跑迁移并把遗留 `awaiting_approval` 改回 `running` |
| `close` | `close` | 释放连接；不提供 `throwOnError` |
| `create` | `create` | 全库 `COUNT(*) >= 20` 时抛 `[AUTOMATION_CREATE_LIMIT_REACHED]`；id 为 `automation-<uuid v4>`；无选择时落库 `"null"`（跟随 workspace）；`location_kind` 恒 `local`；`enabled` 由 `lifecycle_status == "completed"` 反推 |
| `list` | `list` | 按 `created_at DESC`；scope 给 `workspace_path` 时按派生 key 过滤，否则返回全部 workspace |
| `get` | `get` | 单行读取，可选 workspace 过滤 |
| `get_model_selection_for_dispatch` | `getModelSelectionForDispatch` | 行不存在或不属于该 workspace 抛错；有效值直接返回；`"null"` 返回 `None`（跟随 workspace）；SQL NULL 抛「不可用」 |
| `get_bot_delivery_target` | `getBotDeliveryTarget` | 解析失败（非 JSON / provider 非法）返回 `None`，不抛错 |
| `has_task_binding` | `hasTaskBinding` | 同 workspace 下是否存在绑定到该 task 的定义（任一行即可） |
| `get_scheduled_run_count` | `getScheduledRunCount` | 定义不存在时返回 `None`，否则返回计数 |
| `update` | `update` | `None` 不改；`Some(None)` 清空（`model_selection` 落 `"null"`，`mode` 落 SQL NULL）；`reset_retry` 同时重置 attempts / retry_at / dispatch_status；`enabled` 跟随新 `lifecycle_status` |
| `delete` | `delete` | 返回是否真的删到行 |
| `set_enabled` | `setEnabled` | 只改 `enabled` 与 `lifecycle_status`（`active` / `paused`） |
| `restart` | `restart` | 清零 `run_count` / `scheduled_run_count` / attempts / retry / running，回到 `active`，清 `last_error` |
| `run_now` | `runNow` | 事务内：回收超时认领 → 定义 `running 0→1` → 插入 `manual` run（`claimed`）；定义已被占用时返回 `None` |
| `claim_due` | `claimDue` | 见「认领生命周期」 |
| `claim_manual_runs` | `claimManualRuns` | 见「认领生命周期」 |
| `mark_dispatched` | `markDispatched` | 见「派发结果」 |
| `mark_dispatch_failed` | `markDispatchFailed` | 见「派发结果」 |
| `release_claim` | `releaseClaim` | 仅在 `running = 1` 时回 `idle` 并释放认领 |
| `release_manual_claim` | `releaseManualClaim` | 按 `(automation_id, workspace_key)` 释放 running，不动 `dispatch_status` |
| `touch_manual_claim` | `touchManualClaim` | 刷新 `claimed_at` 续租 |
| `skip_and_reschedule` | `skipAndReschedule` | 写入 / 覆盖 `skipped` run；`finalize` 时定义转 `completed` 并停止，否则写 `next_run_at` 并复位派发状态 |
| `ensure_run_claimed` | `ensureRunClaimed` | `ON CONFLICT DO NOTHING`，不覆盖既有 run |
| `upsert_run_claimed` | `upsertRunClaimed` | 冲突时回到 `claimed`、`COALESCE` 保留已有选择、清 outcome / error、attempts + 1 |
| `fix_run_model_selection` | `fixRunModelSelection` | `COALESCE(model_selection, ?)` 原子固定首次选择；读回无效时抛错 |
| `mark_run_dispatch` | `markRunDispatch` | 写状态、`COALESCE` 保留 session、error 整体覆盖（`None` 即清空） |
| `mark_manual_run_dispatched` | `markManualRunDispatched` | 非 `manual` 或已 `dispatched` 返回 `false`；成功时定义 `run_count += 1` 并写 `last_run_at` |
| `mark_run_outcome` | `markRunOutcome` | 迟到的 `running` 结果不能覆盖既有终态 |
| `record_skipped_run` | `recordSkippedRun` | 只写 `dispatch_status = 'skipped'` 与 `error`，不写 `outcome` |
| `list_runs` | `listRuns` | 按 `created_at DESC`，可选 workspace 过滤 |
| `get_run` / `delete_run` | 同名 | 单行读取 / 按 `run_id`（可选 workspace）删除 |
| `prune_runs` | `pruneRuns` | 删除 `created_at < now - max_age_ms`，返回条数 |
| `compute_retry_at` | 内部退避计算 | `now + min(30s × 2^(attempts-1), 15min)` |

## 常量

| 名称 | 值 | 含义 |
| --- | --- | --- |
| `AUTOMATION_CREATE_LIMIT` | 20 | 全库保留上限，所有 workspace 与生命周期一起计数 |
| `AUTOMATION_CREATE_LIMIT_ERROR_CODE` | `AUTOMATION_CREATE_LIMIT_REACHED` | 错误前缀 |
| `DISPATCH_RETRY_BASE_MS` | 30_000 | 首次重试间隔 |
| `DISPATCH_RETRY_CAP_MS` | 900_000 | 退避上限 |
| `DISPATCH_MAX_ATTEMPTS` | 5 | 达到即放弃 |
| `CLAIM_STALE_MS` | 600_000 | 认领超时回收 |

`VALID_MODES = ["yolo", "plan", "edit", "auto", "autoEdit", "build"]`；
`create` 与 `update` 在读取其它字段前校验 mode，非法值直接抛
`Invalid automation mode: <mode>`。
`VALID_BOT_PROVIDERS = ["feishu", "lark", "weixin", "telegram", "wecom", "dingtalk"]`。

## 认领生命周期

```
claim_due(now)
  ├─ 先结算截止日期：enabled=1 AND end_at < now → completed, enabled=0, next_run_at=NULL
  ├─ 回收超时认领：running=1 AND claimed_at <= now-10min → running=0
  ├─ 选出候选：enabled=1 AND running=0 AND
  │    (retry_at IS NOT NULL AND retry_at <= now)
  │    OR (retry_at IS NULL AND next_run_at <= now)
  └─ 逐行 running 0→1（带 running=0 守卫），成功才计入返回值
```

- `retry_at` 优先于 `next_run_at`：处于退避中的任务不会提前按原 cron 触发。
- 认领写入 `dispatch_status = 'claimed'`；定义上的 `running` 是单任务互斥锁，
  同一时刻只有一个派发者能持有。

```
claim_manual_runs(now)
  ├─ 回收超时认领（同上）
  ├─ 选出 automation_runs：trigger='manual' AND dispatch_status='claimed'
  │    AND (attempts = 0 OR updated_at <= now-10min)，按 created_at 升序
  └─ 逐条：定义 running 0→1 成功 → run.attempts += 1、updated_at = now
```

`run_now` 不走 `claim_manual_runs`：它由当前 host 直接持有 manual run，
所以同时完成「占用定义 + 插入 run」。

## 派发结果

`mark_dispatched(dispatched_at, next_run_at?)`：

- `run_count` 与 `scheduled_run_count` 各 +1，清 attempts / retry / last_error，释放认领。
- 终态条件：非循环任务且 `scheduled_run_count >= max_runs.unwrap_or(1)`，或
  `end_at` 存在且 `next_run_at > end_at`（`next_run_at` 缺失按 `i64::MAX` 处理）。
- 终态写 `completed` / `enabled = 0` / `next_run_at = NULL`；否则 `active` / `enabled = 1`
  并写入新的 `next_run_at`。

`mark_dispatch_failed(failed_at, error, kind, next_run_at?)`：

| 分支 | 写入 |
| --- | --- |
| `Permanent` | `failed_to_dispatch` + `lifecycle_status = failed` + `enabled = 0` |
| `Transient` 且 attempts 未达 5 | `failed_to_dispatch`，attempts + 1，`retry_at = compute_retry_at(now, attempts)` |
| `Transient` 达 5 且循环任务 | 回到 `idle`，attempts 清零、`retry_at = NULL`，`next_run_at` 用调用方值 |
| `Transient` 达 5 且单次任务 | 终态 `failed` + `enabled = 0`；**保留** attempts 与 `retry_at`（与 TS 一致，靠 `enabled = 0` 退出认领） |

`mark_run_outcome(run_id, outcome, error?)`：只有新 outcome 为 `running`/`undefined` 时才允许
覆盖；既有终态（`succeeded` / `failed` / `cancelled` 等）保持不变。`error` 用
`COALESCE(?, error)`，只有显式传入才覆盖。

## `ModelSelection` 编码

- 落库（`create` / `update` / `upsert_run_claimed` / `fix_run_model_selection`）前
  严格编码：顶层与 `options` 都拒绝未知键，`providerId` / `modelId` /
  `options.reasoningLevel` 必须 `trim()` 后非空；空对象 `options` 整体省略。
- 读库用同一套严格解析；坏值在 `Automation` 输出里表现为缺失
  `modelSelection`，在 `get_model_selection_for_dispatch` 里表现为「不可用」错误。
- DB 字符串 `"null"` = 跟随 workspace 首选；SQL NULL = 缺配置（派发拒绝）。
  两者不可互换：迁移把旧默认写成 JSON null，不能借旧列 `model` / `thought_level`
  决定执行默认值。

## 输出形状

`Automation` 字段顺序与 TS 对象字面量一致：
`automationId, title, cronExpr, prompt, modelSelection?, mode?, workspaceKey, workspacePath,
workspaceIdentity?, targetTaskId?, locationKind, recurring, maxRuns?, endAt?, scheduleRule?,
scheduleEditedByUser?, runCount, enabled, lifecycleStatus, nextRunAt?, lastRunAt?, dispatchStatus,
dispatchAttempts, retryAt?, lastError?, createdAt, updatedAt`。

`AutomationRun`：`runId, automationId, workspaceKey, scheduledAt?, trigger, modelSelection?,
dispatchStatus, outcome?, sessionId?, error?, attempts, createdAt, updatedAt`。

可空字段缺省时整个键省略（`skip_serializing_if`），与 TS 丢弃 `undefined` 一致。
`schedule_rule` / `bot_delivery_target` 落库为 JSON 文本，读回解析为 JSON 值；
解析失败时 `scheduleRule` 缺失、投递目标返回 `None`。

## 实现约束

- **SQL 占位符不可混用**：SQLite 中 `?` 按出现顺序编号，`?N` 指向第 N 个绑定参数。
  同一语句里混用会让 `?N` 回指前面的匿名位，导致参数错位甚至
  `Wrong number of parameters`。本 crate 凡绑定两个以上参数的写语句一律统一编号
  （`?1…?N`），`tests/repo.rs::workspace_scoped_writes_do_not_touch_other_workspaces`
  锁定 `set_enabled` / `restart` / `run_now` 三处回归。
- 所有写入口都在 `with_conn` 内执行；跨多步的状态迁移（`create` / `run_now` /
  `claim_due` / `claim_manual_runs` / `skip_and_reschedule` /
  `mark_manual_run_dispatched`）用 `BEGIN IMMEDIATE` 包裹，失败 `ROLLBACK`。
- 时间全部来自注入时钟（`with_clock`，默认 `SystemTime`）；`claim_due` /
  `claim_manual_runs` / `run_now` / `mark_dispatched` / `mark_dispatch_failed` /
  `mark_manual_run_dispatched` 使用调用方显式传入的时间，其余方法用仓库时钟。

## 与 TS 的差异

| 差异 | 影响 |
| --- | --- |
| prepared 标记按 crate 独立 | 首次初始化可能多跑一次幂等的 `awaiting_approval` 修复 |
| `create` 的默认主键由 `uuid v4` 生成 | 与 `randomUUID()` 同样不可跨进程比较；fixture 一律显式核对前缀并归一化 |
| 错误文案 | 业务校验错误复刻 TS 文案；「行不存在 / 不属于工作区」为 Rust 文案 |
| `close({throwOnError})` | Rust 返回 `()`，不区分该选项 |
| TS `record()` 的 undefined → null | Rust 用 `Option` + 省略键表达；写库时按列语义落 `NULL` |

## 验收

- `pnpm automation:fixtures` 用真实 TS `AutomationRepo` 生成 **70 步**黄金数据
  （覆盖新增 / 更新三态 / 上限 / 截止 / 认领 / 回收 / 退避与放弃 / manual run /
  skip / 运行历史 / 跨 workspace / 删除与重开）。
- `tests/parity.rs`：迁移 → seed → 逐步重放，比较（a）方法返回 JSON 深等、
  （b）每步整库快照（两表全列 + JSON 文本归一），失败输出首个差异路径。
  UUID 主键与 `:manual:<uuid>` 由两侧同一套规则归一。
- `tests/repo.rs`：退避翻倍与封顶、创建上限稳定错误码、只回收超时认领、
  迟到结果不覆盖终态、workspace 作用域写入隔离、瞬时失败重试到放弃、
  永久失败立即关闭、单次任务达上限收尾、`update` 显式 null 清空。
- `cargo test -p automation-repo`、
  `cargo clippy -p automation-repo --all-targets -- -D warnings`。

## 未纳入

- `task` 频道接线与 session link：本 crate 只提供仓库语义，频道方法注册与
  Host 侧适配仍待 W3 后续波次。
