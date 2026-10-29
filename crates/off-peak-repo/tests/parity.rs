//! 与 TS 的黄金数据对拍：`off_peak_tasks` 状态机与调度认领。
//!
//! fixture 由 `pnpm tsx scripts/generate-off-peak-fixtures.ts` 用真实 TS
//! `OffPeakTaskRepo` 生成；Rust 侧重建同一 DB 并逐步重放。所有写入口都显式传
//! `now`，时间列可直接逐字节比较。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde_json::{json, Map, Value};
use off_peak_repo::{
    CreateOptions, CreateParams, EditableFields, EditableModelSelection, InvalidateOptions,
    MarkRunningOptions, MarkTerminalOptions, ModelSelection, OffPeakTaskRepo,
    ReleaseClaimOptions, SchedulingPatch, SelectionOptions, TaskScope,
};
use tasks_db::RunOptions;

const FIXTURE: &str = include_str!("../fixtures/off-peak.json");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("fixture 必须是合法 JSON")
}

fn string_at(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{key} 必须是字符串"))
        .to_owned()
}

fn optional_string_at(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn int_at(value: &Value, key: &str) -> i64 {
    value
        .get(key)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("{key} 必须是整数"))
}

fn optional_int_at(value: &Value, key: &str) -> Option<i64> {
    match value.get(key) {
        None | Some(Value::Null) => None,
        Some(value) => value.as_i64(),
    }
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir() -> TempDir {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let path = std::env::temp_dir().join(format!(
        "off-peak-parity-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("创建临时目录");
    TempDir(path)
}

fn seed_database(connection: &Connection) {
    let mut insert_task = connection
        .prepare(
            "INSERT INTO tasks (
               workspace_key, workspace_path, workspace_identity, task_id, title, task_status,
               provider, mode, created_at, updated_at, meta_json
             ) VALUES (?1, ?1, NULL, ?2, ?3, 'running', 'glm', 'build', 1, 1, '{}')",
        )
        .expect("prepare task insert");
    insert_task
        .execute(params!["D:/ws-alpha", "session-bound", "bound session title"])
        .expect("insert task");
    insert_task
        .execute(params!["D:/ws-alpha", "session-empty-title", ""])
        .expect("insert task");

    let mut insert_legacy = connection
        .prepare(
            "INSERT INTO off_peak_tasks (
               off_peak_task_id, server_ticket_id, title, conversation_id, session_id,
               prompt, permission_mode, model, thought_level, model_selection,
               workspace_key, workspace_path, workspace_identity,
               status, queued_at, started_at, claim_running, attempt_count, created_at, updated_at
             ) VALUES (?1, NULL, ?2, NULL, ?3, 'legacy prompt', 'build', 'legacy-model', 'low', ?4,
               'D:/ws-alpha', 'D:/ws-alpha', NULL, ?5, 10, ?6, 0, 0, 10, 10)",
        )
        .expect("prepare legacy insert");
    insert_legacy
        .execute(params![
            "legacy-null-selection",
            "legacy legacy-null-selection",
            "session-bound",
            None::<String>,
            "queued",
            None::<i64>
        ])
        .expect("insert legacy");
    insert_legacy
        .execute(params![
            "legacy-bad-json",
            "legacy legacy-bad-json",
            None::<String>,
            "{oops",
            "queued",
            None::<i64>
        ])
        .expect("insert legacy");
    insert_legacy
        .execute(params![
            "legacy-running-started",
            "legacy legacy-running-started",
            None::<String>,
            r#"{"providerId":"zai-api","modelId":"glm-4.6"}"#,
            "running",
            Some(20_i64)
        ])
        .expect("insert legacy");
}

fn sql_to_json(value: rusqlite::types::Value) -> Value {
    match value {
        rusqlite::types::Value::Null => Value::Null,
        rusqlite::types::Value::Integer(value) => json!(value),
        rusqlite::types::Value::Real(value) => json!(value),
        rusqlite::types::Value::Text(value) => json!(value),
        rusqlite::types::Value::Blob(value) => json!(value),
    }
}

fn snapshot(db_path: &Path) -> Value {
    let connection = Connection::open(db_path).expect("打开快照连接");
    let off_peak_tasks = {
        let mut statement = connection
            .prepare("SELECT * FROM off_peak_tasks ORDER BY off_peak_task_id")
            .expect("prepare off-peak snapshot");
        let columns: Vec<String> = statement
            .column_names()
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        statement
            .query_map([], |row| {
                let mut object = Map::new();
                for (index, name) in columns.iter().enumerate() {
                    object.insert(name.clone(), sql_to_json(row.get(index)?));
                }
                Ok(Value::Object(object))
            })
            .expect("query off-peak snapshot")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect off-peak snapshot")
    };
    let tasks = {
        let mut statement = connection
            .prepare(
                "SELECT workspace_key, workspace_path, workspace_identity, task_id, title
                 FROM tasks ORDER BY workspace_key, task_id",
            )
            .expect("prepare tasks snapshot");
        statement
            .query_map([], |row| {
                Ok(json!({
                    "workspace_key": row.get::<_, String>(0)?,
                    "workspace_path": row.get::<_, String>(1)?,
                    "workspace_identity": row.get::<_, Option<String>>(2)?,
                    "task_id": row.get::<_, String>(3)?,
                    "title": row.get::<_, String>(4)?,
                }))
            })
            .expect("query tasks snapshot")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect tasks snapshot")
    };
    json!({ "offPeakTasks": off_peak_tasks, "tasks": tasks })
}

fn selection_from_json(value: &Value) -> ModelSelection {
    let options = value.get("options").and_then(Value::as_object).map(|options| {
        SelectionOptions {
            reasoning_level: options
                .get("reasoningLevel")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }
    });
    ModelSelection {
        provider_id: string_at(value, "providerId"),
        model_id: string_at(value, "modelId"),
        options,
    }
}

fn create_params(input: &Value) -> CreateParams {
    CreateParams {
        title: string_at(input, "title"),
        prompt: string_at(input, "prompt"),
        permission_mode: string_at(input, "permissionMode"),
        model_selection: selection_from_json(&input["modelSelection"]),
        workspace_path: string_at(input, "workspacePath"),
        workspace_identity: optional_string_at(input, "workspaceIdentity"),
        bound_session_id: optional_string_at(input, "boundSessionId"),
    }
}

fn create_options(input: &Value) -> CreateOptions {
    CreateOptions {
        now: optional_int_at(input, "now"),
        off_peak_task_id: optional_string_at(input, "offPeakTaskId"),
        server_ticket_id: optional_string_at(input, "serverTicketId"),
        queue_position: optional_int_at(input, "queuePosition"),
        registered_at: optional_int_at(input, "registeredAt"),
        schedulable: input.get("schedulable").and_then(Value::as_bool),
    }
}

fn editable_fields(input: &Value) -> EditableFields {
    let model_selection = match input.get("modelSelection") {
        None => None,
        Some(Value::Null) => Some(EditableModelSelection::Clear),
        Some(value) => Some(EditableModelSelection::Replace(selection_from_json(value))),
    };
    EditableFields {
        title: optional_string_at(input, "title"),
        prompt: optional_string_at(input, "prompt"),
        permission_mode: optional_string_at(input, "permissionMode"),
        model_selection,
    }
}

fn scheduling_patch(input: &Value) -> SchedulingPatch {
    SchedulingPatch {
        schedulable: input.get("schedulable").and_then(Value::as_bool),
        queue_position: match input.get("queuePosition") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(value) => Some(value.as_i64()),
        },
        next_poll_at: match input.get("nextPollAt") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(value) => Some(value.as_i64()),
        },
        server_ticket_id: optional_string_at(input, "serverTicketId"),
        registered_at: optional_int_at(input, "registeredAt"),
        now: optional_int_at(input, "now"),
    }
}

