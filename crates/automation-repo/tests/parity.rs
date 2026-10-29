//! 与 TS 的黄金数据对拍：`automations` / `automation_runs` 的 CRUD、调度认领与运行历史。
//!
//! fixture 由 `pnpm automation:fixtures` 用真实 TS `AutomationRepo` 生成；Rust 侧
//! 重建同一 DB 并逐步重放。时间源统一固定（TS 生成期 monkey-patch `Date.now`，
//! Rust 侧 `AutomationRepo::with_clock`），主键里的 UUID 在比较前归一。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use automation_repo::{
    AutomationBotDeliveryTarget, AutomationCreateOptions, AutomationCreateParams,
    AutomationRepo, AutomationRestartOptions, AutomationScope, AutomationUpdateOptions,
    AutomationUpdateParams, DispatchFailureKind, EnsureRunClaimedParams, MarkDispatchFailedOptions,
    MarkDispatchedOptions, MarkManualRunDispatchedParams, MarkRunDispatchParams, ModelSelection,
    RecordSkippedRunParams, RunNowOptions, SelectionOptions, SkipAndRescheduleParams,
    UpsertRunClaimedParams,
};
use rusqlite::{params, Connection};
use serde_json::{json, Map, Value};
use tasks_db::RunOptions;

const FIXTURE: &str = include_str!("../fixtures/automation.json");

const DEFAULT_SELECTION: &str =
    r#"{"providerId":"zai-api","modelId":"glm-4.6","options":{"reasoningLevel":"high"}}"#;

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
    value.get(key).and_then(Value::as_str).map(str::to_owned)
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

/// `Option<Option<T>>`：键缺失 = 不改；显式 null = 清空；其余 = 赋值。
fn nullable_int_at(value: &Value, key: &str) -> Option<Option<i64>> {
    match value.get(key) {
        None => None,
        Some(Value::Null) => Some(None),
        Some(value) => Some(Some(value.as_i64().unwrap_or_else(|| panic!("{key} 必须是整数")))),
    }
}

fn nullable_string_at(value: &Value, key: &str) -> Option<Option<String>> {
    match value.get(key) {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(text)) => Some(Some(text.clone())),
        Some(_) => panic!("{key} 必须是字符串或 null"),
    }
}

fn nullable_selection_at(value: &Value, key: &str) -> Option<Option<ModelSelection>> {
    match value.get(key) {
        None => None,
        Some(Value::Null) => Some(None),
        Some(value) => Some(Some(selection_from_json(value))),
    }
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
        "automation-parity-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("创建临时目录");
    TempDir(path)
}

// ── seed（与 TS `seedAutomation` / `seedRun` 逐列对齐）─────────────────────

#[derive(Default)]
struct AutomationSeed {
    title: Option<String>,
    /// `None` = 默认 Selection 文本；`Some(None)` = SQL NULL；`Some(Some(v))` = 原样写入。
    model_selection: Option<Option<String>>,
    mode: Option<String>,
    bot_delivery_target: Option<String>,
    workspace_key: Option<String>,
    workspace_path: Option<String>,
    workspace_identity: Option<String>,
    target_task_id: Option<String>,
    recurring: Option<i64>,
    max_runs: Option<i64>,
    end_at: Option<i64>,
    schedule_rule: Option<String>,
    schedule_edited_by_user: Option<i64>,
    run_count: Option<i64>,
    scheduled_run_count: Option<i64>,
    enabled: Option<i64>,
    lifecycle_status: Option<String>,
    next_run_at: Option<i64>,
    running: Option<i64>,
    claimed_at: Option<i64>,
    dispatch_status: Option<String>,
    dispatch_attempts: Option<i64>,
    retry_at: Option<i64>,
    created_at: i64,
}


