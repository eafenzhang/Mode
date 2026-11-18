//! 冻结迁移清单与 runner（LT-1 C2 T7）：空数据根创建与 oracle 一致的 schema + 账本。
//!
//! 行为真源是 T0 的机器录制，不是手写期望：
//!
//! - 账本行（`id`/`checksum`/`app_version`/顺序）与建库 SQL 来自
//!   `packages/desktop/tests/agent-runtime-oracle/fixtures/c2/fresh-install-ledger.json`
//!   的 `schema_migration` 行与 `sqlite_master` 条目（oracle 真实库观察）。
//! - **单条迁移的 SQL 切分不可观察**（T0 只录制了最终 schema）：22 步按迁移名把
//!   69 个对象归属到最贴合的一步；无法推导切分的步登记为 no-op 占位或缺列守卫
//!   （`tasks-db` 的 FROZEN_COLUMNS 模式），可观察契约是最终 schema + 账本字节，
//!   两者逐字节对齐（`agent-c2-contract.md` §5、`agent-c2-context-memory.md` §6.1）。
//! - 每条 CREATE 语句用 fixture 的 oracle 原文加 `IF NOT EXISTS`：SQLite 在
//!   `sqlite_master.sql` 里剥掉该子句，因此新建库的存储文本与 oracle 逐字节一致。
//!   发布后不得改写这些字符串；布局变化只能新增迁移。
//!
//! runner 语义沿用 `crates/tasks-db`（同仓库模式参考）：`BEGIN IMMEDIATE` 单事务、
//! 锁内重读账本（并发首启只有一个创建者）、checksum 不一致 fail-closed、
//! `time_applied` 机器毫秒且同事务内非递减。决策表见 `SessionStore::open_with_progress`：
//! 空 schema → initialize；非空 schema 且账本缺失/为空 → 保持 C1 fail-closed；
//! 账本有行 → 缺失的步补跑（upgrade）、checksum 不符 → `storage_schema_unsupported`。

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::StoreError;
use crate::schema::SCHEMA_MIGRATION_TABLE;

/// 账本 DDL：fixture `schema_migration` 的 oracle 原文加 `IF NOT EXISTS`。
const LEDGER_DDL: &str = "CREATE TABLE IF NOT EXISTS schema_migration (
      id text primary key, checksum text not null, app_version text, time_applied integer not null
    )";

/// 迁移需求种类（`databaseMigrationFactsSchema` 的 `kind` 取值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationKind {
    /// 冻结清单全部已应用。
    None,
    /// 空库初始化。
    Initialize,
    /// 已有库补跑缺失的迁移。
    Upgrade,
}

impl MigrationKind {
    /// 协议取值（小写字符串）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Initialize => "initialize",
            Self::Upgrade => "upgrade",
        }
    }
}

/// 迁移事实快照（门帧 `migration` 字段的事实源）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationFacts {
    pub kind: MigrationKind,
    pub executed_count: u64,
    pub committed_count: u64,
    /// 锁内账本基线（`SELECT id ... ORDER BY id DESC LIMIT 1`）；空账本为
    /// `None`（oracle 帧 fresh 全程 null 的口径）。预检事实尚未取得基线。
    pub last_applied_migration_id: Option<String>,
}

impl Default for MigrationFacts {
    fn default() -> Self {
        Self {
            kind: MigrationKind::None,
            executed_count: 0,
            committed_count: 0,
            last_applied_migration_id: None,
        }
    }
}

/// 迁移进度事件：`open_with_progress` 的回调载荷。
///
/// `Migrating` 在**执行该步之前**发出（oracle 帧：`migrationId` 是即将执行的步，
/// `completed` 是已执行数）；`Committing` 在 `COMMIT` 之前发出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationEvent {
    /// 预检完成（门第 3 个 `checking` 帧的事实源；尚无 `lastAppliedMigrationId`）。
    Inspected { facts: MigrationFacts },
    /// 即将执行一步迁移。
    Migrating {
        migration_id: &'static str,
        facts: MigrationFacts,
    },
    /// 全部缺失步执行完，准备提交。
    Committing { facts: MigrationFacts },
}

/// 一条冻结迁移：账本行事实 + 该步的建库 SQL / 缺列守卫。
pub struct MigrationDefinition {
    pub id: &'static str,
    pub checksum: &'static str,
    pub app_version: &'static str,
    /// 该步负责的对象建库 SQL（幂等 `IF NOT EXISTS`）；无可观察建库对象的步为
    /// 空串（no-op 占位，仍记录账本行）。
    pub create_sql: &'static str,
    /// 缺列才补的 `(表, 列, 列定义)`：升级路径守卫（建库 DDL 已含最终列，
    /// 新建路径恒 no-op）。
    pub add_columns: &'static [(&'static str, &'static str, &'static str)],
}

static SESSION_TARGET_ACCOUNTING_COLUMNS: &[(&str, &str, &str)] = &[
    (
        "session_target",
        "tokens_used",
        "integer not null default 0",
    ),
    (
        "session_target",
        "time_used_seconds",
        "integer not null default 0",
    ),
];
static INPUT_HISTORY_ATTACHMENTS_COLUMNS: &[(&str, &str, &str)] =
    &[("input_history", "attachments", "text")];
