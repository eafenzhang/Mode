//! 桌面任务库（`tasks` SQLite）的 schema、冻结迁移与快照。
//!
//! 行为真源（clean-room：只读本仓库 TS）：
//!
//! - `packages/services/src/session/tasksDatabase/schema-v1.ts`
//! - `packages/services/src/session/tasksDatabase/migrations.ts`
//! - `packages/services/src/session/tasksDatabase/provider-selection-v2.ts`
//! - `packages/services/src/session/tasksDatabase/official-glm-selection-v3.ts`
//! - `packages/services/src/session/tasksDatabase/startup.ts`
//! - `scripts/sqlite-schema-snapshot.ts` + `scripts/lib/sqlite-schema.ts`
//!
//! 规范见 `docs/specs/tasks-db.md`。本 crate 不包含 CLI 会话库，也不接管三个 Repo 的
//! `ensureReady()`。

mod error;
mod js_compat;
mod legacy_selection;
mod migrations;
mod schema;
mod snapshot;
mod startup;

pub use error::{TasksDbError, TasksDbErrorKind};
pub use legacy_selection::import_legacy_automation_selections;
pub use migrations::{
    adopt_schema, are_migrations_applied, inspect_tasks_migration_kind, run_migrations,
    MigrationFacts, MigrationKind, MigrationProgress, RunOptions,
};
pub use schema::{
    migration_definitions, MigrationDefinition, AUTOMATION_SCHEMA, MIGRATION_IDS,
    OFFICIAL_GLM_SELECTION_MIGRATION_SQL, OFF_PEAK_SCHEMA, TASK_INDEX_SCHEMA,
};
pub use snapshot::{
    build_tasks_schema_snapshot, ColumnSnapshot, IndexSnapshot, TableSnapshot, TasksSchemaSnapshot,
};
pub use startup::{prepare_tasks_storage, PrepareOptions, TasksStoragePhase};
