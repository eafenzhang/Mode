//! `dwf_run` / `dwf_event` 写入（C4 T3/T4）。
//!
//! 行为契约见 `docs/specs/agent-c4-contract.md` §4：run 行是唯一事实源，
//! event 按 `sequence` 追加（run-started → run-launched → run-settled）。
//! 列清单来自 `scripts/sqlite-schema-baseline.json` 的黑盒 schema。

use rusqlite::params;

use crate::error::StoreError;
use crate::SessionStore;

/// `dwf_run` 一行（插入用；settled 更新走 [`SessionStore::settle_workflow_run`]）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkflowRunInsert {
    pub id: String,
    pub parent_session_id: Option<String>,
    pub cwd: Option<String>,
    pub name: Option<String>,
    pub script_text: Option<String>,
    pub script_hash: Option<String>,
    pub args_json: Option<String>,
    pub tool_call_id: Option<String>,
    pub resumed_from: Option<String>,
    pub caps_max_concurrency: Option<i64>,
    pub spent_tokens: Option<i64>,
    pub status: Option<String>,
    pub result_json: Option<String>,
    pub failure_json: Option<String>,
    pub time_created: Option<i64>,
    pub time_updated: Option<i64>,
}

/// `dwf_event` 一行。
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowEventInsert {
    pub run_id: String,
    pub sequence: i64,
    pub kind: String,
    pub payload_json: String,
    pub time_created: i64,
}

impl SessionStore {
    /// 插入 `dwf_run`（幂等 upsert：重导同 id 覆盖）。
    pub fn insert_workflow_run(&self, run: &WorkflowRunInsert) -> Result<(), StoreError> {
        let mut connection = self.connection();
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(StoreError::from)?;
        transaction
            .execute(
                "insert into dwf_run (id, parent_session_id, cwd, name, script_text, script_hash, \
                 args_json, tool_call_id, resumed_from, caps_max_concurrency, spent_tokens, \
                 status, result_json, failure_json, time_created, time_updated)
                 values (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
                 on conflict(id) do update set
                   parent_session_id=excluded.parent_session_id,
                   cwd=excluded.cwd,
                   name=excluded.name,
                   script_text=excluded.script_text,
                   script_hash=excluded.script_hash,
                   args_json=excluded.args_json,
                   tool_call_id=excluded.tool_call_id,
                   resumed_from=excluded.resumed_from,
                   caps_max_concurrency=excluded.caps_max_concurrency,
                   spent_tokens=excluded.spent_tokens,
                   status=excluded.status,
                   result_json=excluded.result_json,
                   failure_json=excluded.failure_json,
                   time_created=excluded.time_created,
                   time_updated=excluded.time_updated",
                params![
                    run.id,
                    run.parent_session_id,
                    run.cwd,
                    run.name,
                    run.script_text,
                    run.script_hash,
                    run.args_json,
                    run.tool_call_id,
                    run.resumed_from,
                    run.caps_max_concurrency,
                    run.spent_tokens,
                    run.status,
                    run.result_json,
                    run.failure_json,
                    run.time_created,
                    run.time_updated,
                ],
            )
            .map_err(StoreError::from)?;
        transaction
            .commit()
            .map_err(StoreError::from)
    }

    /// 追加 `dwf_event`（sequence 由调用方分配，run 内单调）。
    pub fn insert_workflow_event(&self, event: &WorkflowEventInsert) -> Result<(), StoreError> {
        let mut connection = self.connection();
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(StoreError::from)?;
        transaction
            .execute(
                "insert into dwf_event (run_id, sequence, type, payload_json, time_created)
                 values (?1,?2,?3,?4,?5)",
                params![
                    event.run_id,
                    event.sequence,
                    event.kind,
                    event.payload_json,
                    event.time_created,
                ],
            )
            .map_err(StoreError::from)?;
        transaction
            .commit()
            .map_err(StoreError::from)
    }

    /// 结算 run：status/result/failure/time_updated/spent_tokens 原位更新。
    pub fn settle_workflow_run(
        &self,
        run_id: &str,
        status: &str,
        result_json: Option<&str>,
        failure_json: Option<&str>,
        spent_tokens: i64,
        time_updated: i64,
    ) -> Result<(), StoreError> {
        let connection = self.connection();
        connection
            .execute(
                "update dwf_run set status=?2, result_json=?3, failure_json=?4, \
                 spent_tokens=?5, time_updated=?6 where id=?1",
                params![
                    run_id,
                    status,
                    result_json,
                    failure_json,
                    spent_tokens,
                    time_updated
                ],
            )
            .map_err(StoreError::from)?;
        Ok(())
    }
}