static WORKFLOW_DEFINITION_SCOPE_COLUMNS: &[(&str, &str, &str)] = &[(
    "workflow_definition",
    "scope",
    "text not null default 'explicit' check(scope in ('builtin', 'explicit', 'project', 'user'))",
)];
static SESSION_TITLE_METADATA_COLUMNS: &[(&str, &str, &str)] = &[
    ("session", "title_source", "text not null default 'first_input' check(title_source in ('default', 'first_input', 'generated', 'custom'))"),
    ("session", "title_message_id", "text"),
    ("session", "time_title_updated", "integer"),
];
static SESSION_TARGET_SUMMARY_TITLE_COLUMNS: &[(&str, &str, &str)] = &[
    ("session_target", "summary_title", "text"),
    ("session_target", "active_input_id", "text"),
];
static SESSION_TRACE_ID_COLUMNS: &[(&str, &str, &str)] = &[("session", "trace_id", "text")];
static SESSION_TARGET_ACTIVE_RUN_COLUMNS: &[(&str, &str, &str)] = &[
    ("session_target", "active_run_started_at", "integer"),
    ("session_target", "active_run_last_seen_at", "integer"),
];
static MESSAGE_PART_SEQUENCE_COLUMNS: &[(&str, &str, &str)] = &[
    ("message", "sequence", "integer"),
    ("part", "sequence", "integer"),
];

/// 冻结迁移清单（22 条；id/checksum/app_version 与 fixture 逐字节一致）。
static MIGRATIONS: &[MigrationDefinition] = &[
    MigrationDefinition {
        id: "0001_base_session_store",
        checksum: "60e2d6a38ab36f31417c4f92c02690c96c7dcaaa0b6abe1741117d62a55c6462",
        app_version: "0.2.0",
        create_sql: r#"CREATE TABLE IF NOT EXISTS session (
        id text primary key,
        project_id text not null,
        workspace_id text,
        parent_id text,
        slug text not null,
        directory text not null,
        path text,
        title text not null,
        version text not null,
        share_url text,
        summary_additions integer,
        summary_deletions integer,
        summary_files integer,
        summary_diffs text,
        revert text,
        permission text,
        time_created integer not null,
        time_updated integer not null,
        time_compacting integer,
        time_archived integer
      , task_type text not null default 'interactive', title_source text not null default 'first_input'
        check(title_source in ('default', 'first_input', 'generated', 'custom')), title_message_id text, time_title_updated integer, trace_id text);
CREATE TABLE IF NOT EXISTS message (
        id text primary key,
        session_id text not null references session(id) on delete cascade,
        time_created integer not null,
        time_updated integer not null,
        data text not null
      , sequence integer);
CREATE TABLE IF NOT EXISTS part (
        id text primary key,
        message_id text not null references message(id) on delete cascade,
        session_id text not null,
        time_created integer not null,
        time_updated integer not null,
        data text not null
      , sequence integer);
CREATE TABLE IF NOT EXISTS session_entry (
        id text primary key,
        session_id text not null references session(id) on delete cascade,
        type text not null,
        time_created integer not null,
        time_updated integer not null,
        data text not null
      );
CREATE TABLE IF NOT EXISTS todo (
        session_id text not null references session(id) on delete cascade,
        content text not null,
        status text not null,
        priority text not null,
        position integer not null,
        time_created integer not null,
        time_updated integer not null,
        primary key(session_id, position)
      );
CREATE TABLE IF NOT EXISTS permission (
        project_id text primary key,
        time_created integer not null,
        time_updated integer not null,
        data text not null
      );
CREATE INDEX IF NOT EXISTS session_parent_idx on session(parent_id);
CREATE INDEX IF NOT EXISTS session_project_idx on session(project_id);
CREATE INDEX IF NOT EXISTS session_task_type_idx on session(task_type);
CREATE INDEX IF NOT EXISTS session_trace_idx on session(trace_id);
CREATE INDEX IF NOT EXISTS session_workspace_idx on session(workspace_id);
CREATE INDEX IF NOT EXISTS message_session_sequence_idx
        on message(session_id, sequence, time_created, id);
CREATE INDEX IF NOT EXISTS message_session_time_created_id_idx
        on message(session_id, time_created, id);
CREATE INDEX IF NOT EXISTS part_message_id_id_idx on part(message_id, id);
CREATE INDEX IF NOT EXISTS part_message_sequence_idx
        on part(message_id, sequence, time_created, id);
CREATE INDEX IF NOT EXISTS part_session_idx on part(session_id);
CREATE INDEX IF NOT EXISTS part_session_message_sequence_idx
        on part(session_id, message_id, sequence);
CREATE INDEX IF NOT EXISTS session_entry_session_idx on session_entry(session_id);
CREATE INDEX IF NOT EXISTS session_entry_session_type_idx on session_entry(session_id, type);
CREATE INDEX IF NOT EXISTS session_entry_time_created_idx on session_entry(time_created);
CREATE INDEX IF NOT EXISTS todo_session_idx on todo(session_id);"#,
        add_columns: NO_ADD_COLUMNS,
    },
    MigrationDefinition {
        id: "0002_local_setting",
        checksum: "22a6ada9325c9ad55a00a1f0ecf72332c63c1e89fc3d91fec8d155bb0058b465",
        app_version: "0.2.0",
        create_sql: r#"CREATE TABLE IF NOT EXISTS local_setting (
        scope text not null,
        scope_id text not null,
        namespace text not null,
        key text not null,
        value text not null,
        schema_version integer not null,
        time_created integer not null,
        time_updated integer not null,
        primary key(scope, scope_id, namespace, key)
      );
