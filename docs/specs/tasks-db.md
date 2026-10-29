# Spec：桌面任务库 schema + 迁移 runner（`crates/tasks-db`）

状态：2026-10-29 实现（W3-3 前置）。行为真源（clean-room：只读本仓库 TS）：

- `packages/services/src/session/tasksDatabase/schema-v1.ts`（三个 DDL 模板）
- `packages/services/src/session/tasksDatabase/migrations.ts`（迁移定义、账本、adopt、inspect/areApplied）
- `packages/services/src/session/tasksDatabase/provider-selection-v2.ts`（0002 旧数据导入）
- `packages/services/src/session/tasksDatabase/official-glm-selection-v3.ts`（0003 SQL）
- `packages/services/src/session/tasksDatabase/startup.ts`（打开、锁等待、阶段回调）
- `packages/services/src/session/tasksDatabase/prepared.ts`（只理解，不在 Rust 复刻进程内交接集合）
- `scripts/sqlite-schema-snapshot.ts` + `scripts/lib/sqlite-schema.ts`（快照形状与归一化）

## 范围

本 crate 只负责**桌面任务库（`tasks` SQLite 库）的 schema 与迁移**：

- 三个 DDL 常量与 3 个迁移定义/checksum；
- 迁移账本 `tasks_schema_migration` 的建立、比对、执行与事实返回；
- 0001 `adoptSchema`、0002 旧 provider/model 选择导入、0003 官方 GLM 展示名；
- `prepare_tasks_storage`：建目录、打开连接、WAL/锁等待、迁移、阶段回调；
- `build_tasks_schema_snapshot`：内存库跑完迁移后按 TS 快照形状产出的可序列化结构。

**不在范围**：

- CLI 会话库（`session-store`）迁移；
- `TaskIndexRepo` / `AutomationRepo` / `OffPeakTaskRepo` 的 `ensureReady()`。TS
  `prepareTasksIndexStorage` 在迁移提交后还会调用这三个 Repo 做历史修复并上报 `ready`；
  本 crate 只上报 `maintaining` 与 `ready` 的迁移阶段，Repo 级修理由后续任务域接管。
  后续域在自己的 `ensureReady` 完成后才能把存储视为 prepared。
- 进程内交接集合（`prepared.ts` 的 `migrated` / `prepared` Set）。这是同一进程内的
  快捷凭据，不是事实来源；Rust 侧每次都以账本 + checksum 为准。
- `scripts/check-sqlite-schema.ts`、基线文件与 CI 门禁。本 crate 只提供快照函数，
  门禁切换到 Rust 生产者是后续独立步骤。

## 迁移账本与 checksum

账本表（与 TS 同名同列）：

```sql
CREATE TABLE IF NOT EXISTS tasks_schema_migration (
  id TEXT PRIMARY KEY,
  checksum TEXT NOT NULL,
  time_applied INTEGER NOT NULL
);
```

三条迁移：

| id | checksumInput |
| --- | --- |
| `0001_adopt_task_schema` | `[TASK_INDEX_SCHEMA, AUTOMATION_SCHEMA, OFF_PEAK_SCHEMA, columns, indexes, boundIndex, "scheduled-count-backfill-v1"]` |
| `0002_provider_selection` | `["legacy-automation-selection-v1", "no-provider-for-legacy-off-peak-v1"]` |
| `0003_official_glm_selection` | `[OFFICIAL_GLM_SELECTION_MIGRATION_SQL]` |

**checksum = `sha256(JSON.stringify(checksumInput))` 的小写 hex。** 必须与 TS 字节一致：

