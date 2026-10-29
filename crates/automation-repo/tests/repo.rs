//! 定向单测：覆盖 golden fixture 不易表达的边界（退避封顶、创建限额、陈旧
//! claim 回收、迟到 running 不覆盖终态、工作区过滤）。
//!
//! 行为真源：`packages/services/src/session/automationRepo.ts`。回归点附中文注释。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use automation_repo::{
    AutomationCreateOptions, AutomationCreateParams, AutomationRepo, AutomationUpdateOptions,
    AutomationUpdateParams, DispatchFailureKind, MarkDispatchFailedOptions, MarkDispatchedOptions,
    DISPATCH_MAX_ATTEMPTS, DISPATCH_RETRY_BASE_MS, DISPATCH_RETRY_CAP_MS,
};
use rusqlite::{params, Connection};
use tasks_db::RunOptions;

const NOW: i64 = 2_000_000_000_000;
const WS_A: &str = "D:/ws-alpha";
const WS_B: &str = "remote:ssh:box:22:me:/srv/ws";

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
        "automation-repo-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("创建临时目录");
    TempDir(path)
}

fn open_repo(temp: &TempDir) -> (AutomationRepo, PathBuf) {
    let db_path = temp.0.join("tasks-index.sqlite");
    {
        let connection = Connection::open(&db_path).expect("打开库");
        tasks_db::run_migrations(&connection, RunOptions::default()).expect("迁移");
    }
    (AutomationRepo::with_clock(&db_path, || NOW), db_path)
}

fn insert_automation(db_path: &Path, id: &str, workspace_key: &str, extra: &[(&str, i64)]) {
    let connection = Connection::open(db_path).expect("打开库");
    let mut next_run_at = 1_i64;
    let mut running = 0_i64;
    let mut claimed_at: Option<i64> = None;
    let mut enabled = 1_i64;
    let mut recurring = 1_i64;
    for (key, value) in extra {
        match *key {
            "next_run_at" => next_run_at = *value,
            "running" => running = *value,
            "claimed_at" => claimed_at = Some(*value),
            "enabled" => enabled = *value,
            "recurring" => recurring = *value,
            other => panic!("未知字段: {other}"),
        }
    }
    connection
        .execute(
            "INSERT INTO automations (
               automation_id, title, cron_expr, prompt, model_selection,
               workspace_key, workspace_path, recurring, enabled, next_run_at, running, claimed_at,
               created_at, updated_at
             ) VALUES (?1, ?2, '*/5 * * * *', 'prompt', 'null', ?3, ?3, ?4, ?5, ?6, ?7, ?8, 10, 10)",
            params![
                id,
                format!("title {id}"),
                workspace_key,
                recurring,
                enabled,
                next_run_at,
                running,
                claimed_at
            ],
        )
        .expect("insert automation");
}

fn insert_run(db_path: &Path, run_id: &str, automation_id: &str, dispatch_status: &str) {
    let connection = Connection::open(db_path).expect("打开库");
    connection
        .execute(
            "INSERT INTO automation_runs (
               run_id, automation_id, workspace_key, trigger, dispatch_status,
               attempts, created_at, updated_at
             ) VALUES (?, ?, ?, 'schedule', ?, 0, 10, 10)",
            params![run_id, automation_id, WS_A, dispatch_status],
        )
        .expect("insert run");
}

fn insert_placeholders(db_path: &Path, count: i64) {
    for index in 0..count {
        insert_automation(db_path, &format!("fill-{index}"), WS_A, &[]);
    }
}

fn create_params() -> AutomationCreateParams {
    AutomationCreateParams {
        title: "created".to_owned(),
        cron_expr: "0 9 * * *".to_owned(),
        prompt: "prompt".to_owned(),
        workspace_path: WS_A.to_owned(),
        recurring: true,
        ..AutomationCreateParams::default()
    }
}

#[test]
fn retry_backoff_doubles_and_caps() {
    assert_eq!(
        automation_repo::compute_retry_at(0, 1),
        DISPATCH_RETRY_BASE_MS,
        "首次失败按 30s 退避"
    );
    assert_eq!(automation_repo::compute_retry_at(0, 2), 60_000);
    assert_eq!(automation_repo::compute_retry_at(0, 3), 120_000);
    assert_eq!(automation_repo::compute_retry_at(0, 5), 480_000);
    assert_eq!(
        automation_repo::compute_retry_at(0, 6),
        DISPATCH_RETRY_CAP_MS,
        "第 6 次起封顶 15min，不再指数增长"
    );
    assert_eq!(
        automation_repo::compute_retry_at(0, 0),
        DISPATCH_RETRY_BASE_MS,
        "attempts<=0 等价首次退避"
    );
    assert_eq!(
        automation_repo::compute_retry_at(NOW, 2),
        NOW + 60_000,
        "退避基于失败时间累加"
    );
}

