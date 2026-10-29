//! 闲时任务仓库的单元/集成测试：初始化修复、默认主键、认领回收与终态守卫。

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use off_peak_repo::{
    CreateOptions, CreateParams, ModelSelection, OffPeakTaskRepo, SelectionOptions,
};
use tasks_db::RunOptions;

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
        "off-peak-repo-test-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("创建临时目录");
    TempDir(path)
}

fn migrated_db(path: &PathBuf) -> Connection {
    let connection = Connection::open(path).expect("打开库");
    tasks_db::run_migrations(&connection, RunOptions::default()).expect("迁移");
    connection
}

fn selection() -> ModelSelection {
    ModelSelection {
        provider_id: "zai-api".to_owned(),
        model_id: "glm-4.6".to_owned(),
        options: Some(SelectionOptions {
            reasoning_level: Some("high".to_owned()),
        }),
    }
}

fn params() -> CreateParams {
    CreateParams {
        title: "t".to_owned(),
        prompt: "p".to_owned(),
        permission_mode: "build".to_owned(),
        model_selection: selection(),
        workspace_path: "D:/ws".to_owned(),
        workspace_identity: None,
        bound_session_id: None,
    }
}

#[test]
fn ensure_ready_repairs_awaiting_approval_with_injected_clock() {
    let temp = temp_dir();
    let path = temp.0.join("tasks.sqlite");
    {
        let connection = migrated_db(&path);
        connection
            .execute(
                "INSERT INTO off_peak_tasks (
                   off_peak_task_id, title, prompt, permission_mode, workspace_key, workspace_path,
                   status, queued_at, created_at, updated_at
                 ) VALUES ('legacy', 'l', 'p', 'build', 'D:/ws', 'D:/ws', 'awaiting_approval', 1, 1, 1)",
                [],
            )
            .unwrap();
    }
    let repo = OffPeakTaskRepo::with_clock(&path, || 777);
    repo.ensure_ready().unwrap();

    let connection = Connection::open(&path).unwrap();
    let (status, updated_at): (String, i64) = connection
        .query_row(
            "SELECT status, updated_at FROM off_peak_tasks WHERE off_peak_task_id = 'legacy'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "running");
    assert_eq!(updated_at, 777, "修复时间必须来自注入时钟");
}

#[test]
fn create_generates_offpeak_prefixed_id_and_queues() {
    let temp = temp_dir();
    let path = temp.0.join("tasks.sqlite");
    let _ = migrated_db(&path);
    let repo = OffPeakTaskRepo::with_clock(&path, || 1000);
    let task = repo
        .create(
            &params(),
            &CreateOptions {
                now: Some(1000),
                ..CreateOptions::default()
            },
        )
        .unwrap();
    assert!(task.off_peak_task_id.starts_with("offpeak-"));
    assert_eq!(task.status, "queued");
    assert_eq!(task.queued_at, 1000);
    assert_eq!(task.workspace_key, "D:/ws");
    assert!(!task.schedulable);
    assert!(task.model_selection.is_some());
}

#[test]
fn claim_due_recovers_stale_claims_before_claiming() {
    let temp = temp_dir();
    let path = temp.0.join("tasks.sqlite");
    {
        let connection = migrated_db(&path);
        // 一条僵尸认领（超时）与一条健康排队任务。
        connection
            .execute(
                "INSERT INTO off_peak_tasks (
                   off_peak_task_id, title, prompt, permission_mode, model_selection,
                   workspace_key, workspace_path, status, queued_at, schedulable,
                   claim_running, claimed_at, created_at, updated_at
                 ) VALUES ('stale', 's', 'p', 'build',
                   '{\"providerId\":\"zai-api\",\"modelId\":\"glm-4.6\"}',
                   'D:/ws', 'D:/ws', 'queued', 1, 1, 1, 5, 1, 5)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO off_peak_tasks (
                   off_peak_task_id, title, prompt, permission_mode, model_selection,
                   workspace_key, workspace_path, status, queued_at, schedulable,
                   claim_running, claimed_at, created_at, updated_at
                 ) VALUES ('due', 'd', 'p', 'build',
                   '{\"providerId\":\"zai-api\",\"modelId\":\"glm-4.6\"}',
                   'D:/ws', 'D:/ws', 'queued', 2, 1, 0, NULL, 2, 2)",
                [],
            )
            .unwrap();
    }
    let repo = OffPeakTaskRepo::with_clock(&path, || 1);
    // now 足够大，使 stale 的 claimed_at 过期。
    let claimed = repo.claim_due(1_000_000).unwrap();
    let mut ids: Vec<&str> = claimed
        .iter()
        .map(|task| task.off_peak_task_id.as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["due", "stale"]);
}

#[test]
fn terminal_status_is_irreversible() {
    let temp = temp_dir();
    let path = temp.0.join("tasks.sqlite");
    let _ = migrated_db(&path);
    let repo = OffPeakTaskRepo::with_clock(&path, || 1);
    repo.create(
        &params(),
        &CreateOptions {
            now: Some(10),
            off_peak_task_id: Some("op-1".to_owned()),
            schedulable: Some(true),
            ..CreateOptions::default()
        },
    )
    .unwrap();
    repo.claim_due(20).unwrap();
    repo.mark_running(
        "op-1",
        &off_peak_repo::MarkRunningOptions {
            started_at: 30,
            conversation_id: None,
            session_id: None,
            server_ticket_id: None,
        },
    )
    .unwrap();
    let first = repo
        .mark_terminal(
            "op-1",
            &off_peak_repo::MarkTerminalOptions {
                status: "completed".to_owned(),
                ended_at: 40,
                failure_reason: None,
                files_changed: None,
                dispatch_error: None,
            },
        )
        .unwrap();
    assert!(first.is_some());
    let second = repo
        .mark_terminal(
            "op-1",
            &off_peak_repo::MarkTerminalOptions {
                status: "failed".to_owned(),
                ended_at: 50,
                failure_reason: None,
                files_changed: None,
                dispatch_error: None,
            },
        )
        .unwrap();
    assert!(second.is_none(), "终态不可逆出");

    let connection = Connection::open(&path).unwrap();
    let status: String = connection
        .query_row(
            "SELECT status FROM off_peak_tasks WHERE off_peak_task_id = 'op-1'",
            params![],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "completed");
}