CREATE INDEX IF NOT EXISTS local_setting_namespace_key_idx
        on local_setting(namespace, key);
CREATE INDEX IF NOT EXISTS local_setting_scope_idx
        on local_setting(scope, scope_id);"#,
        add_columns: NO_ADD_COLUMNS,
    },
    MigrationDefinition {
        id: "0003_backfill_permission_local_setting",
        checksum: "bd880375ba7b948c8bcda847e7eb52bc568d065bcba48190f6c3bfb14d11b7dc",
        app_version: "0.2.0",
        create_sql: "",
        add_columns: NO_ADD_COLUMNS,
    },
    MigrationDefinition {
        id: "0004_session_target",
        checksum: "df670752991c78e38e2f25b0a003abc0e9689bc8d4894351358f2f83335a3ae1",
        app_version: "0.7.0",
        create_sql: r#"CREATE TABLE IF NOT EXISTS "session_target" (
        session_id text primary key references session(id) on delete cascade,
        target_id text not null,
        objective text not null,
        status text not null check(status in ('active', 'paused', 'budget_limited', 'complete')),
        token_budget integer,
        tokens_used integer not null default 0,
        time_used_seconds integer not null default 0,
        time_created integer not null,
        time_updated integer not null
      , summary_title text, active_input_id text, active_run_started_at integer, active_run_last_seen_at integer);"#,
        add_columns: NO_ADD_COLUMNS,
    },
    MigrationDefinition {
        id: "0005_session_target_accounting",
        checksum: "6138ed4562dfdd5b571d39d3b62266c55dd9eb8f2f41948e046441a0e4a954cc",
        app_version: "0.7.0",
        create_sql: "",
        add_columns: SESSION_TARGET_ACCOUNTING_COLUMNS,
    },
    MigrationDefinition {
        id: "0006_input_history_attachments",
        checksum: "a2eab98649d738e15bdae27de7c5c114713f7777c55aa4111b19e03ee5ced54f",
        app_version: "0.11.0",
        create_sql: r#"CREATE TABLE IF NOT EXISTS input_history (
        id text primary key,
        project_id text not null,
        session_id text,
        text text not null,
        kind text not null,
        time_created integer not null
      , attachments text);
CREATE INDEX IF NOT EXISTS input_history_project_time_idx
        on input_history(project_id, time_created desc, id desc);
CREATE INDEX IF NOT EXISTS input_history_time_idx
        on input_history(time_created desc, id desc);"#,
        add_columns: INPUT_HISTORY_ATTACHMENTS_COLUMNS,
    },
    MigrationDefinition {
        id: "0007_workflow_script_runtime",
        checksum: "0068fc4bcaffe4de4669442a62eee227726e0e8e81b61c7458213a20ac596009",
        app_version: "0.13.0",
        create_sql: r#"CREATE TABLE IF NOT EXISTS workflow_definition (
        id text primary key,
        name text not null,
        source text not null check(source in ('builtin', 'user')),
        trusted integer not null default 0 check(trusted in (0, 1)),
        enabled integer not null default 1 check(enabled in (0, 1)),
        script_path text,
        script_hash text not null,
        meta_json text not null,
        time_created integer not null,
        time_updated integer not null
      , scope text not null default 'explicit'
        check(scope in ('builtin', 'explicit', 'project', 'user')));
CREATE TABLE IF NOT EXISTS workflow_run (
        id text primary key,
        definition_id text,
        name text not null,
        kind text not null default 'script',
        parent_session_id text references session(id) on delete set null,
        cwd text not null,
        script_path text,
        script_hash text not null,
        args_json text,
        args_hash text,
        status text not null check(status in (
          'pending',
          'running',
          'paused',
          'completed',
          'failed',
          'cancelled'
        )),
        current_phase text,
        budget_total integer,
        budget_spent integer not null default 0,
        stats_json text,
        failure_json text,
        time_created integer not null,
        time_started integer,
        time_updated integer not null,
        time_completed integer
      );
