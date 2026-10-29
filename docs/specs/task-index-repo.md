# Spec：任务索引仓库（`crates/task-index-repo`）

状态：2026-10-29 实现（W3-3 任务域第一批：核心任务行 + 列表/搜索）。
行为真源（clean-room：只读本仓库 TS）：

- `packages/services/src/session/taskIndexRepo.ts`（本轮列出的方法）
- `packages/services/src/session/modeTaskListTypes.ts`
- `packages/services/src/session/tasksDatabase/*`（迁移基础由 `crates/tasks-db` 提供）
- `packages/shared/src/task-realtime-core.ts`（`resolveWorkspaceKey`）
- `packages/shared/src/remote-workspace-identity.ts`（`isRemoteWorkspaceIdentity`）
- `packages/shared/src/validation.ts`（`modeTaskMetaSchema`）
- `packages/shared/src/mode-task-types.ts`（`CRON_DEFAULT_GROUP_ID` / `OFF_PEAK_DEFAULT_GROUP_ID`）
- `packages/shared/src/mode-agent-policy.ts`（`MODE_AGENT_PROVIDER = "glm"`）

## 范围

本 crate 只接管任务索引的**核心行 + 列表/搜索**。数据库仍由 `crates/tasks-db`
负责 schema 与冻结迁移；本 crate 复用 `tasks_db::run_migrations`，不复制 DDL。

纳入方法：

| Rust 方法 | 对应 TS | 关键语义 |
| --- | --- | --- |
| `ensure_ready` | `ensureReady` / `initialize` | 建父目录、打开连接、`busy_timeout` / `foreign_keys=ON` / `WAL` / `synchronous=NORMAL`；未 prepared 时跑迁移，再执行 3 个幂等回填/清理 |
| `close` | `close` | 释放连接、清空路径与写锁表；`throwOnError` 语义不提供（Rust 返回 `Result`） |
| `get_task_meta` | `getTaskMeta` | 不存在或 `deleted=1` 返回 `None` |
| `sync_task_meta` | `syncTaskMeta` | 合并运行态快照；`updatedAt` 单调；保留更新更晚的终态；保留用户标题/迁移来源/cron/off-peak 身份；首次获得 cron/off-peak 标记时补系统分组归属 |
| `seed_task_meta_if_missing` | `seedTaskMetaIfMissing` | 行已存在（含 deleted）原样返回，不覆盖产品壳状态 |
| `clear_task_unread_if_matches` | `clearTaskUnreadIfMatches` | 写事务内 CAS 清未读；不存在/已删除报错 |
| `delete_archived_task` | `deleteArchivedTask` | 仅 `archived=1` 且未删除时写 tombstone；同事务删分组引用；否则 `None` |
| `update_task_state` | `updateTaskState` | 状态补丁；未读 marker 严格递增分配；删除/未读变更走写事务 |
| `apply_agent_patch` | `applyAgentPatch` | 行不存在/已删除返回 `None`；用户改过标题后拒绝 agent 标题 |
| `list_task_metas` | `listTaskMetas` | 可选 workspace/provider/pinned/archived/includeDeleted |
| `list_deleted_task_ids` | `listDeletedTaskIds` | tombstone 列表，`ORDER BY task_id` |
| `list_sessions_by_automation` | `listSessionsByAutomation` | `deleted=0`，按 `created_at DESC, task_id DESC` |
| `query_task_list` | `queryTaskList` | workspace 集合、kind、provider、搜索、limit、snippet |
| `archive_stale_tasks` | `archiveStaleTasks` | `olderThanDays` 至少 1 天；只归档未删/未归档/未置顶/无未读的 completed 行 |

明确不纳入（下一片：分组视图与分组管理）：
`hasGroupedWorkspaceBootstrapRun`、`createTaskGroup`、`renameTaskGroup`、
`updateTaskGroupColor`、`deleteTaskGroup`、`initializeGroupedTaskAtTop`、
`queryGroupedTaskView`、`queryGroupedTaskViewStructure`、`applyGroupedTaskViewOrder`、
`syncTaskMetaAtGroupedTop`。本 crate 不为它们提供近似实现。