fn seed_automation(connection: &Connection, id: &str, seed: AutomationSeed) {
    let workspace_path = seed
        .workspace_path
        .unwrap_or_else(|| "D:/ws-alpha".to_owned());
    let workspace_key = seed.workspace_key.unwrap_or_else(|| workspace_path.clone());
    let model_selection = match seed.model_selection {
        None => Some(DEFAULT_SELECTION.to_owned()),
        Some(value) => value,
    };
    connection
        .execute(
            "INSERT INTO automations (
               automation_id, title, cron_expr, prompt, model, provider, model_selection,
               workspace_key, workspace_path, workspace_identity, target_task_id, bot_delivery_target,
               location_kind, recurring, max_runs, end_at, schedule_rule, schedule_edited_by_user,
               run_count, scheduled_run_count, enabled, lifecycle_status, next_run_at, last_run_at,
               running, claimed_at, dispatch_status, dispatch_attempts, retry_at, last_error,
               mode, thought_level, created_at, updated_at
             ) VALUES (?, ?, '*/5 * * * *', ?, NULL, NULL, ?,
               ?, ?, ?, ?, ?,
               'local', ?, ?, ?, ?, ?,
               ?, ?, ?, ?, ?, NULL,
               ?, ?, ?, ?, ?, NULL,
               ?, 'low', ?, ?)",
            params![
                id,
                seed.title.unwrap_or_else(|| format!("title {id}")),
                format!("prompt {id}"),
                model_selection,
                workspace_key,
                workspace_path,
                seed.workspace_identity,
                seed.target_task_id,
                seed.bot_delivery_target,
                seed.recurring.unwrap_or(1),
                seed.max_runs,
                seed.end_at,
                seed.schedule_rule,
                seed.schedule_edited_by_user.unwrap_or(0),
                seed.run_count.unwrap_or(0),
                seed.scheduled_run_count.unwrap_or(0),
                seed.enabled.unwrap_or(1),
                seed.lifecycle_status.unwrap_or_else(|| "active".to_owned()),
                seed.next_run_at.or(Some(5_000_000)),
                seed.running.unwrap_or(0),
                seed.claimed_at,
                seed.dispatch_status.unwrap_or_else(|| "idle".to_owned()),
                seed.dispatch_attempts.unwrap_or(0),
                seed.retry_at,
                seed.mode.unwrap_or_else(|| "build".to_owned()),
                seed.created_at,
                seed.created_at,
            ],
        )
        .expect("insert automation");
}

fn seed_run(connection: &Connection, run_id: &str, automation_id: &str, scheduled_at: Option<i64>, trigger: &str, created_at: i64) {
    connection
        .execute(
            "INSERT INTO automation_runs (
               run_id, automation_id, workspace_key, scheduled_at, trigger,
               model_selection, dispatch_status, outcome, session_id, error,
               attempts, created_at, updated_at
             ) VALUES (?, ?, 'D:/ws-alpha', ?, ?, NULL, 'claimed', NULL, NULL, NULL,
               0, ?, ?)",
            params![run_id, automation_id, scheduled_at, trigger, created_at, created_at],
        )
        .expect("insert run");
}