CREATE TABLE IF NOT EXISTS workflow_activity (
        id text primary key,
        run_id text not null references workflow_run(id) on delete cascade,
        parent_activity_id text,
        call_index integer not null,
        call_path text not null,
        attempt integer not null default 1,
        type text not null,
        phase text,
        label text,
        input_hash text not null,
        prompt text,
        opts_json text,
        status text not null check(status in (
          'queued',
          'running',
          'completed',
          'failed',
          'skipped',
          'cancelled',
          'cached',
          'lost'
        )),
        child_session_id text references session(id) on delete set null,
        result_json text,
        error_json text,
        time_created integer not null,
        time_started integer,
        time_updated integer not null,
        time_completed integer,
        unique(run_id, call_path, attempt)
      );
CREATE TABLE IF NOT EXISTS workflow_event (
        id text primary key,
        run_id text not null references workflow_run(id) on delete cascade,
        sequence integer not null,
        type text not null,
        phase text,
        activity_id text references workflow_activity(id) on delete set null,
        payload_json text,
        time_created integer not null,
        unique(run_id, sequence)
      );
CREATE TABLE IF NOT EXISTS session_task_link (
        id text primary key,
        root_workflow_run_id text references workflow_run(id) on delete cascade,
        parent_link_id text references session_task_link(id) on delete cascade,
        activity_id text references workflow_activity(id) on delete set null,
        parent_session_id text references session(id) on delete set null,
        child_session_id text not null references session(id) on delete cascade,
        role text not null,
        depth integer not null default 0,
        path text not null,
        phase text,
        label text,
        agent_type text,
        model text,
        status text not null,
        time_created integer not null,
        time_updated integer not null,
        unique(child_session_id)
      );
CREATE INDEX IF NOT EXISTS workflow_definition_source_idx
        on workflow_definition(source, enabled);
CREATE INDEX IF NOT EXISTS workflow_run_cwd_status_idx
        on workflow_run(cwd, status, time_updated desc);
CREATE INDEX IF NOT EXISTS workflow_run_definition_idx
        on workflow_run(definition_id);
CREATE INDEX IF NOT EXISTS workflow_run_parent_session_idx
        on workflow_run(parent_session_id);
CREATE INDEX IF NOT EXISTS workflow_activity_child_session_idx
        on workflow_activity(child_session_id);
CREATE INDEX IF NOT EXISTS workflow_activity_run_status_idx
        on workflow_activity(run_id, status, call_index);
CREATE INDEX IF NOT EXISTS workflow_event_run_sequence_idx
        on workflow_event(run_id, sequence);
CREATE INDEX IF NOT EXISTS session_task_link_activity_idx
        on session_task_link(activity_id);
CREATE INDEX IF NOT EXISTS session_task_link_parent_idx
        on session_task_link(parent_link_id);
CREATE INDEX IF NOT EXISTS session_task_link_root_workflow_idx
        on session_task_link(root_workflow_run_id, depth, path);"#,
        add_columns: NO_ADD_COLUMNS,
    },
    MigrationDefinition {
        id: "0008_workflow_definition_scope",
        checksum: "f7ee304e4005c291fb8883cfc180005263e6c6b2f94077487443f2c17a71d3eb",
        app_version: "0.13.0",
        create_sql: "",
        add_columns: WORKFLOW_DEFINITION_SCOPE_COLUMNS,
    },
    MigrationDefinition {
        id: "0009_session_title_metadata",
        checksum: "3855cf957177ae6319ae91866cdff59e946ca07a10b36b5b59689818bd13fe00",
        app_version: "0.14.0",
        create_sql: "",
        add_columns: SESSION_TITLE_METADATA_COLUMNS,
    },
    MigrationDefinition {
        id: "0010_usage_observability",
        checksum: "36918b0a98f465fe844097aa60c65ef73ea9c62cc266f02742bf3fc2cedf860b",
        app_version: "0.15.0",
        create_sql: r#"CREATE TABLE IF NOT EXISTS turn_usage (
        session_id text not null references session(id) on delete cascade,
        turn_id text not null,
        trace_id text,
        user_message_id text,
        status text not null check(status in ('running', 'completed', 'error', 'cancelled')),
        started_at integer not null,
        first_model_start_at integer,
        first_token_at integer,
        completed_at integer,
        duration_ms integer,
        time_to_first_token_ms integer,
        model_request_count integer not null default 0,
        model_retry_count integer not null default 0,
        tool_call_count integer not null default 0,
        tool_error_count integer not null default 0,
        input_tokens integer not null default 0,
        output_tokens integer not null default 0,
        reasoning_tokens integer not null default 0,
        cache_creation_input_tokens integer not null default 0,
        cache_read_input_tokens integer not null default 0,
        computed_total_tokens integer not null default 0,
        retryable integer not null default 0 check(retryable in (0, 1)),
        cancelled_by_user integer not null default 0 check(cancelled_by_user in (0, 1)),
        context_exceeded integer not null default 0 check(context_exceeded in (0, 1)),
        error_type text,
        error_code text,
        primary key(session_id, turn_id)
      );