fn dispatch(repo: &OffPeakTaskRepo, op: &str, input: &Value) -> off_peak_repo::Result<Value> {
    Ok(match op {
        "create" => serde_json::to_value(repo.create(
            &create_params(&input["params"]),
            &create_options(&input["options"]),
        )?)?,
        "list" => serde_json::to_value(repo.list(&TaskScope {
            workspace_path: optional_string_at(input, "workspacePath"),
            workspace_identity: optional_string_at(input, "workspaceIdentity"),
        })?)?,
        "get" => serde_json::to_value(repo.get(&string_at(input, "offPeakTaskId"))?)?,
        "countNonTerminal" => json!(repo.count_non_terminal()?),
        "countActive" => json!(repo.count_active()?),
        "hasActiveBoundTask" => json!(repo.has_active_bound_task(
            &string_at(input, "workspaceKey"),
            &string_at(input, "sessionId"),
        )?),
        "updateEditableFields" => serde_json::to_value(repo.update_editable_fields(
            &string_at(input, "offPeakTaskId"),
            &editable_fields(&input["fields"]),
            &InvalidateOptions {
                now: optional_int_at(&input["options"], "now"),
            },
        )?)?,
        "updateSchedulingSnapshot" => {
            repo.update_scheduling_snapshot(
                &string_at(input, "offPeakTaskId"),
                &scheduling_patch(&input["patch"]),
            )?;
            Value::Null
        }
        "claimDue" => serde_json::to_value(repo.claim_due(int_at(input, "now"))?)?,
        "markRunning" => serde_json::to_value(repo.mark_running(
            &string_at(input, "offPeakTaskId"),
            &MarkRunningOptions {
                started_at: int_at(&input["options"], "startedAt"),
                conversation_id: optional_string_at(&input["options"], "conversationId"),
                session_id: optional_string_at(&input["options"], "sessionId"),
                server_ticket_id: optional_string_at(&input["options"], "serverTicketId"),
            },
        )?)?,
        "markTerminal" => serde_json::to_value(repo.mark_terminal(
            &string_at(input, "offPeakTaskId"),
            &MarkTerminalOptions {
                status: string_at(&input["options"], "status"),
                ended_at: int_at(&input["options"], "endedAt"),
                failure_reason: optional_string_at(&input["options"], "failureReason"),
                files_changed: optional_int_at(&input["options"], "filesChanged"),
                dispatch_error: optional_string_at(&input["options"], "dispatchError"),
            },
        )?)?,
        "releaseClaim" => {
            repo.release_claim(
                &string_at(input, "offPeakTaskId"),
                &ReleaseClaimOptions {
                    error: optional_string_at(&input["options"], "error"),
                    now: optional_int_at(&input["options"], "now"),
                },
            )?;
            Value::Null
        }
        "recoverInterrupted" => json!(repo.recover_interrupted(int_at(input, "now"))?),
        "requeueForContinuation" => serde_json::to_value(repo.requeue_for_continuation(
            &string_at(input, "offPeakTaskId"),
            &InvalidateOptions {
                now: optional_int_at(&input["options"], "now"),
            },
        )?)?,
        "setPaused" => serde_json::to_value(repo.set_paused(
            &string_at(input, "offPeakTaskId"),
            input.get("paused").and_then(Value::as_bool).expect("paused"),
            &InvalidateOptions {
                now: optional_int_at(&input["options"], "now"),
            },
        )?)?,
        "invalidateModelSelection" => serde_json::to_value(repo.invalidate_model_selection(
            &string_at(input, "offPeakTaskId"),
            &selection_from_json(&input["selection"]),
            &InvalidateOptions {
                now: optional_int_at(&input["options"], "now"),
            },
        )?)?,
        "markHistoryDeleted" => serde_json::to_value(repo.mark_history_deleted(
            &string_at(input, "offPeakTaskId"),
            &InvalidateOptions {
                now: optional_int_at(&input["options"], "now"),
            },
        )?)?,
        "markSettled" => {
            repo.mark_settled(
                &string_at(input, "offPeakTaskId"),
                int_at(input, "settledAt"),
            )?;
            Value::Null
        }
        "listNonTerminal" => serde_json::to_value(repo.list_non_terminal()?)?,
        "listUnsettledTerminal" => serde_json::to_value(repo.list_unsettled_terminal()?)?,
        "delete" => {
            repo.delete(&string_at(input, "offPeakTaskId"))?;
            Value::Null
        }
        "close" => {
            repo.close();
            Value::Null
        }
        other => panic!("未知 op: {other}"),
    })
}