fn seed_database(connection: &Connection) {
    seed_automation(
        connection,
        "a-1",
        AutomationSeed {
            title: Some("alpha".to_owned()),
            target_task_id: Some("session-1".to_owned()),
            bot_delivery_target: Some(
                json!({
                    "provider": "feishu",
                    "botId": "bot-1",
                    "providerUserId": "u-1",
                    "chatType": "private",
                })
                .to_string(),
            ),
            schedule_rule: Some(
                json!({
                    "unit": "daily",
                    "interval": 1,
                    "hour": 9,
                    "minute": 0,
                    "anchorAt": 1000,
                })
                .to_string(),
            ),
            schedule_edited_by_user: Some(1),
            run_count: Some(2),
            scheduled_run_count: Some(2),
            created_at: 100,
            ..AutomationSeed::default()
        },
    );
    seed_automation(
        connection,
        "a-2",
        AutomationSeed {
            model_selection: Some(Some("null".to_owned())),
            mode: Some(String::new()),
            recurring: Some(0),
            max_runs: Some(2),
            created_at: 200,
            ..AutomationSeed::default()
        },
    );
    seed_automation(
        connection,
        "a-3",
        AutomationSeed {
            end_at: Some(500),
            created_at: 300,
            ..AutomationSeed::default()
        },
    );
    seed_automation(
        connection,
        "a-4",
        AutomationSeed {
            enabled: Some(0),
            lifecycle_status: Some("paused".to_owned()),
            created_at: 400,
            ..AutomationSeed::default()
        },
    );
    seed_automation(
        connection,
        "a-5",
        AutomationSeed {
            model_selection: Some(None),
            enabled: Some(0),
            lifecycle_status: Some("paused".to_owned()),
            created_at: 500,
            ..AutomationSeed::default()
        },
    );
    seed_automation(
        connection,
        "a-6",
        AutomationSeed {
            bot_delivery_target: Some("{oops".to_owned()),
            retry_at: Some(400_000),
            dispatch_status: Some("failed_to_dispatch".to_owned()),
            dispatch_attempts: Some(1),
            created_at: 600,
            ..AutomationSeed::default()
        },
    );
    seed_automation(
        connection,
        "a-7",
        AutomationSeed {
            dispatch_attempts: Some(4),
            created_at: 700,
            ..AutomationSeed::default()
        },
    );
    seed_automation(
        connection,
        "a-8",
        AutomationSeed {
            running: Some(1),
            claimed_at: Some(100),
            created_at: 800,
            ..AutomationSeed::default()
        },
    );
    seed_automation(
        connection,
        "a-9",
        AutomationSeed {
            created_at: 900,
            ..AutomationSeed::default()
        },
    );
    seed_automation(
        connection,
        "a-10",
        AutomationSeed {
            workspace_key: Some("remote:ssh:box:22:me:/srv/ws".to_owned()),
            workspace_path: Some("/srv/ws".to_owned()),
            workspace_identity: Some("remote:ssh:box:22:me:/srv/ws".to_owned()),
            created_at: 1000,
            ..AutomationSeed::default()
        },
    );
    seed_automation(
        connection,
        "a-11",
        AutomationSeed {
            enabled: Some(0),
            lifecycle_status: Some("paused".to_owned()),
            created_at: 1100,
            ..AutomationSeed::default()
        },
    );
    seed_run(
        connection,
        "a-11:manual:seed",
        "a-11",
        Some(1100),
        "manual",
        1100,
    );
}

// ── 快照与归一 ────────────────────────────────────────────────────────────

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
    let automations = {
        let mut statement = connection
            .prepare("SELECT * FROM automations ORDER BY automation_id")
            .expect("prepare automations snapshot");
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
            .expect("query automations snapshot")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect automations snapshot")
    };
    let automation_runs = {
        let mut statement = connection
            .prepare("SELECT * FROM automation_runs ORDER BY run_id")
            .expect("prepare runs snapshot");
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
            .expect("query runs snapshot")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect runs snapshot")
    };
    let automations = automations
        .into_iter()
        .map(|row| {
            let mut object = row.as_object().cloned().unwrap_or_default();
            for key in ["automation_id", "schedule_rule", "bot_delivery_target"] {
                let replaced = match object.get(key) {
                    Some(Value::String(raw)) if key != "automation_id" => {
                        match serde_json::from_str::<Value>(raw) {
                            Ok(value) => value,
                            Err(_) => json!({ "unparsable": raw }),
                        }
                    }
                    Some(value) => value.clone(),
                    None => Value::Null,
                };
                object.insert(key.to_owned(), replaced);
            }
            Value::Object(object)
        })
        .collect::<Vec<_>>();
    let automation_runs = automation_runs
        .into_iter()
        .map(|row| {
            let mut object = row.as_object().cloned().unwrap_or_default();
            if let Some(Value::String(run_id)) = object.get("run_id").cloned() {
                object.insert("run_id".to_owned(), Value::String(run_id));
            }
            Value::Object(object)
        })
        .collect::<Vec<_>>();
    normalize_value(&json!({
        "automations": automations,
        "automationRuns": automation_runs,
    }))
}

fn is_automation_uuid(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("automation-") else {
        return false;
    };
    let groups = [8usize, 4, 4, 4, 12];
    let parts: Vec<&str> = rest.split('-').collect();
    if parts.len() != groups.len() {
        return false;
    }
    parts
        .iter()
        .zip(groups)
        .all(|(part, len)| part.len() == len && part.chars().all(|ch| ch.is_ascii_hexdigit()))
}