CREATE TABLE IF NOT EXISTS model_usage (
        id text primary key,
        logical_request_id text not null,
        attempt_index integer not null default 0,
        session_id text not null references session(id) on delete cascade,
        turn_id text,
        trace_id text,
        span_id text,
        assistant_message_id text,
        parent_user_message_id text,
        query_source text not null,
        provider_id text not null,
        model_id text not null,
        variant text,
        agent text,
        mode text,
        task_type text,
        status text not null check(status in ('running', 'completed', 'error', 'cancelled')),
        started_at integer not null,
        first_token_at integer,
        completed_at integer,
        duration_ms integer,
        time_to_first_token_ms integer,
        finish_reason text,
        tool_call_count integer not null default 0,
        input_tokens integer not null default 0,
        output_tokens integer not null default 0,
        reasoning_tokens integer not null default 0,
        cache_creation_input_tokens integer not null default 0,
        cache_read_input_tokens integer not null default 0,
        provider_total_tokens integer,
        computed_total_tokens integer not null default 0,
        retry_count integer not null default 0,
        retryable integer not null default 0 check(retryable in (0, 1)),
        cancelled_by_user integer not null default 0 check(cancelled_by_user in (0, 1)),
        context_exceeded integer not null default 0 check(context_exceeded in (0, 1)),
        error_type text,
        error_code text,
        error_message text,
        raw_usage_json text,
        provider_metadata_json text
      );
CREATE TABLE IF NOT EXISTS tool_usage (
        id text primary key,
        session_id text not null references session(id) on delete cascade,
        turn_id text,
        trace_id text,
        tool_call_id text not null,
        tool_name text not null,
        side_effect_scope text,
        read_only integer check(read_only in (0, 1)),
        destructive integer check(destructive in (0, 1)),
        approval_status text,
        status text not null check(status in ('running', 'completed', 'error', 'cancelled')),
        started_at integer not null,
        first_output_at integer,
        completed_at integer,
        duration_ms integer,
        time_to_first_output_ms integer,
        exit_code integer,
        output_bytes integer not null default 0,
        stdout_bytes integer not null default 0,
        stderr_bytes integer not null default 0,
        truncated integer not null default 0 check(truncated in (0, 1)),
        retry_count integer not null default 0,
        retryable integer not null default 0 check(retryable in (0, 1)),
        cancelled_by_user integer not null default 0 check(cancelled_by_user in (0, 1)),
        error_type text,
        error_code text,
        error_message text
      );
CREATE INDEX IF NOT EXISTS turn_usage_started_idx
        on turn_usage(started_at);
CREATE INDEX IF NOT EXISTS model_usage_query_source_idx
        on model_usage(query_source);
CREATE INDEX IF NOT EXISTS model_usage_session_turn_idx
        on model_usage(session_id, turn_id);
CREATE INDEX IF NOT EXISTS model_usage_started_model_idx
        on model_usage(started_at, provider_id, model_id);
CREATE INDEX IF NOT EXISTS model_usage_trace_idx
        on model_usage(trace_id);
CREATE UNIQUE INDEX IF NOT EXISTS tool_usage_session_tool_call_idx
        on tool_usage(session_id, tool_call_id);
CREATE INDEX IF NOT EXISTS tool_usage_session_turn_idx
        on tool_usage(session_id, turn_id);
CREATE INDEX IF NOT EXISTS tool_usage_started_tool_idx
        on tool_usage(started_at, tool_name);"#,
        add_columns: NO_ADD_COLUMNS,
    },
    MigrationDefinition {
        id: "0011_session_target_summary_title",
        checksum: "2b7723479426a4e7a1ed9901ed817495c1bbf63e9547eb1c63cd9e24bf9305f8",
        app_version: "0.15.0",
        create_sql: "",
        add_columns: SESSION_TARGET_SUMMARY_TITLE_COLUMNS,
    },
    MigrationDefinition {
        id: "0012_session_trace_id",
        checksum: "9dcef90998dd00c8ed1b22a2170e180eb15471b93947e65ebe101d41e96bdb60",
        app_version: "0.15.0",
        create_sql: "",
        add_columns: SESSION_TRACE_ID_COLUMNS,
    },
    MigrationDefinition {
        id: "0013_session_target_active_run_accounting",
        checksum: "7ab185540ebb7d26c5403ca52a50de6cf161c79cb93ccd47ff1b43b87415fe1c",
        app_version: "0.15.0",
        create_sql: "",
        add_columns: SESSION_TARGET_ACTIVE_RUN_COLUMNS,
    },
    MigrationDefinition {
        id: "0014_message_part_sequence",
        checksum: "66b45c45e4d3a1a60829f193f38d865dcdbd3de2eb78aa79ba954fe7ef1aab08",
        app_version: "0.15.0",
        create_sql: "",
        add_columns: MESSAGE_PART_SEQUENCE_COLUMNS,
    },
    MigrationDefinition {
        id: "0015_message_part_sequence_backfill_and_guard",
        checksum: "da3046bf061ebb5ba253bb772f0fc9e4d1f2856dac4cdbc4cc0a65aae00e8511",
        app_version: "0.15.2",
        create_sql: r#"CREATE TRIGGER IF NOT EXISTS message_sequence_autofill
      after insert on message
      when new.sequence is null
      begin
        update message
        set sequence = (
          select coalesce(max(sequence), -1) + 1
          from message
          where session_id = new.session_id
        )
        where id = new.id;
      end;
CREATE TRIGGER IF NOT EXISTS part_sequence_autofill
      after insert on part
      when new.sequence is null
      begin
        update part
        set sequence = (
          select coalesce(max(sequence), -1) + 1
          from part
          where message_id = new.message_id
        )
        where id = new.id;
      end;"#,
        add_columns: NO_ADD_COLUMNS,
    },
    MigrationDefinition {
        id: "0016_session_input_ledger",
        checksum: "18d51ae3f5e1425dc1e5c809282129fdc7430cacb6b1ce517b412ddbc34be790",
        app_version: "0.15.2",
        create_sql: r#"CREATE TABLE IF NOT EXISTS session_input (
        id text primary key,
        session_id text not null references session(id) on delete cascade,
        kind text not null,
        delivery text not null check(delivery in ('startNow', 'guide', 'queue')),
        payload text not null,
        admitted_sequence integer not null,
        promoted_sequence integer,
        promoted_message_id text,
        status text not null check(status in ('admitted', 'promoted', 'cancelled', 'discarded', 'failed')),
        status_reason text,
        time_created integer not null,
        time_updated integer not null
      );