- `JSON.stringify` 无空格；Rust 使用 `serde_json::to_string`。
- 字符串转义规则只在 `"`、`\`、`< 0x20` 控制字符上转义；不转义 `/`、不转义非 ASCII。
- 三个 DDL 常量必须与 TS 模板字符串**逐字节一致**（含前导换行、缩进、末尾换行后的缩进
  空格），因为 checksum 直接吃原始字符串。冻结列声明、`indexes`、`boundIndex` 与
  0003 SQL 同理。发布后不得修改这些字符串；布局变化只能新增迁移。

`adoptSchema` 的冻结列清单（顺序即执行顺序）：

```
tasks: title_overridden, last_unread_at, searchable_text, cron_automation_id, off_peak_task_id
automations: target_task_id, bot_delivery_target, mode, end_at, schedule_rule,
             schedule_edited_by_user, thought_level, model_selection, scheduled_run_count
automation_runs: model_selection
off_peak_tasks: thought_level, model_selection, history_deleted_at
```

## runner 语义（`run_migrations`）

入参：连接、`transaction_open`、可选初始 facts、可选 progress 回调。

1. `transaction_open == false` 时执行 `BEGIN IMMEDIATE`。
2. 没有外部传入 facts 时，先 `inspect_tasks_migration_kind` 得到 `kind`。
3. 建账本表。
4. baseline：`SELECT id FROM tasks_schema_migration ORDER BY id DESC LIMIT 1`。
   - 有行且 id 匹配 `^[a-zA-Z_0-9-]{1,128}$` → `lastAppliedMigrationId = id`；
   - 无行 → `null`；
   - 有行但 id 非法 → 视为 `null`（TS `safeParse(...).data` 为 `undefined`）。
5. 依次比对每条迁移：
   - 已存在且 checksum 相等 → 跳过；
   - 已存在但 checksum 不同 → 失败 `kind = checksum_mismatch`，携带 `migrationId`；
   - 不存在 → 若当前 `kind == "none"` 置 `upgrade`；先回调 `migrating`；执行对应迁移；
     `executedCount += 1`；插入账本行，`time_applied = 当前毫秒`。
6. 回调 `committing`；`COMMIT`；`committedCount = executedCount`；返回 facts。
7. 任一步失败：若仍在事务内则 `ROLLBACK`（回滚错误不得覆盖首因）；错误附加当前
   `migrationId`（若错误自身没有）后返回。

`inspect_tasks_migration_kind`：

- 无账本 → 有待执行项；
- 有账本则逐条查 checksum，缺失任意一条即 `pending`，任一不匹配直接
  `checksum_mismatch`；
- 无 pending → `none`；有 pending 且存在除
  `tasks_schema_migration` / `sqlite_sequence` 之外的表 → `upgrade`，否则 `initialize`。

`are_migrations_applied`：

- 账本表不存在 → `false`；
- 任一迁移缺行 → `false`；
- 任一 checksum 不匹配 → 抛 `checksum_mismatch`（带 `migrationId`）；
- 全部匹配 → `true`。

## 0001 adoptSchema

1. 顺序执行 `TASK_INDEX_SCHEMA + AUTOMATION_SCHEMA + OFF_PEAK_SCHEMA`（全是
   `CREATE TABLE/INDEX IF NOT EXISTS`）。
2. 对冻结列清单逐项 `PRAGMA table_info(<table>)`：列已存在则跳过，否则
   `ALTER TABLE <table> ADD COLUMN <column> <definition>`。
3. 新增 `automations.scheduled_run_count` 时立即回填
   `UPDATE automations SET scheduled_run_count = run_count`。
4. 执行冻结索引 SQL（`idx_tasks_cron_automation` / `idx_tasks_off_peak_task` /
   `idx_automations_target_task`）。
5. 旧版 `off_peak_tasks` 可能存在重复活跃绑定：
   `SELECT 1 ... WHERE session_id IS NOT NULL AND status NOT IN ('completed','failed','cancelled')
   GROUP BY workspace_key, session_id HAVING count(*) > 1 LIMIT 1`。
   有重复 → **跳过**唯一索引 `idx_off_peak_bound_active`；无重复 → 创建。

不吞掉权限/语法/磁盘等真实 SQL 错误。

## 0002 旧 provider/model 选择导入

逐行读取 `automations WHERE model IS NOT NULL`，冻结复刻 `decodeLegacySelection`：

- `value = model.trim()`；空 → 不解码。
- `custom:` 前缀：
  - `body = value[7..]`，无 `:` → 无法确定身份；
  - `body.split(':')` 长度 ≥ 3 且首段为 `builtin` → `provider = "builtin:" + parts[1]`，
    `model = decodeURIComponent(parts[2..].join(":"))`；
  - 否则 `provider = decodeURIComponent(body[..sep])`，
    `model = decodeURIComponent(body[sep+1..])`。
- 含 `/`：以第一个 `/` 切 provider/model；model 中第一个 `$` 且不在首尾时，
  `$` 之后 trim 得到 `reasoningLevel`，空则整体视为无法确定。
- 否则 provider 为 `glm` / `mode` → 旧执行后端，不是供应商标识，视为无法确定。
- `provider/model` 再 trim；任一为空 → 无法确定。
- `provider` 以 `builtin:` 开头时查冻结映射表；未知一律无法确定。
- 成功 → `{ "providerId": ..., "modelId": ..., "options"?: { "reasoningLevel": ... } }`。

**键序**：TS 对象字面量插入序是 `providerId`、`modelId`、`options`；`options` 内为
`reasoningLevel`。Rust 不使用 `serde_json::Map`（默认有序容器会改键序），而是按该顺序
手工拼接 JSON；每个字符串值用 `serde_json::to_string` 生成转义（与 `JSON.stringify`
的字符串转义一致）。

写回：

```sql
UPDATE automations SET model_selection = ?
 WHERE automation_id = ? AND model IS ? AND provider IS ? AND thought_level IS ?