fn normalize_string(value: &str) -> String {
    if is_automation_uuid(value) {
        return "automation-normalized".to_owned();
    }
    if let Some(index) = value.rfind(":manual:") {
        let suffix = &value[index + ":manual:".len()..];
        if suffix.len() == 36
            && suffix
                .chars()
                .all(|ch| ch.is_ascii_hexdigit() || ch == '-')
        {
            return format!("{}:manual:normalized", &value[..index]);
        }
    }
    value.to_owned()
}

fn normalize_value(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(normalize_string(text)),
        Value::Array(items) => Value::Array(items.iter().map(normalize_value).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, child)| (key.clone(), normalize_value(child)))
                .collect(),
        ),
        other => other.clone(),
    }
}

// ── op 派发 ───────────────────────────────────────────────────────────────

fn create_params(input: &Value) -> AutomationCreateParams {
    let bot_delivery_target = input
        .get("botDeliveryTarget")
        .filter(|value| !value.is_null())
        .map(|value| AutomationBotDeliveryTarget {
            provider: string_at(value, "provider"),
            bot_id: string_at(value, "botId"),
            provider_user_id: string_at(value, "providerUserId"),
            chat_type: string_at(value, "chatType"),
        });
    // 非法 mode 在读取其它字段之前就抛错，这几个必填项按缺省构造即可与 TS 对齐。
    AutomationCreateParams {
        title: optional_string_at(input, "title").unwrap_or_default(),
        cron_expr: optional_string_at(input, "cronExpr").unwrap_or_default(),
        prompt: optional_string_at(input, "prompt").unwrap_or_default(),
        model_selection: input
            .get("modelSelection")
            .filter(|value| !value.is_null())
            .map(selection_from_json),
        mode: optional_string_at(input, "mode"),
        workspace_path: optional_string_at(input, "workspacePath").unwrap_or_default(),
        workspace_identity: optional_string_at(input, "workspaceIdentity"),
        target_task_id: optional_string_at(input, "targetTaskId"),
        bot_delivery_target,
        recurring: input.get("recurring").and_then(Value::as_bool).unwrap_or(false),
        max_runs: optional_int_at(input, "maxRuns"),
        end_at: optional_int_at(input, "endAt"),
        schedule_rule: input.get("scheduleRule").filter(|value| !value.is_null()).cloned(),
    }
}

fn create_options(input: &Value) -> AutomationCreateOptions {
    AutomationCreateOptions {
        next_run_at: optional_int_at(input, "nextRunAt"),
        lifecycle_status: optional_string_at(input, "lifecycleStatus"),
    }
}

fn update_params(input: &Value) -> AutomationUpdateParams {
    AutomationUpdateParams {
        title: optional_string_at(input, "title"),
        cron_expr: optional_string_at(input, "cronExpr"),
        prompt: optional_string_at(input, "prompt"),
        model_selection: nullable_selection_at(input, "modelSelection"),
        mode: nullable_string_at(input, "mode"),
        recurring: input.get("recurring").and_then(Value::as_bool),
        max_runs: nullable_int_at(input, "maxRuns"),
        end_at: nullable_int_at(input, "endAt"),
        schedule_rule: match input.get("scheduleRule") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(value) => Some(Some(value.clone())),
        },
        schedule_edited_by_user: input.get("scheduleEditedByUser").and_then(Value::as_bool),
    }
}

fn update_options(input: &Value) -> AutomationUpdateOptions {
    AutomationUpdateOptions {
        next_run_at: nullable_int_at(input, "nextRunAt"),
        lifecycle_status: optional_string_at(input, "lifecycleStatus"),
        reset_retry: input.get("resetRetry").and_then(Value::as_bool).unwrap_or(false),
    }
}