#[test]
fn create_is_rejected_at_limit_with_stable_error() {
    let temp = temp_dir();
    let (repo, db_path) = open_repo(&temp);
    insert_placeholders(&db_path, 20);
    let error = repo
        .create(&create_params(), &AutomationCreateOptions {
            next_run_at: None,
            lifecycle_status: None,
        })
        .expect_err("达到上限后必须拒绝创建");
    assert_eq!(
        error.to_string(),
        "[AUTOMATION_CREATE_LIMIT_REACHED] At most 20 automations may be retained. Delete an existing automation before creating another.",
        "错误文案与错误码必须与 TS 一致"
    );
    repo.close();

    let temp = temp_dir();
    let (repo, db_path) = open_repo(&temp);
    insert_placeholders(&db_path, 19);
    repo.create(&create_params(), &AutomationCreateOptions {
            next_run_at: None,
            lifecycle_status: None,
        })
        .expect("上限内的最后一条仍可创建");
    repo.close();
}

#[test]
fn claim_due_reclaims_only_stale_claims() {
    let temp = temp_dir();
    let (repo, db_path) = open_repo(&temp);
    insert_automation(
        &db_path,
        "stale",
        WS_A,
        &[
            ("next_run_at", 1),
            ("running", 1),
            ("claimed_at", NOW - automation_repo::CLAIM_STALE_MS),
        ],
    );
    insert_automation(
        &db_path,
        "fresh",
        WS_A,
        &[("next_run_at", 1), ("running", 1), ("claimed_at", NOW - 1)],
    );

    let claimed = repo.claim_due(NOW).expect("claim_due");
    let ids: Vec<&str> = claimed.iter().map(|row| row.automation_id.as_str()).collect();
    assert_eq!(ids, vec!["stale"], "只回收 claimed_at <= now-10min 的陈旧持有");

    let fresh = repo.get("fresh", None).expect("get").expect("fresh 存在");
    assert!(fresh.enabled, "仍在有效期内的 claim 不应被改写");
    assert_eq!(fresh.dispatch_status, "idle", "未被认领的行保持原 dispatch_status");
    repo.close();
}

#[test]
fn late_running_outcome_does_not_override_terminal_state() {
    let temp = temp_dir();
    let (repo, _db_path) = open_repo(&temp);
    insert_run(&_db_path, "run-1", "a-1", "dispatched");

    // 失败终态先到，随后迟到的 running 事件不能把它翻回进行中。
    repo.mark_run_outcome("run-1", "failed", Some("boom"))
        .expect("failed");
    repo.mark_run_outcome("run-1", "running", None)
        .expect("late running");
    let run = repo.get_run("run-1").expect("get_run").expect("run 存在");
    assert_eq!(run.outcome.as_deref(), Some("failed"));
    assert_eq!(run.error.as_deref(), Some("boom"), "终态错误必须保留");

    // 正常顺序下 running 可以先落库，再被终态覆盖。
    insert_run(&_db_path, "run-2", "a-1", "dispatched");
    repo.mark_run_outcome("run-2", "running", None)
        .expect("running");
    repo.mark_run_outcome("run-2", "succeeded", None)
        .expect("succeeded");
    let run = repo.get_run("run-2").expect("get_run").expect("run 存在");
    assert_eq!(run.outcome.as_deref(), Some("succeeded"));
    repo.close();
}

#[test]
fn workspace_scoped_writes_do_not_touch_other_workspaces() {
    let temp = temp_dir();
    let (repo, db_path) = open_repo(&temp);
    insert_automation(&db_path, "a-1", WS_A, &[]);
    insert_automation(&db_path, "b-1", WS_B, &[]);

    // 曾经的占位符混用会让 workspace 过滤失效，把另一工作区的行一并改写。
    repo.set_enabled("a-1", false, Some(WS_B))
        .expect("set_enabled 不应报错");
    repo.restart(
        "a-1",
        &automation_repo::AutomationRestartOptions {
            next_run_at: Some(42),
        },
        Some(WS_B),
    )
    .expect("restart 不应报错");
    let untouched = repo.get("a-1", None).expect("get").expect("a-1 存在");
    assert!(untouched.enabled, "异工作区调用不得改变 enabled");
    assert_eq!(untouched.lifecycle_status, "active");

    repo.set_enabled("a-1", false, Some(WS_A))
        .expect("同工作区调用");
    let disabled = repo.get("a-1", None).expect("get").expect("a-1 存在");
    assert!(!disabled.enabled);
    assert_eq!(disabled.lifecycle_status, "paused");

    repo.restart(
        "a-1",
        &automation_repo::AutomationRestartOptions {
            next_run_at: Some(42),
        },
        Some(WS_A),
    )
    .expect("同工作区 restart");
    let restarted = repo.get("a-1", None).expect("get").expect("a-1 存在");
    assert!(restarted.enabled);
    assert_eq!(restarted.next_run_at, Some(42));
    repo.close();
}