fn first_difference(expected: &Value, actual: &Value, path: &str) -> Option<String> {
    match (expected, actual) {
        (Value::Object(left), Value::Object(right)) => {
            for key in left.keys().chain(right.keys()) {
                let next = format!("{path}/{key}");
                match (left.get(key), right.get(key)) {
                    (Some(left), Some(right)) => {
                        if let Some(difference) = first_difference(left, right, &next) {
                            return Some(difference);
                        }
                    }
                    (left, right) => return Some(format!("{next}: ts={left:?} rust={right:?}")),
                }
            }
            None
        }
        (Value::Array(left), Value::Array(right)) => {
            for index in 0..left.len().min(right.len()) {
                let next = format!("{path}[{index}]");
                if let Some(difference) = first_difference(&left[index], &right[index], &next) {
                    return Some(difference);
                }
            }
            if left.len() != right.len() {
                Some(format!("{path}: length ts={} rust={}", left.len(), right.len()))
            } else {
                None
            }
        }
        (left, right) if left == right => None,
        (left, right) => Some(format!("{path}: ts={left:?} rust={right:?}")),
    }
}

#[test]
fn ts_fixture_steps_match() {
    let fixture = fixture();
    let temp = temp_dir();
    let db_path = temp.0.join("tasks-index.sqlite");

    {
        let connection = Connection::open(&db_path).expect("打开 seed 库");
        tasks_db::run_migrations(&connection, RunOptions::default()).expect("迁移");
        seed_database(&connection);
    }

    let clock = fixture["clockMs"].as_i64().expect("clockMs");
    let repo = OffPeakTaskRepo::with_clock(&db_path, move || clock);

    let steps = fixture["steps"].as_array().expect("steps");
    assert!(!steps.is_empty(), "fixture 不能为空");
    for step in steps {
        let id = step["id"].as_str().expect("step id");
        let op = step["op"].as_str().expect("step op");
        let result = dispatch(&repo, op, &step["input"]);
        match &step["error"] {
            Value::String(expected) => {
                let error = match result {
                    Ok(_) => panic!("{id}: 期望错误但成功了"),
                    Err(error) => error,
                };
                assert_eq!(error.to_string(), *expected, "{id}: 错误文案必须与 TS 一致");
            }
            _ => {
                let output = result.unwrap_or_else(|error| panic!("{id}: 意外错误 {error}"));
                assert_eq!(output, step["output"], "{id}: 方法返回值必须与 TS 深度相等");
            }
        }
        let actual = snapshot(&db_path);
        if actual != step["snapshot"] {
            let detail = first_difference(&step["snapshot"], &actual, "")
                .unwrap_or_else(|| "（无差异定位）".to_owned());
            panic!("{id}: DB 快照必须与 TS 一致\n{detail}");
        }
    }
    repo.close();
}
