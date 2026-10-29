# Spec：任务索引仓库（`crates/task-index-repo`）

状态：2026-10-29 实现（W3-3 任务域第二批：分组视图与分组管理完成，Repo 面收口）。
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

本 crate 接管 `TaskIndexRepo` 的全部公开方法。数据库仍由 `crates/tasks-db`
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
| `has_grouped_workspace_bootstrap_run` | `hasGroupedWorkspaceBootstrapRun` | `task_group_workspace_bootstraps` 是否有任意行 |
| `create_task_group` | `createTaskGroup` | id = `task-group-<uuid v4>`；标题 trim 后空则 `New Group`；颜色缺省 `gray`；同时分配顶层排序 |
| `rename_task_group` | `renameTaskGroup` | 标题 trim 后空则 `New Group`；不存在/读回失败按 TS 文案报错 |
| `update_task_group_color` | `updateTaskGroupColor` | 先校验 7 种颜色，再更新 |
| `delete_task_group` | `deleteTaskGroup` | 单事务删组 + 顶层排序；成员由外键级联删除 |
| `initialize_grouped_task_at_top` | `initializeGroupedTaskAtTop` | 仅未删/未归档/未置顶、且无成员关系与顶层排序时分配 `min - 1000` |
| `query_grouped_task_view` | `queryGroupedTaskView` | workspace bootstrap、组可见性、组内/顶层排序补全、cron 组固定倒序 |
| `query_grouped_task_view_structure` | `queryGroupedTaskViewStructure` | 只读三张分组表，不 join tasks、不写回 |
| `apply_grouped_task_view_order` | `applyGroupedTaskViewOrder` | 校验后单事务重写成员与顶层排序，并写全局 bootstrap 标记 |
| `sync_task_meta_at_grouped_top` | `syncTaskMetaAtGroupedTop` | `sync_task_meta` + 顶层顺序初始化包在同一个 `BEGIN IMMEDIATE` 里 |

分组表的写入副作用（`ensure_ready` 回填、`sync_task_meta` 首次获得
cron/off-peak 标记）与 TS 一致。

## 分组视图语义

- workspace bootstrap 只执行一次：`task_group_workspace_bootstraps` 出现任意行后
  不再自动建组；`apply_grouped_task_view_order` 会写全局 ONCE_KEY 行永久关闭。
- workspace 组 id = `workspace-group-` + `sha256(workspaceKey)` 前 24 位十六进制；
  颜色 = `sha256` 首字节 `% 6` 取 `red/orange/yellow/green/blue/purple`；
  标题取路径最后一段（去尾部分隔符，JS trim 后为空则回落 `Workspace`）。
- 排序步长 `GROUPED_TASK_ORDER_STEP = 1000`；新顶层节点取 `MIN - 1000`，
  补齐缺失节点取 `MAX + 1000`（按 `createdAt` 倒序、再按节点 key 升序写入）。
- cron 系统分组不参与成员排序，固定按 `createdAt` 倒序、再按节点 key 倒序。
- 组可见性：带 bootstrap 记录的组只在其 workspace 可见；无记录的组始终可见。
- `queryGroupedTaskView` 的 tasks 查询**不读** `off_peak_task_id` 列
  （TS SELECT 清单里没有），因此 off-peak 身份只可能来自 `meta_json`。
- 顶层 task 节点 key 是 `JSON.stringify([workspaceKey, taskId])`；
  成员表用 `workspaceKey + "\0" + taskId`（NUL 不能进 `node_key`）。
- 错误文案逐字一致：`Task group 不存在，无法重命名`、
  `Task group 颜色无效`、`Grouped task order 包含不存在的 group` 等。

## 初始化回填只跑一次

`ensure_ready` 首次完成迁移与三个回填后写入进程内 prepared 注册表（等价 TS
`initializePromise`）。否则 `backfillOffPeakGroupMemberships` 的 `INSERT OR IGNORE`
会在用户把闲时任务移出系统分组后再次把它加回去，并重建已清空的分组；
`tests/repo.rs::ensure_ready_does_not_repeat_group_backfill` 覆盖该回归。

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
- 时钟只影响 `archive_stale_tasks` 的 cutoff 与分组写入的
  `created_at/updated_at/added_at`。fixture 固定 `clockMs = 2_000_000_000_000`，Rust 对拍
  使用同一值；分组行快照把时间列归一为 0。

## 与 TS 的差异

| 差异 | 影响 |
| --- | --- |
| `normalizeWorkspaceKeys` / 节点排序用码点序，不是 `localeCompare` | 大小写/重音混合时顺序可能不同；fixture 使用小写 ASCII key 并在归一后显式定序 |
| 搜索小写用 Unicode 默认映射，不是宿主 locale 的 `toLocaleLowerCase` | 非 ASCII 大小写折叠可能不同；fixture 搜索词为 ASCII |
| UTF-16 切断增补平面字符 | Rust 产生 U+FFFD，JS 产生孤立代理项；仅影响极端正文 |
| `update_task_state` 的 `unreadAt: undefined`（present-undefined 清空）用 `Some(None)` 表达 | TS 的 `"unreadAt" in patch` 语义保留；API 不接受 JSON null 充当 undefined |
| `meta_json` 属性顺序 | 不比较原始字节，比较解析后的 JSON |
| Rust API 使用强类型 `ModeTaskMeta` | TS 写入时原样 `JSON.stringify` 调用方对象（可含未知字段、可不带 `target.summaryTitle`）；Rust 按 schema 形状序列化（未知字段无法进入，`target.summaryTitle` 恒输出 `null`） |
| `close({throwOnError})` | Rust 返回 `Result`，不区分该选项 |
| `create_task_group` 的 id 由 `uuid v4` 生成 | 与 TS `randomUUID()` 同样不可跨进程比较；fixture 归一为 `task-group-normalized` |
| 分组写入的时间戳由注入时钟决定 | fixture 把分组表时间列与结构输出的 `addedAt` 归一为 0 |

## 下一片

- `host-channels` 的任务索引频道与会话链路接线（本 crate 已提供全部 Repo 方法）；
- `AutomationRepo` / `OffPeakTaskRepo` 等其余任务域表（如仍需 Rust 化）。

## 验收

- `crates/task-index-repo` 单元/集成测试：`ensure_ready` 幂等、写串行化、CRUD、
  列表/搜索、归档清理、错误路径。
- `tests/parity.rs`（`pnpm task-index:fixtures` 生成）：按步骤重建同一 DB，
  返回 JSON 深度比较 + `tasks` 关键列原值 + 分组表（时间列归一）快照。
- `cargo test -p task-index-repo`、`cargo clippy -p task-index-repo --all-targets -- -D warnings`。