fn dispatch(repo: &AutomationRepo, op: &str, input: &Value) -> automation_repo::Result<Value> {
    Ok(match op {
        "list" => serde_json::to_value(repo.list(&AutomationScope {
            workspace_path: optional_string_at(input, "workspacePath"),
            workspace_identity: optional_string_at(input, "workspaceIdentity"),
        })?)?,
        "get" => serde_json::to_value(repo.get(
            &string_at(input, "automationId"),
            optional_string_at(input, "workspaceKey").as_deref(),
        )?)?,
        "getModelSelectionForDispatch" => serde_json::to_value(
            repo.get_model_selection_for_dispatch(
                &string_at(input, "automationId"),
                &string_at(input, "workspaceKey"),
            )?,
        )?,
        "getBotDeliveryTarget" => serde_json::to_value(repo.get_bot_delivery_target(
            &string_at(input, "automationId"),
            optional_string_at(input, "workspaceKey").as_deref(),
        )?)?,
        "hasTaskBinding" => {
            let identity = optional_string_at(input, "workspaceIdentity");
            json!(repo.has_task_binding(
                &string_at(input, "workspacePath"),
                identity.as_deref(),
                &string_at(input, "targetTaskId"),
            )?)
        }
        "getScheduledRunCount" => serde_json::to_value(repo.get_scheduled_run_count(
            &string_at(input, "automationId"),
            optional_string_at(input, "workspaceKey").as_deref(),
        )?)?,
        "update" => serde_json::to_value(repo.update(
            &string_at(input, "automationId"),
            &update_params(&input["params"]),
            &update_options(&input["options"]),
            optional_string_at(input, "workspaceKey").as_deref(),
        )?)?,
        "delete" => json!(repo.delete(
            &string_at(input, "automationId"),
            optional_string_at(input, "workspaceKey").as_deref(),
        )?),
        "setEnabled" => {
            repo.set_enabled(
                &string_at(input, "automationId"),
                input.get("enabled").and_then(Value::as_bool).expect("enabled"),
                optional_string_at(input, "workspaceKey").as_deref(),
            )?;
            Value::Null
        }
        "restart" => {
            repo.restart(
                &string_at(input, "automationId"),
                &AutomationRestartOptions {
                    next_run_at: optional_int_at(&input["options"], "nextRunAt"),
                },
                optional_string_at(input, "workspaceKey").as_deref(),
            )?;
            Value::Null
        }
        "runNow" => serde_json::to_value(repo.run_now(
            &string_at(input, "automationId"),
            &RunNowOptions {
                now: int_at(&input["options"], "now"),
            },
            optional_string_at(input, "workspaceKey").as_deref(),
        )?)?,
        "claimDue" => serde_json::to_value(repo.claim_due(int_at(input, "now"))?)?,
        "claimManualRuns" => serde_json::to_value(repo.claim_manual_runs(int_at(input, "now"))?)?,
        "markDispatched" => {
            repo.mark_dispatched(
                &string_at(input, "automationId"),
                &MarkDispatchedOptions {
                    dispatched_at: int_at(&input["options"], "dispatchedAt"),
                    next_run_at: optional_int_at(&input["options"], "nextRunAt"),
                },
            )?;
            Value::Null
        }
        "markDispatchFailed" => {
            repo.mark_dispatch_failed(
                &string_at(input, "automationId"),
                &MarkDispatchFailedOptions {
                    failed_at: int_at(&input["options"], "failedAt"),
                    error: string_at(&input["options"], "error"),
                    kind: match string_at(&input["options"], "kind").as_str() {
                        "transient" => DispatchFailureKind::Transient,
                        "permanent" => DispatchFailureKind::Permanent,
                        other => panic!("未知失败类型: {other}"),
                    },
                    next_run_at: optional_int_at(&input["options"], "nextRunAt"),
                },
            )?;
            Value::Null
        }
        "releaseClaim" => {
            repo.release_claim(&string_at(input, "automationId"))?;
            Value::Null
        }
        "releaseManualClaim" => {
            repo.release_manual_claim(
                &string_at(input, "automationId"),
                &string_at(input, "workspaceKey"),
            )?;
            Value::Null
        }
        "touchManualClaim" => {
            repo.touch_manual_claim(
                &string_at(input, "automationId"),
                &string_at(input, "workspaceKey"),
            )?;
            Value::Null
        }
        "skipAndReschedule" => {
            repo.skip_and_reschedule(&SkipAndRescheduleParams {
                automation_id: string_at(input, "automationId"),
                run_id: string_at(input, "runId"),
                workspace_key: string_at(input, "workspaceKey"),
                scheduled_at: optional_int_at(input, "scheduledAt"),
                reason: string_at(input, "reason"),
                next_run_at: optional_int_at(input, "nextRunAt"),
                finalize: input.get("finalize").and_then(Value::as_bool).unwrap_or(false),
            })?;
            Value::Null
        }
        "ensureRunClaimed" => {
            repo.ensure_run_claimed(&EnsureRunClaimedParams {
                run_id: string_at(input, "runId"),
                automation_id: string_at(input, "automationId"),
                workspace_key: string_at(input, "workspaceKey"),
                scheduled_at: optional_int_at(input, "scheduledAt"),
                trigger: string_at(input, "trigger"),
            })?;
            Value::Null
        }
        "upsertRunClaimed" => {
            repo.upsert_run_claimed(&UpsertRunClaimedParams {
                run_id: string_at(input, "runId"),
                automation_id: string_at(input, "automationId"),
                workspace_key: string_at(input, "workspaceKey"),
                scheduled_at: optional_int_at(input, "scheduledAt"),
                trigger: string_at(input, "trigger"),
                model_selection: input
                    .get("modelSelection")
                    .filter(|value| !value.is_null())
                    .map(selection_from_json),
            })?;
            Value::Null
        }
        "fixRunModelSelection" => serde_json::to_value(repo.fix_run_model_selection(
            &string_at(input, "runId"),
            &selection_from_json(&input["selection"]),
        )?)?,
        "markRunDispatch" => {
            repo.mark_run_dispatch(&MarkRunDispatchParams {
                run_id: string_at(input, "runId"),
                dispatch_status: string_at(input, "dispatchStatus"),
                session_id: optional_string_at(input, "sessionId"),
                error: optional_string_at(input, "error"),
            })?;
            Value::Null
        }
        "markManualRunDispatched" => json!(repo.mark_manual_run_dispatched(
            &MarkManualRunDispatchedParams {
                run_id: string_at(input, "runId"),
                session_id: optional_string_at(input, "sessionId"),
                dispatched_at: int_at(input, "dispatchedAt"),
            },
        )?),
        "markRunOutcome" => {
            let error = optional_string_at(input, "error");
            repo.mark_run_outcome(
                &string_at(input, "runId"),
                &string_at(input, "outcome"),
                error.as_deref(),
            )?;
            Value::Null
        }
        "recordSkippedRun" => {
            repo.record_skipped_run(&RecordSkippedRunParams {
                run_id: string_at(input, "runId"),
                automation_id: string_at(input, "automationId"),
                workspace_key: string_at(input, "workspaceKey"),
                scheduled_at: optional_int_at(input, "scheduledAt"),
                trigger: string_at(input, "trigger"),
                reason: string_at(input, "reason"),
            })?;
            Value::Null
        }
        "listRuns" => serde_json::to_value(repo.list_runs(
            &string_at(input, "automationId"),
            optional_string_at(input, "workspaceKey").as_deref(),
        )?)?,
        "getRun" => serde_json::to_value(repo.get_run(&string_at(input, "runId"))?)?,
        "deleteRun" => {
            repo.delete_run(
                &string_at(input, "runId"),
                optional_string_at(input, "workspaceKey").as_deref(),
            )?;
            Value::Null
        }
        "pruneRuns" => json!(repo.prune_runs(int_at(input, "maxAgeMs"))?),
        "create" => serde_json::to_value(repo.create(
            &create_params(&input["params"]),
            &create_options(&input["options"]),
        )?)?,
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
    let repo = AutomationRepo::with_clock(&db_path, move || clock);

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
                let output = normalize_value(&output);
                if output != step["output"] {
                    let detail = first_difference(&step["output"], &output, "")
                        .unwrap_or_else(|| "（无差异定位）".to_owned());
                    panic!("{id}: 方法返回值必须与 TS 深度相等\n{detail}");
                }
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