CREATE INDEX IF NOT EXISTS session_input_session_admitted_idx
        on session_input(session_id, admitted_sequence);
CREATE INDEX IF NOT EXISTS session_input_session_status_idx
        on session_input(session_id, status);"#,
        add_columns: NO_ADD_COLUMNS,
    },
    MigrationDefinition {
        id: "0017_session_input_start_now_delivery",
        checksum: "8c2da5985ecdf342438a2df713c9e18114276a0e85ce6f2e4f79bdf1596b52f8",
        app_version: "0.15.2",
        create_sql: "",
        add_columns: NO_ADD_COLUMNS,
    },
    MigrationDefinition {
        id: "0018_session_input_failed_status",
        checksum: "a4d1a7b7c5d4af426b695769ed0f3a031efac8aa7af1f6412a85292c42b5d15b",
        app_version: "0.15.2",
        create_sql: "",
        add_columns: NO_ADD_COLUMNS,
    },
    MigrationDefinition {
        id: "0019_dwf_journal",
        checksum: "1c8da5568d2357f25c392be486fae80e1f3ad3ebc830dab1bee941634573e8ba",
        app_version: "0.16.5",
        create_sql: r#"CREATE TABLE IF NOT EXISTS dwf_run (
        id text primary key,
        parent_session_id text,
        cwd text,
        name text,
        script_text text,
        script_hash text,
        args_json text,
        tool_call_id text,
        resumed_from text,
        caps_max_concurrency integer not null,
        spent_tokens integer not null default 0,
        status text not null check(status in (
          'pending',
          'running',
          'completed',
          'failed',
          'cancelled'
        )),
        result_json text,
        failure_json text,
        time_created integer not null,
        time_updated integer not null
      );
CREATE TABLE IF NOT EXISTS dwf_actor (
        id integer primary key autoincrement,
        run_id text not null references dwf_run(id) on delete cascade,
        site_id text not null,
        ordinal integer not null,
        name text,
        persona_json text,
        resolved_model text,
        session_id text,
        time_created integer not null,
        time_updated integer not null,
        unique(run_id, site_id, ordinal)
      );
CREATE TABLE IF NOT EXISTS dwf_node (
        id integer primary key autoincrement,
        run_id text not null references dwf_run(id) on delete cascade,
        site_id text not null,
        ordinal integer not null,
        kind text not null check(kind in ('ask', 'world-read', 'world-run', 'report', 'artifact')),
        actor_site_id text,
        actor_ordinal integer,
        actor_seq integer,
        input_hash text not null,
        input_json text,
        status text not null check(status in ('running', 'completed', 'failed')),
        result_json text,
        error_json text,
        stats_json text,
        message_boundary integer,
        artifact_id text,
        time_created integer not null,
        time_updated integer not null,
        unique(run_id, site_id, ordinal)
      );
CREATE TABLE IF NOT EXISTS dwf_event (
        id integer primary key autoincrement,
        run_id text not null references dwf_run(id) on delete cascade,
        sequence integer not null,
        type text not null,
        payload_json text not null,
        time_created integer not null,
        unique(run_id, sequence)
      );