但有两类**分组表写入是已纳入方法的副作用**，必须保持一致：

- `ensure_ready` 的 `backfillOffPeakGroupMemberships` / `cleanupDeletedTaskGroupingReferences`；
- `sync_task_meta` 首次获得 `cronAutomationId` / `offPeakTaskId` 时的
  `ensureSystemGroupMembership`。

## 行 → `ModeTaskMeta` 投影

`row_to_meta` 与 TS `rowToMeta` 同构：

1. `meta_json` 先按 `modeTaskMetaSchema` 解析（等价实现见下）。解析成功时以
   `meta_json` 为底，再覆盖：
   - `taskId` / `workspacePath` 取列；
   - `workspaceIdentity`：列 identity 去空白后等于 `workspace_key` 时取列；
     否则若 `workspace_key` 本身是合法远端 identity 则取 `workspace_key`；否则省略；
   - `unreadAt`：只取列（列 NULL 则省略）；
   - `cronAutomationId` / `offPeakTaskId`：`meta_json` 优先、列兜底；
   - `titleOverridden`：取列布尔。
2. 解析失败（JSON 非法或 schema 拒绝）时用列重建：`traceId = "mode-" + taskId`、
   `mode` / `migrationSource` / `status` 直接透传列原字符串（不做枚举校验）、
   `provider` 仅在列为 `"glm"` 时保留。

`modeTaskMetaSchema` 的等价解析：

- 必需：`taskId` / `traceId` / `workspacePath`（字符串、trim 后非空）、`title`
  （任意字符串）、`createdAt` / `updatedAt`（非负整数）、`mode`（六个枚举值之一）。
- `nonEmptyString` 字段在解析时 **trim**（`traceId` 等）；可选字段出现 `null`
  视为解析失败（`z.optional()` 不接受 null）。
- 未知字段被丢弃；`target` 内的 `summaryTitle` 缺省补 `null`；`target` 本身可为
  `null`（表示显式清空），与“字段缺失”区分。
- `lastError`、`changeSummary`、`target` 的子对象丢弃未知字段；`lastError.attribution`
  是 strict：出现未知字段即整体解析失败。

## `meta_json` 合并与序列化

- `sync_task_meta` 的合并顺序：先展开入参 meta，再按 TS 顺序覆盖
  `title`、`titleOverridden`、`status`、`lastError`、`target`、`migrationSource`、
  `cronAutomationId`、`offPeakTaskId`、`updatedAt`、`unreadAt`。
- `updatedAt = max(入参 updatedAt, 现有 meta updatedAt)`，保证不回退。
- 现有终态（`completed` / `error`）且入参状态为空或 `running`，且现有
  `updatedAt` 更大时，保留现有 `status` 与 `lastError`。
- 用户改过标题（`titleOverridden`）时保留现有标题。
- **序列化差异**：TS 用 `JSON.stringify(meta)`，属性顺序取决于调用方对象顺序；
  Rust 用固定字段声明顺序。对拍按“解析后的 JSON 深度相等”比较 `meta_json`，
  不比较原始字节（属性顺序不是语义）。

## 搜索与 snippet

- `query_task_list` 的 `search` 先做 JS `trim`；命中条件：
  `LOWER(title) LIKE %search% OR LOWER(searchable_text) LIKE %search%`。
  `searchable_text` 写入时按 **UTF-16 码元**截断到 200000。
- snippet：以匹配点为中心，前 20 / 后 72 个 UTF-16 码元；前缀/后缀补 `...`；
  归一化空白（JS `\s+` → 单空格、JS trim）后截断 140；窗口重叠（交集 > 0）去重；
  最多 4 条；正文无命中但标题命中时回退一条整段摘要。