```

`IS ?` 用于 NULL 安全比较；旧列与时间戳不改。无法确定但 `model.trim()` 非空的旧行，
写 `model_selection = NULL`（避免静默换模型）。最后：

```sql
UPDATE automations SET model_selection = 'null'
 WHERE model_selection IS NULL AND (model IS NULL OR trim(model) = '')
```

`decodeURIComponent` 语义：解码 `%XX` 字节流后必须是合法 UTF-8，否则**回退原文**
（TS 的 `URIError` 分支）。`%` 未跟两位十六进制同样回退。

## 0003 官方 GLM 展示名

单条冻结 SQL：仅当 `model_selection` 是合法 JSON、`providerId` 属于官方编码计划且
`modelId` 的 lower 命中冻结清单时，用 `json_set` 改写 `$.modelId` 为展示名。坏 JSON
不影响启动。

## 打开与锁等待（`prepare_tasks_storage`）

阶段：`checking` →（可能 `waiting_for_lock`）→ `migrating` / `committing` →
`maintaining` → `ready`。

1. 回调 `checking`；递归创建父目录。
2. 打开连接；`PRAGMA busy_timeout = 25`；`PRAGMA foreign_keys = ON`。
3. 对 `PRAGMA journal_mode = WAL` 与 `BEGIN IMMEDIATE` 使用同一重试器：
   - 仅 `SQLITE_BUSY`（errcode & 0xff == 5）重试；其它错误直接失败；
   - 首次等待时回调一次 `waiting_for_lock`（携带当时的 facts）；
   - 每 100 ms 重试，超过 `lock_wait_ms`（默认 60 分钟）失败
     `kind = lock_timeout`，消息 `Task storage lock wait expired`。
4. `PRAGMA synchronous = NORMAL`。
5. 锁内 `inspect_tasks_migration_kind` 得到 facts（`executedCount/committedCount = 0`），
   回调 `checking` + facts。
6. `BEGIN IMMEDIATE` → `run_migrations(transaction_open = true, 初始 facts)`，
   其 `migrating` / `committing` 直接上报。
7. 提交成功后回调 `maintaining` + facts。
8. 关闭连接；关闭失败且此前无失败 → 失败。
9. 回调 `ready` + facts，返回 facts。

失败时把当时的迁移 facts 附在错误上（`startup_migration`），不倒退上报
`checking` / `migrating`，也不覆盖首因。锁超时/checksum 等错误同时保留
`migrationId` 与 SQLite 主错误码。

**不做的部分**：TS 在 `maintaining` 之后创建三个 Repo 并 `ensureReady()` 做历史修复，
然后才 `markPrepared` 并上报 `ready`。本 crate 不实现这些 Repo；`ready` 只表示
schema/迁移层完成。后续任务域接管 Repo 修复，完成后再对外发布 prepared。

## 快照口径（`build_tasks_schema_snapshot`）

- 内存库执行完整迁移后读取。
- 表：`sqlite_master.type='table' AND name NOT LIKE 'sqlite_%'`，排除
  `tasks_schema_migration`、`schema_migration`，按 name 升序。
- 列：`PRAGMA table_info(<quoted name>)`，字段
  `name` / `type` / `notNull`（`notnull === 1`）/ `defaultValue`（`dflt_value ?? null`）/
  `primaryKey`（`pk`），按 name 升序。
- 索引：`type='index' AND tbl_name=? AND sql IS NOT NULL`，按 name 升序；SQL 做
  「空白段折叠为单个空格并 trim」归一化（JS `\s` 口径）。
- 输出 `{ "tables": { <name>: { "columns": [...], "indexes": [...] } } }`，字段名
  camelCase。
- 本函数只供将来把 SQLite 门禁的生产者切到 Rust，不修改现有门禁/基线/CI。

## 与 TS 的已知差异

| 差异 | 影响 |
| --- | --- |
| `prepared.ts` 的进程内 Set 未复刻 | 每次 `are_migrations_applied` 都读账本；更严格，不会把替换/清空文件当 ready |
| 三个 Repo 的 `ensureReady` 未复刻 | `prepare_tasks_storage` 的 `ready` 只代表 schema 就绪；Repo 修复由后续域接管 |
| 错误对象形态（TS `Object.assign` 附加字段） | Rust 用 `TasksDbError { kind, message, migration_id, startup_migration, sqlite_code }` 表达同一信息 |
| `lastAppliedMigrationId` 的「缺失 / null / 非法 id」 | Rust 用 `Option<Option<String>>` 区分缺失与 null；非法 id 归 null，与 TS `safeParse(...).data` 一致 |
| progress 回调把引用交给调用方而不是深拷贝 | Rust 回调签名是借用；调用方若需要留存必须自行 clone。语义仍是「当时的事实」 |
| JS `trim`/`\s` 与 Rust `char::is_whitespace` 不完全相同 | 实现按 ECMAScript WhiteSpace + LineTerminator 集合复刻（含 `\uFEFF`，排除 `\u0085`） |

## 验收

1. 三条迁移的 checksum 与 TS 实跑账本一致（`pnpm tasks-db:fixtures` 生成 fixture）。
2. Rust 快照与 TS `buildTasksSchemaSnapshot()` 解析后深度相等。
3. 空库迁移 → 3 条账本、`kind = initialize`、`executed/committed = 3`；再次运行 →
   `kind = none`、执行 0 条。
4. 篡改账本 checksum → `checksum_mismatch` + `migrationId`。
5. inspect：空库 `initialize`；无账本但已有其它表 `upgrade`；完整账本 `none`。
6. 预置旧 `tasks` / `automations` 表时 adopt 补齐异常列并回填
   `scheduled_run_count = run_count`。
7. 旧 `off_peak_tasks` 有重复活跃 `(workspace_key, session_id)` 时不创建唯一索引。
8. 0002 fixture 的全部旧行分支导入结果逐字符串一致（含 NULL 与 `'null'`）。
9. `prepare_tasks_storage` 在临时路径上跑通并产出 3 条账本（可用短 `lock_wait_ms`）。
10. `cargo test -p tasks-db`、`cargo clippy -p tasks-db --all-targets -- -D warnings`
    全绿。
