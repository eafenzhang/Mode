# Spec：SQLite schema 只增门禁

状态：2026-10-28 实现。关联：
`packages/services/src/session/tasksDatabase/migrations.ts`、
`apps/mode-cli/packages/adapters/src/storage/session-store/migrations.ts`。

## 目的

数据库迁移只能做「加表、加列、加索引」；删除表/列、修改列定义、替换索引都会让已安装
旧版本的数据库在升级后无法启动。类型检查和单元测试不覆盖这条契约，需要独立的 schema
快照门禁。

## 所有者与入口

- `scripts/sqlite-schema-snapshot.ts`：从真实迁移生成两个数据库的结构快照：
  - `tasks`：桌面任务库，调用 `runTasksDatabaseMigrations`；
  - `session-store`：CLI 会话库，按 `SQLITE_MIGRATIONS` 顺序执行 SQL。
- `scripts/lib/sqlite-schema.ts`：纯比较逻辑，唯一判定「只增」的地方。
- `scripts/check-sqlite-schema.ts`：检查/更新基线。
  - 默认 check：当前迁移产出的快照必须与 `scripts/sqlite-schema-baseline.json` 完全一致。
  - `--update`：先确认当前快照相对旧基线只增，再写新基线。
  - `--base-ref <ref>`：额外读取 `git show <ref>:scripts/sqlite-schema-baseline.json`，
    要求新基线相对它只增，防止「删列同时改基线」绕过检查。
- CI 入口：`.github/workflows/sqlite-schema.yml`（PR/push，`fetch-depth: 0`）与
  `release.yml` 的 `verify` job（当前基线一致性）。

## 快照口径

- 表：`sqlite_master.type='table'`，排除 `sqlite_%`、`tasks_schema_migration`、
  `schema_migration`。
- 列：`PRAGMA table_info` 的 `name/type/notnull/dflt_value/pk`。
- 索引：`sqlite_master.type='index' AND sql IS NOT NULL`，SQL 归一化空白后保存。
- 新增表、列、索引允许；删除或修改已存在项一律违规。
- 快照按数据库名、表名、列顺序、索引名排序，保证跨平台与跨运行稳定。

## 生产者对拍（过渡期）

任务库通过 `crates/tasks-db` 完成了 schema + 迁移的 Rust 复刻，但**写库的所有者仍是
Node Host**，所以门禁的权威生产者暂时仍是 TS：

- `scripts/check-tasks-schema-parity.ts`（`pnpm check:tasks-schema-parity`）在同一台
  机器上分别跑 TS `buildTasksSchemaSnapshot()` 与 Rust `tasks-db-snapshot` 二进制，
  按 `canonicalSchemaJson` 逐表比较；不一致直接失败并列出缺失/多出/变化的表。
- `.github/workflows/sqlite-schema.yml` 在门禁步骤前执行该对拍（需要 Rust toolchain）。
  路径过滤包含 `crates/tasks-db/**`，Rust 侧单独改动也会触发。
- 本地 cargo 不在 PATH 时用 `CARGO=D:\tools\cargo-msvc.cmd pnpm check:tasks-schema-parity`。

**何时切换生产者**：等 Rust 任务域真正接管写库（Repo / 频道落地）后，把
`sqlite-schema-snapshot.ts` 的 `tasks` 快照改为调用 Rust 二进制，并重新生成基线；
`--base-ref` 的「相对目标分支只增」语义不变。在那之前切生产者会让门禁看不到 TS 侧
的迁移变化，属于反向削弱，不做。

## 失败语义

- 历史基线读取失败（非「路径不存在」）必须报错，不能静默跳过。
- 基线文件缺失时 check 失败，提示运行 `pnpm sqlite-schema:update`。
- `--update` 发现删除/修改时失败，不写文件；先修迁移再更新基线。
- `--base-ref` 指向的树里没有基线文件时视为首次引入，允许；文件存在但读取失败则失败。
- CI 不自动更新基线；`--update` 只允许人工在本地执行并提交。

## 验收

1. 新增列/表/索引：check 失败（基线未更新）；`sqlite-schema:update` 后通过。
2. 删除列、修改列类型、删除索引、修改索引 SQL：check 与 update 都失败。
3. 同时删除列并修改基线：`--base-ref <旧分支>` 仍失败。
4. 两个数据库的快照都纳入基线。
5. `pnpm exec tsx --test scripts/test/sqlite-schema.test.ts` 覆盖上述纯比较分支。