CREATE INDEX IF NOT EXISTS dwf_run_cwd_idx on dwf_run(cwd, time_updated);
CREATE INDEX IF NOT EXISTS dwf_actor_run_idx on dwf_actor(run_id);
CREATE INDEX IF NOT EXISTS dwf_node_artifact_idx on dwf_node(run_id, artifact_id);
CREATE INDEX IF NOT EXISTS dwf_node_run_idx on dwf_node(run_id);
CREATE INDEX IF NOT EXISTS dwf_event_artifact_idx
        on dwf_event(run_id, json_extract(payload_json, '$.artifactId'), sequence);"#,
        add_columns: NO_ADD_COLUMNS,
    },
    MigrationDefinition {
        id: "0020_provider_model_selection",
        checksum: "681180b4fcb497e13289b6970021678f24c975efdf3538777541105c5b2b444e",
        app_version: "0.16.5",
        create_sql: "",
        add_columns: NO_ADD_COLUMNS,
    },
    MigrationDefinition {
        id: "0021_official_glm_selection",
        checksum: "433a8da454682406e962b043f2e0f412e9801bf7e4641ff25760aa16625ebafe",
        app_version: "0.16.5",
        create_sql: "",
        add_columns: NO_ADD_COLUMNS,
    },
    MigrationDefinition {
        id: "0022_backfilled_session_reasoning",
        checksum: "aee472fe499e9c25d2e71df0fbc68d92e462b30d1d02919ca35d540bf133c99a",
        app_version: "0.16.5",
        create_sql: "",
        add_columns: NO_ADD_COLUMNS,
    },
];

/// 冻结清单之外的账本行不校验、不重写（`tasks-db` inspect 口径）。
static NO_ADD_COLUMNS: &[(&str, &str, &str)] = &[];

/// 冻结迁移清单（供门帧 `total` 等使用）。
pub fn migration_definitions() -> &'static [MigrationDefinition] {
    MIGRATIONS
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

/// 账本 id 的合法形状（`tasks-db` 同款；非法 id 不作为基线上报）。
fn is_valid_migration_id(value: &str) -> bool {
    let mut length = 0;
    for byte in value.bytes() {
        let valid = byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-';
        if !valid {
            return false;
        }
        length += 1;
    }
    (1..=128).contains(&length)
}

fn table_exists(connection: &Connection, name: &str) -> Result<bool, StoreError> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
            params![name],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false))
}

fn table_has_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, StoreError> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn checksum_for_id(connection: &Connection, id: &str) -> Result<Option<String>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT checksum FROM schema_migration WHERE id=?",
            params![id],
            |row| row.get(0),
        )
        .optional()?)
}

fn ledger_row_count(connection: &Connection) -> Result<i64, StoreError> {
    Ok(
        connection.query_row("SELECT COUNT(*) FROM schema_migration", [], |row| {
            row.get(0)
        })?,
    )
}

/// 账本表与 `sqlite_%` 之外是否有业务表。
fn has_other_tables(connection: &Connection) -> Result<bool, StoreError> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name NOT IN ('schema_migration') \
             AND name NOT LIKE 'sqlite_%' LIMIT 1",
            [],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false))
}

fn checksum_mismatch(id: &str) -> StoreError {
    StoreError::StorageSchemaUnsupported {
        reason: format!("迁移 {id} 的 checksum 与冻结清单不一致"),
    }
}

/// 预检：决策 kind + 逐条比对冻结 checksum。不授权执行；锁内 runner 仍复查。
///
/// 决策表（`agent-c2-context-memory.md` §6.1）：空 schema → initialize；非空
/// schema 且账本缺失/为空 → fail-closed（非本 runner 建的库不做修复，保持 C1）；
/// 账本有行 → upgrade（缺失步补跑）。
fn inspect_migrations(connection: &Connection) -> Result<MigrationFacts, StoreError> {
    let has_ledger = table_exists(connection, SCHEMA_MIGRATION_TABLE)?;
    let mut pending = !has_ledger;
    if has_ledger {
        for definition in MIGRATIONS {
            match checksum_for_id(connection, definition.id)? {
                None => pending = true,
                Some(applied) if applied != definition.checksum => {
                    return Err(checksum_mismatch(definition.id));
                }
                Some(_) => {}
            }
        }
    }
    let kind = if !pending {
        MigrationKind::None
    } else if has_other_tables(connection)? {
        if !has_ledger {
            return Err(StoreError::StorageSchemaUnsupported {
                reason: format!(
                    "缺少 {SCHEMA_MIGRATION_TABLE} 迁移账本，数据根不是已迁移库（Rust 不修复无账本库）"
                ),
            });
        }
        if ledger_row_count(connection)? == 0 {
            return Err(StoreError::StorageSchemaUnsupported {
                reason: format!(
                    "{SCHEMA_MIGRATION_TABLE} 账本为空且已有业务表，不是可创建的空数据根"
                ),
            });
        }
        MigrationKind::Upgrade
    } else {
        MigrationKind::Initialize
    };
    Ok(MigrationFacts {
        kind,
        executed_count: 0,
        committed_count: 0,
        last_applied_migration_id: None,
    })
}

fn apply_definition(
    connection: &Connection,
    definition: &MigrationDefinition,
) -> Result<(), StoreError> {
    if !definition.create_sql.trim().is_empty() {
        connection.execute_batch(definition.create_sql)?;
    }
    for &(table, column, column_definition) in definition.add_columns {
        if !table_exists(connection, table)? || table_has_column(connection, table, column)? {
            continue;
        }
        connection.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {column_definition}"
        ))?;
    }
    Ok(())
}