- Rust 用 Unicode 默认小写（`to_lowercase`）复刻 `toLocaleLowerCase()`；fixture
  搜索词限定 ASCII。字符串切片按 UTF-16 码元；增补平面字符被切断时 Rust 用
  `from_utf16_lossy`，与 JS 产生孤立代理项的行为不同（已记录）。

## 写入串行化

- TS：`writeChains` 以 `workspaceKey + "\0" + taskId` 为 key 串行。
- Rust：每个 key 一个 `Arc<Mutex<()>>` 写闸门，先拿 key 闸门再拿连接锁；
  同一 key 的写入不交错，不同 key 之间由连接锁天然串行（rusqlite 单连接）。
  key 在无人等待时从表中回收。`ensure_ready` 自身由状态锁串行且幂等。

## 时钟

- `TaskIndexRepo::with_clock(path, fn)` 注入 `now_ms`；默认 `SystemTime`。
- 时钟只影响 `archive_stale_tasks` 的 cutoff 与系统分组成员行的
  `created_at/updated_at/added_at`。fixture 固定 `clockMs = 2_000_000_000_000`，Rust 对拍
  使用同一值；分组行快照把时间列归一为 0。重复生成 fixture 字节稳定。

## 与 TS 的差异

| 差异 | 影响 |
| --- | --- |
| 无模块级 prepared/migrated 注册表时每次 `ensure_ready` 都跑幂等回填 | 结果一致，只多一点 SQL |
| 不提供 `syncTaskMetaAtGroupedTop` 等分组 admission | 属于下一片；本轮不写近似实现 |
| `normalizeWorkspaceKeys` 用码点排序，不是 `localeCompare` | 大小写/重音混合的 key 顺序可能不同；fixture 使用小写 ASCII key |
| 搜索小写用 Unicode 默认映射，不是宿主 locale 的 `toLocaleLowerCase` | 非 ASCII 大小写折叠可能不同；fixture 搜索词为 ASCII |
| UTF-16 切断增补平面字符 | Rust 产生 U+FFFD，JS 产生孤立代理项；仅影响极端正文 |
| `update_task_state` 的 `unreadAt: undefined`（present-undefined 清空）用 `Some(None)` 表达 | TS 的 `"unreadAt" in patch` 语义保留；API 不接受 JSON null 充当 undefined |
| `meta_json` 属性顺序 | 不比较原始字节，比较解析后的 JSON |
| Rust API 使用强类型 `ModeTaskMeta` | TS 写入时原样 `JSON.stringify` 调用方对象（可含未知字段、可不带 `target.summaryTitle`）；Rust 按 schema 形状序列化（未知字段无法进入，`target.summaryTitle` 恒输出 `null`） |
| `close({throwOnError})` | Rust 返回 `Result`，不区分该选项 |

## 下一片（分组视图与分组管理）

- 分组结构查询：`queryGroupedTaskViewStructure`、`queryGroupedTaskView`、
  `applyGroupedTaskViewOrder`；
- 分组 CRUD 与颜色校验、workspace bootstrap 一次性初始化；
- 顶层/组内排序分配（`GROUPED_TASK_ORDER_STEP = 1000`）与 `task_group_view_node_orders`；
- `syncTaskMetaAtGroupedTop` 的原子 admission；
- 验收：同一 fixture 扩展分组步骤，Rust 输出与 TS 深度相等；时间列归一策略不变。

## 验收

- `crates/task-index-repo` 单元/集成测试：`ensure_ready` 幂等、写串行化、CRUD、
  列表/搜索、归档清理、错误路径。
- `tests/parity.rs`（`pnpm task-index:fixtures` 生成）：按步骤重建同一 DB，
  返回 JSON 深度比较 + `tasks` 关键列原值 + 分组表（时间列归一）快照。
- `cargo test -p task-index-repo`、`cargo clippy -p task-index-repo --all-targets -- -D warnings`。