#[test]
fn transient_failure_retries_then_gives_up_at_max_attempts() {
    let temp = temp_dir();
    let (repo, db_path) = open_repo(&temp);
    insert_automation(&db_path, "once", WS_A, &[("recurring", 0)]);

    repo.mark_dispatch_failed(
        "once",
        &MarkDispatchFailedOptions {
            failed_at: NOW,
            error: "retry".to_owned(),
            kind: DispatchFailureKind::Transient,
            next_run_at: Some(123),
        },
    )
    .expect("第一次瞬时失败");
    let row = repo.get("once", None).expect("get").expect("once 存在");
    assert_eq!(row.dispatch_attempts, 1);
    assert_eq!(row.retry_at, Some(NOW + DISPATCH_RETRY_BASE_MS));
    assert_eq!(row.dispatch_status, "failed_to_dispatch");
    assert!(row.enabled, "瞬时失败不关闭任务");

    for attempt in 2..DISPATCH_MAX_ATTEMPTS {
        repo.mark_dispatch_failed(
            "once",
            &MarkDispatchFailedOptions {
                failed_at: NOW,
                error: "retry".to_owned(),
                kind: DispatchFailureKind::Transient,
                next_run_at: Some(123),
            },
        )
        .expect("继续失败");
        let row = repo.get("once", None).expect("get").expect("once 存在");
        assert_eq!(row.dispatch_attempts, attempt);
    }

    // 非周期任务达到上限后进入失败终态，不再等待重试。
    repo.mark_dispatch_failed(
        "once",
        &MarkDispatchFailedOptions {
            failed_at: NOW,
            error: "retry".to_owned(),
            kind: DispatchFailureKind::Transient,
            next_run_at: Some(123),
        },
    )
    .expect("第 5 次失败");
    let row = repo.get("once", None).expect("get").expect("once 存在");
    assert_eq!(row.lifecycle_status, "failed");
    assert_eq!(row.dispatch_status, "failed_to_dispatch");
    assert!(!row.enabled);
    // 与 TS 一致：放弃时不清 retry_at，靠 enabled=0 退出认领条件。
    assert_eq!(row.retry_at, Some(NOW + 240_000));
    repo.close();
}

#[test]
fn permanent_failure_closes_task_immediately() {
    let temp = temp_dir();
    let (repo, db_path) = open_repo(&temp);
    insert_automation(&db_path, "broken", WS_A, &[]);
    repo.mark_dispatch_failed(
        "broken",
        &MarkDispatchFailedOptions {
            failed_at: NOW,
            error: "nope".to_owned(),
            kind: DispatchFailureKind::Permanent,
            next_run_at: None,
        },
    )
    .expect("永久失败");
    let row = repo.get("broken", None).expect("get").expect("broken 存在");
    assert_eq!(row.lifecycle_status, "failed");
    assert!(!row.enabled);
    assert_eq!(row.last_error.as_deref(), Some("nope"));
    assert_eq!(row.dispatch_attempts, 0, "永久失败不累计重试次数");
    repo.close();
}

#[test]
fn mark_dispatched_finishes_single_shot_when_max_runs_reached() {
    let temp = temp_dir();
    let (repo, db_path) = open_repo(&temp);
    let connection = Connection::open(&db_path).expect("打开库");
    connection
        .execute(
            "INSERT INTO automations (
               automation_id, title, cron_expr, prompt, model_selection,
               workspace_key, workspace_path, recurring, max_runs, scheduled_run_count,
               created_at, updated_at
             ) VALUES ('once', 'title', '*/5 * * * *', 'prompt', 'null', ?1, ?1, 0, 1, 0, 10, 10)",
            params![WS_A],
        )
        .expect("insert single shot");

    repo.mark_dispatched(
        "once",
        &MarkDispatchedOptions {
            dispatched_at: NOW,
            next_run_at: Some(NOW + 1000),
        },
    )
    .expect("mark_dispatched");
    let row = repo.get("once", None).expect("get").expect("once 存在");
    assert_eq!(
        repo.get_scheduled_run_count("once", None).expect("count"),
        Some(1)
    );
    assert_eq!(row.lifecycle_status, "completed");
    assert!(!row.enabled);
    assert_eq!(row.next_run_at, None, "达到 maxRuns 后清空下次运行时间");
    repo.close();
}

#[test]
fn update_can_clear_nullable_fields_with_explicit_null() {
    let temp = temp_dir();
    let (repo, db_path) = open_repo(&temp);
    let connection = Connection::open(&db_path).expect("打开库");
    connection
        .execute(
            "INSERT INTO automations (
               automation_id, title, cron_expr, prompt, model_selection,
               workspace_key, workspace_path, max_runs, end_at, schedule_rule,
               created_at, updated_at
             ) VALUES ('a-1', 'title', '*/5 * * * *', 'prompt', 'null', ?1, ?1, 5, 999,
               '{\"unit\":\"daily\",\"interval\":1,\"hour\":9,\"minute\":0}', 10, 10)",
            params![WS_A],
        )
        .expect("insert automation");

    let updated = repo
        .update(
            "a-1",
            &AutomationUpdateParams {
                max_runs: Some(None),
                end_at: Some(None),
                schedule_rule: Some(None),
                ..AutomationUpdateParams::default()
            },
            &AutomationUpdateOptions::default(),
            None,
        )
        .expect("update")
        .expect("a-1 存在");
    assert_eq!(updated.max_runs, None);
    assert_eq!(updated.end_at, None);
    assert_eq!(updated.schedule_rule, None);
    assert_eq!(updated.title, "title", "未提供的字段保持原值");
    repo.close();
}