/// 打开路径上的迁移入口（`SessionStore::open_with_progress` 调用）。
///
/// 事件顺序：`Inspected` →（有缺失步时）每步一个 `Migrating` → `Committing`。
/// 事务失败回滚（无半截 schema）；busy 耗尽报 `storage_locked`。
pub(crate) fn run_open_migrations(
    connection: &Connection,
    path: &Path,
    on_event: &mut dyn FnMut(MigrationEvent),
) -> Result<MigrationFacts, StoreError> {
    let result = open_and_migrate(connection, on_event);
    match result {
        Ok(facts) => Ok(facts),
        Err(error) => {
            // 回滚失败不覆盖首因。
            if !connection.is_autocommit() {
                let _ = connection.execute_batch("ROLLBACK");
            }
            Err(map_busy_error(path, error))
        }
    }
}

fn open_and_migrate(
    connection: &Connection,
    on_event: &mut dyn FnMut(MigrationEvent),
) -> Result<MigrationFacts, StoreError> {
    let mut facts = inspect_migrations(connection)?;
    on_event(MigrationEvent::Inspected {
        facts: facts.clone(),
    });
    if facts.kind == MigrationKind::Initialize {
        // 持久 pragma（fixture §5.3 的 journal_mode=wal）；已 wal 时是 no-op。
        enable_wal_journal_mode(connection)?;
    }
    connection.execute_batch("BEGIN IMMEDIATE")?;
    run_locked(connection, &mut facts, on_event)
}

/// `PRAGMA journal_mode=wal` 的有界重试。
///
/// journal 模式转换不走 busy handler：并发首启时另一个连接正在转换（或持有
/// EXCLUSIVE）会立即返回 `SQLITE_BUSY`。转换是持久属性且两个创建者意图一致，
/// 重试到对方完成即可；最终仍非 wal 则显式报错，不静默降级。
fn enable_wal_journal_mode(connection: &Connection) -> Result<(), StoreError> {
    for _ in 0..50 {
        match connection.execute_batch("PRAGMA journal_mode=wal;") {
            Ok(()) => return Ok(()),
            Err(rusqlite::Error::SqliteFailure(code, _))
                if matches!(
                    code.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) =>
            {
                // 对方的转换在毫秒级落地；等待后重读，已是 wal 就直接继续。
                std::thread::sleep(std::time::Duration::from_millis(10));
                let current: String = connection
                    .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                    .map_err(StoreError::Sqlite)?;
                if current.eq_ignore_ascii_case("wal") {
                    return Ok(());
                }
            }
            Err(error) => return Err(StoreError::Sqlite(error)),
        }
    }
    Err(StoreError::StorageSchemaUnsupported {
        reason: "并发首启下无法把 journal_mode 切换为 wal（持续 SQLITE_BUSY）".to_owned(),
    })
}

/// 锁内主流程：建账本 → 基线 → 逐条补缺失步（执行前发 `Migrating`）→
/// `Committing` → `COMMIT`。并发首启时后到者在这里重读到完整账本并全部跳过。
fn run_locked(
    connection: &Connection,
    facts: &mut MigrationFacts,
    on_event: &mut dyn FnMut(MigrationEvent),
) -> Result<MigrationFacts, StoreError> {
    // 锁内复查：预检与拿锁之间账本可能被并发创建者写满。
    *facts = inspect_migrations(connection)?;
    connection.execute_batch(LEDGER_DDL)?;
    let baseline: Option<String> = connection
        .query_row(
            "SELECT id FROM schema_migration ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    facts.last_applied_migration_id = baseline.filter(|value| is_valid_migration_id(value));

    let mut last_time_applied: i64 = 0;
    for definition in MIGRATIONS {
        match checksum_for_id(connection, definition.id)? {
            Some(applied) if applied != definition.checksum => {
                return Err(checksum_mismatch(definition.id));
            }
            Some(_) => continue,
            None => {}
        }
        if facts.kind == MigrationKind::None {
            facts.kind = MigrationKind::Upgrade;
        }
        on_event(MigrationEvent::Migrating {
            migration_id: definition.id,
            facts: facts.clone(),
        });
        apply_definition(connection, definition)?;
        facts.executed_count += 1;
        // 同事务内非递减（fixture 只断言单调性）。
        let time_applied = now_millis().max(last_time_applied);
        last_time_applied = time_applied;
        connection.execute(
            "INSERT INTO schema_migration (id, checksum, app_version, time_applied) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                definition.id,
                definition.checksum,
                definition.app_version,
                time_applied
            ],
        )?;
    }
    on_event(MigrationEvent::Committing {
        facts: facts.clone(),
    });
    connection.execute_batch("COMMIT")?;
    facts.committed_count = facts.executed_count;
    Ok(facts.clone())
}

/// 迁移写事务的 busy 映射与 `SessionStore::with_write_transaction` 一致。
fn map_busy_error(path: &Path, error: StoreError) -> StoreError {
    if let StoreError::Sqlite(rusqlite::Error::SqliteFailure(code, _)) = &error {
        if matches!(
            code.code,
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
        ) {
            return StoreError::StorageLocked {
                path: path.to_path_buf(),
            };
        }
    }
    error
}
