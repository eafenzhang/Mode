//! 任务索引仓库的单元/集成测试：初始化幂等、写串行化、CRUD、列表/搜索、归档清理。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde_json::json;
use task_index_repo::{
    parse_mode_task_meta, ListTaskMetasParams, ModeTaskMeta, SyncTaskMetaParams, TaskIndexRepo,
    TaskIndexStatePatch, TaskListKind, TaskListQuery, TaskListSortBy, TaskListWorkspaceScope,
    TaskRef,
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
        "task-index-repo-test-{}-{nanos}",
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

fn meta(task_id: &str, workspace_path: &str, title: &str, updated_at: i64) -> ModeTaskMeta {
    parse_mode_task_meta(&json!({
        "taskId": task_id,
        "traceId": format!("mode-{task_id}"),
        "title": title,
        "workspacePath": workspace_path,
        "createdAt": 1,
        "updatedAt": updated_at,
        "mode": "build",
        "provider": "glm",
        "status": "running"
    }))
    .expect("meta 合法")
}

fn task_ref(workspace_path: &str, task_id: &str) -> TaskRef {
    TaskRef {
        workspace_path: workspace_path.to_owned(),
        workspace_identity: None,
        task_id: task_id.to_owned(),
    }
}

fn scope(workspace_path: &str) -> TaskListWorkspaceScope {
    TaskListWorkspaceScope {
        workspace_path: workspace_path.to_owned(),
        workspace_identity: None,
        workspace_purpose: None,
    }
}

#[test]
fn ensure_ready_is_idempotent_and_backfills_off_peak() {
    let temp = temp_dir();
    let path = temp.0.join("tasks.sqlite");
    {
        let connection = migrated_db(&path);
        connection
            .execute(
                "INSERT INTO off_peak_tasks (
                   off_peak_task_id, title, session_id, prompt, permission_mode,
                   workspace_key, workspace_path, status, queued_at, created_at, updated_at
                 ) VALUES ('op-1', 'op', 'task-1', 'p', 'default', 'D:/ws', 'D:/ws', 'queued', 1, 1, 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO tasks (
                   workspace_key, workspace_path, task_id, title, mode, created_at, updated_at, meta_json
                 ) VALUES ('D:/ws', 'D:/ws', 'task-1', 'task', 'build', 1, 2, '{}')",
                [],
            )
            .unwrap();
    }

    let repo = TaskIndexRepo::new(&path);
    repo.ensure_ready().unwrap();
    repo.ensure_ready().unwrap();
    repo.ensure_ready().unwrap();

    let connection = Connection::open(&path).unwrap();
    let migrations: i64 = connection
        .query_row("SELECT COUNT(*) FROM tasks_schema_migration", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(migrations, 3, "重复 ensure_ready 不得重复执行迁移");

    let marker: Option<String> = connection
        .query_row(
            "SELECT off_peak_task_id FROM tasks WHERE workspace_key='D:/ws' AND task_id='task-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(marker.as_deref(), Some("op-1"));

    let memberships: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM task_group_members WHERE task_id='task-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(memberships, 1, "首次回填补齐闲时系统分组归属");

    repo.ensure_ready().unwrap();
    let memberships_again: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM task_group_members WHERE task_id='task-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(memberships_again, 1, "回填必须幂等");
}

#[test]
fn ensure_ready_does_not_repeat_group_backfill() {
    let temp = temp_dir();
    let path = temp.0.join("tasks.sqlite");
    {
        let connection = migrated_db(&path);
        connection
            .execute(
                "INSERT INTO off_peak_tasks (
                   off_peak_task_id, title, session_id, prompt, permission_mode,
                   workspace_key, workspace_path, status, queued_at, created_at, updated_at
                 ) VALUES ('op-1', 'op', 'task-1', 'p', 'default', 'D:/ws', 'D:/ws', 'queued', 1, 1, 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO tasks (
                   workspace_key, workspace_path, task_id, title, mode, created_at, updated_at, meta_json
                 ) VALUES ('D:/ws', 'D:/ws', 'task-1', 'task', 'build', 1, 2, '{}')",
                [],
            )
            .unwrap();
    }

    let repo = TaskIndexRepo::new(&path);
    repo.ensure_ready().unwrap();
    {
        // 模拟用户把闲时任务移出系统分组后删除空分组。
        let connection = Connection::open(&path).unwrap();
        connection
            .execute("DELETE FROM task_group_members WHERE task_id='task-1'", [])
            .unwrap();
        connection
            .execute(
                "DELETE FROM task_groups WHERE group_id='mode-default-group-off-peak'",
                [],
            )
            .unwrap();
    }

    repo.ensure_ready().unwrap();

    let connection = Connection::open(&path).unwrap();
    let groups: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM task_groups WHERE group_id='mode-default-group-off-peak'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        groups, 0,
        "首次初始化后的 ensure_ready 不得重建用户已移出的系统分组"
    );
    let memberships: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM task_group_members WHERE task_id='task-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(memberships, 0, "回填只能执行一次，不能把任务重新拉回分组");
}

#[test]
fn crud_flow_matches_expected_state_transitions() {
    let temp = temp_dir();
    let path = temp.0.join("tasks.sqlite");
    drop(migrated_db(&path));
    let repo = TaskIndexRepo::with_clock(&path, || 1_000_000);
    repo.ensure_ready().unwrap();

    let created = repo
        .seed_task_meta_if_missing(&meta("t1", "D:/ws", "hello", 10))
        .unwrap();
    assert_eq!(created.title, "hello");
    assert_eq!(created.title_overridden, Some(false));

    let again = repo
        .seed_task_meta_if_missing(&meta("t1", "D:/ws", "ignored", 11))
        .unwrap();
    assert_eq!(again.title, "hello", "已有行不得被 seed 覆盖");

    let renamed = repo
        .update_task_state(
            &task_ref("D:/ws", "t1"),
            &TaskIndexStatePatch {
                title: Some("renamed".to_owned()),
                pinned: Some(true),
                updated_at: Some(20),
                ..TaskIndexStatePatch::default()
            },
        )
        .unwrap();
    assert_eq!(renamed.title, "renamed");
    assert_eq!(renamed.updated_at, 20);

    let unread = repo
        .update_task_state(
            &task_ref("D:/ws", "t1"),
            &TaskIndexStatePatch {
                unread_at: Some(Some(100)),
                ..TaskIndexStatePatch::default()
            },
        )
        .unwrap();
    assert_eq!(unread.unread_at, Some(100), "未读 marker 从持久 watermark 起算");

    let cleared = repo
        .clear_task_unread_if_matches(&task_ref("D:/ws", "t1"), 100)
        .unwrap();
    assert!(cleared.cleared);
    assert_eq!(cleared.meta.unread_at, None);

    let mismatch = repo
        .clear_task_unread_if_matches(&task_ref("D:/ws", "t1"), 100)
        .unwrap();
    assert!(!mismatch.cleared);

    let agent = repo
        .apply_agent_patch(
            &task_ref("D:/ws", "t1"),
            &task_index_repo::AgentPatch {
                title: Some("agent".to_owned()),
                status: Some("completed".to_owned()),
                updated_at: Some(30),
                ..task_index_repo::AgentPatch::default()
            },
        )
        .unwrap()
        .expect("行存在");
    assert_eq!(agent.title, "agent");
    assert_eq!(agent.status.as_deref(), Some("completed"));

    // 用户改过标题后，agent 标题被拒绝。
    repo.update_task_state(
        &task_ref("D:/ws", "t1"),
        &TaskIndexStatePatch {
            title_overridden: Some(true),
            ..TaskIndexStatePatch::default()
        },
    )
    .unwrap();
    let rejected = repo
        .apply_agent_patch(
            &task_ref("D:/ws", "t1"),
            &task_index_repo::AgentPatch {
                title: Some("nope".to_owned()),
                ..task_index_repo::AgentPatch::default()
            },
        )
        .unwrap()
        .expect("行存在");
    assert_eq!(rejected.title, "agent");

    assert!(repo
        .get_task_meta(&task_ref("D:/ws", "missing"))
        .unwrap()
        .is_none());
    let error = repo
        .update_task_state(
            &task_ref("D:/ws", "missing"),
            &TaskIndexStatePatch::default(),
        )
        .unwrap_err();
    assert_eq!(error.to_string(), "task index 中不存在 task: missing");
}

#[test]
fn sync_preserves_monotonic_time_terminal_status_and_user_title() {
    let temp = temp_dir();
    let path = temp.0.join("tasks.sqlite");
    drop(migrated_db(&path));
    let repo = TaskIndexRepo::new(&path);

    let mut done = meta("t1", "D:/ws", "done", 100);
    done.status = Some("completed".to_owned());
    done.last_error = Some(task_index_repo::ModeTaskLastError {
        code: None,
        detail: None,
        message: "boom".to_owned(),
        trace_id: None,
        task_id: None,
        attribution: None,
    });
    repo.sync_task_meta(SyncTaskMetaParams {
        meta: done,
        pinned: None,
        archived: None,
        deleted: None,
        title_overridden: Some(true),
        searchable_text: Some("body".to_owned()),
    })
    .unwrap();

    let mut older = meta("t1", "D:/ws", "agent newer title", 50);
    older.status = Some("running".to_owned());
    let merged = repo
        .sync_task_meta(SyncTaskMetaParams {
            meta: older,
            pinned: None,
            archived: None,
            deleted: None,
            title_overridden: None,
            searchable_text: None,
        })
        .unwrap();
    assert_eq!(merged.updated_at, 100, "updatedAt 单调不回退");
    assert_eq!(merged.status.as_deref(), Some("completed"), "保留更晚终态");
    assert_eq!(
        merged.last_error.as_ref().map(|error| error.message.as_str()),
        Some("boom")
    );
    assert_eq!(merged.title, "done", "titleOverridden 时保留用户标题");

    // searchable_text 未传时保留旧值。
    let connection = Connection::open(&path).unwrap();
    let searchable: String = connection
        .query_row(
            "SELECT searchable_text FROM tasks WHERE task_id='t1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(searchable, "body");
}

#[test]
fn list_and_search_cover_filters_limits_and_snippets() {
    let temp = temp_dir();
    let path = temp.0.join("tasks.sqlite");
    drop(migrated_db(&path));
    let repo = TaskIndexRepo::new(&path);

    for (index, (task_id, title, body, updated)) in [
        ("a", "alpha", "alpha body", 10),
        ("b", "beta", "deploy failed deploy", 20),
        ("c", "gamma", "gamma body", 30),
    ]
    .into_iter()
    .enumerate()
    {
        repo.sync_task_meta(SyncTaskMetaParams {
            meta: meta(task_id, "D:/ws", title, updated),
            pinned: Some(index == 2),
            archived: None,
            deleted: None,
            title_overridden: None,
            searchable_text: Some(body.to_owned()),
        })
        .unwrap();
    }

    let active = repo
        .query_task_list(&TaskListQuery {
            kind: TaskListKind::Active,
            workspace_scopes: vec![scope("D:/ws")],
            sort_by: TaskListSortBy::Updated,
            search: None,
            provider: Some("glm".to_owned()),
            limit: None,
        })
        .unwrap();
    assert_eq!(active.total, 2);
    assert_eq!(
        active
            .items
            .iter()
            .map(|item| item.meta.task_id.as_str())
            .collect::<Vec<_>>(),
        vec!["b", "a"]
    );

    let pinned = repo
        .query_task_list(&TaskListQuery {
            kind: TaskListKind::Pinned,
            workspace_scopes: vec![scope("D:/ws")],
            sort_by: TaskListSortBy::Updated,
            search: None,
            provider: None,
            limit: None,
        })
        .unwrap();
    assert_eq!(pinned.total, 1);
    assert_eq!(pinned.items[0].meta.task_id, "c");

    let searched = repo
        .query_task_list(&TaskListQuery {
            kind: TaskListKind::Active,
            workspace_scopes: vec![scope("D:/ws")],
            sort_by: TaskListSortBy::Updated,
            search: Some("  DEPLOY  ".to_owned()),
            provider: None,
            limit: None,
        })
        .unwrap();
    assert_eq!(searched.total, 1);
    let item = &searched.items[0];
    assert_eq!(item.meta.task_id, "b");
    assert!(item.search_snippet.as_deref().unwrap_or("").contains("deploy"));
    assert_eq!(item.search_snippets.as_ref().map(Vec::len), Some(1));

    let limited = repo
        .query_task_list(&TaskListQuery {
            kind: TaskListKind::Active,
            workspace_scopes: vec![scope("D:/ws")],
            sort_by: TaskListSortBy::Created,
            search: None,
            provider: None,
            limit: Some(1.0),
        })
        .unwrap();
    assert_eq!(limited.items.len(), 1);
    assert!(limited.has_more);

    let empty = repo
        .query_task_list(&TaskListQuery {
            kind: TaskListKind::Active,
            workspace_scopes: vec![],
            sort_by: TaskListSortBy::Updated,
            search: None,
            provider: None,
            limit: None,
        })
        .unwrap();
    assert_eq!(empty.total, 0);
    assert!(!empty.has_more);

    let purpose = repo
        .query_task_list(&TaskListQuery {
            kind: TaskListKind::Active,
            workspace_scopes: vec![TaskListWorkspaceScope {
                workspace_path: "D:/ws".to_owned(),
                workspace_identity: None,
                workspace_purpose: Some("conversation".to_owned()),
            }],
            sort_by: TaskListSortBy::Updated,
            search: None,
            provider: None,
            limit: None,
        })
        .unwrap();
    assert!(purpose
        .items
        .iter()
        .all(|item| item.meta.workspace_purpose.as_deref() == Some("conversation")));
}

#[test]
fn write_serialization_prevents_lost_unread_updates() {
    let temp = temp_dir();
    let path = temp.0.join("tasks.sqlite");
    drop(migrated_db(&path));
    let repo = Arc::new(TaskIndexRepo::with_clock(&path, || 0));
    repo.ensure_ready().unwrap();
    repo.seed_task_meta_if_missing(&meta("t1", "D:/ws", "task", 1))
        .unwrap();

    let mut handles = Vec::new();
    for index in 0..8i64 {
        let repo = Arc::clone(&repo);
        handles.push(thread::spawn(move || {
            repo.update_task_state(
                &task_ref("D:/ws", "t1"),
                &TaskIndexStatePatch {
                    unread_at: Some(Some(index)),
                    ..TaskIndexStatePatch::default()
                },
            )
            .unwrap()
            .unread_at
            .expect("分配后必然有 unreadAt")
        }));
    }
    let markers: Vec<i64> = handles.into_iter().map(|handle| handle.join().unwrap()).collect();
    let unique: HashSet<i64> = markers.iter().copied().collect();
    assert_eq!(unique.len(), markers.len(), "同一 key 的写入不得交错复用 marker");
    assert!(
        markers.iter().copied().max().unwrap() >= 8,
        "每次都必须在持久 watermark 上严格递增"
    );

    let final_meta = repo
        .get_task_meta(&task_ref("D:/ws", "t1"))
        .unwrap()
        .unwrap();
    assert_eq!(final_meta.unread_at, Some(markers.iter().copied().max().unwrap()));
}

#[test]
fn archive_stale_and_delete_archived_clean_grouping_rows() {
    let temp = temp_dir();
    let path = temp.0.join("tasks.sqlite");
    drop(migrated_db(&path));
    let repo = TaskIndexRepo::with_clock(&path, || 10_000_000_000);

    let mut stale = meta("stale", "D:/ws", "stale", 100);
    stale.status = Some("completed".to_owned());
    repo.sync_task_meta(SyncTaskMetaParams {
        meta: stale,
        pinned: None,
        archived: None,
        deleted: None,
        title_overridden: None,
        searchable_text: None,
    })
    .unwrap();

    let mut fresh = meta("fresh", "D:/ws", "fresh", 9_999_000_000);
    fresh.status = Some("completed".to_owned());
    repo.sync_task_meta(SyncTaskMetaParams {
        meta: fresh,
        pinned: None,
        archived: None,
        deleted: None,
        title_overridden: None,
        searchable_text: None,
    })
    .unwrap();

    let archived = repo
        .archive_stale_tasks(&task_index_repo::ArchiveStaleTasksParams {
            workspace_path: "D:/ws".to_owned(),
            workspace_identity: None,
            older_than_days: 1.0,
            provider: Some("glm".to_owned()),
        })
        .unwrap();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].task_id, "stale");
    assert_eq!(
        repo.list_task_metas(&ListTaskMetasParams {
            workspace_path: Some("D:/ws".to_owned()),
            ..ListTaskMetasParams::default()
        })
        .unwrap()
        .len(),
        2,
        "listTaskMetas 未指定 archived 时包含归档行"
    );
    let archived_only = repo
        .list_task_metas(&ListTaskMetasParams {
            workspace_path: Some("D:/ws".to_owned()),
            archived: Some(true),
            ..ListTaskMetasParams::default()
        })
        .unwrap();
    assert_eq!(archived_only.len(), 1);
    assert_eq!(archived_only[0].task_id, "stale");

    // 给归档行造一个 cron 分组归属，删除时必须清理。
    let mut cron = meta("cron-archived", "D:/ws", "cron", 5_000);
    cron.status = Some("completed".to_owned());
    cron.cron_automation_id = Some("auto-1".to_owned());
    repo.sync_task_meta(SyncTaskMetaParams {
        meta: cron,
        pinned: None,
        archived: Some(true),
        deleted: None,
        title_overridden: None,
        searchable_text: None,
    })
    .unwrap();

    let deleted = repo
        .delete_archived_task(&task_ref("D:/ws", "cron-archived"))
        .unwrap()
        .expect("归档行存在");
    assert_eq!(deleted.task_id, "cron-archived");
    assert!(repo
        .delete_archived_task(&task_ref("D:/ws", "cron-archived"))
        .unwrap()
        .is_none());
    assert!(repo
        .delete_archived_task(&task_ref("D:/ws", "fresh"))
        .unwrap()
        .is_none(), "未归档行不得删除");

    let connection = Connection::open(&path).unwrap();
    let members: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM task_group_members WHERE task_id='cron-archived'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(members, 0, "删除 task 必须清理分组引用");

    let deleted_ids = repo
        .list_deleted_task_ids(&task_index_repo::ListDeletedTaskIdsParams {
            workspace_path: "D:/ws".to_owned(),
            workspace_identity: None,
            provider: None,
        })
        .unwrap();
    assert_eq!(deleted_ids, vec!["cron-archived".to_owned()]);
}

#[test]
fn row_projection_falls_back_without_valid_meta_json() {
    let temp = temp_dir();
    let path = temp.0.join("tasks.sqlite");
    {
        let connection = migrated_db(&path);
        connection
            .execute(
                "INSERT INTO tasks (
                   workspace_key, workspace_path, workspace_identity, task_id, title, task_status,
                   provider, mode, model, created_at, updated_at, title_overridden, meta_json
                 ) VALUES ('D:/ws', 'D:/ws', NULL, 'raw', 'raw title', 'error', 'glm', 'plan',
                           'glm-4.5', 1, 2, 1, '{bad json')",
                [],
            )
            .unwrap();
    }
    let repo = TaskIndexRepo::new(&path);
    let projected = repo
        .get_task_meta(&task_ref("D:/ws", "raw"))
        .unwrap()
        .expect("行存在");
    assert_eq!(projected.trace_id, "mode-raw");
    assert_eq!(projected.mode, "plan");
    assert_eq!(projected.status.as_deref(), Some("error"));
    assert_eq!(projected.model.as_deref(), Some("glm-4.5"));
    assert_eq!(projected.provider.as_deref(), Some("glm"));
    assert_eq!(projected.title_overridden, Some(true));
    assert_eq!(projected.workspace_identity, None);
}

#[test]
fn workspace_identity_projection_follows_workspace_key() {
    let temp = temp_dir();
    let path = temp.0.join("tasks.sqlite");
    let remote = "remote:ssh:box:22:me:/srv/ws";
    {
        let connection = migrated_db(&path);
        let mut insert = connection
            .prepare(
                "INSERT INTO tasks (
                   workspace_key, workspace_path, workspace_identity, task_id, title, mode,
                   created_at, updated_at, meta_json
                 ) VALUES (?, ?, ?, ?, ?, 'build', 1, 2, ?)",
            )
            .unwrap();
        insert
            .execute(params![
                "D:/local",
                "D:/local",
                "remote:ssh:other:22:me:/elsewhere",
                "local-mismatch",
                "local",
                "{}"
            ])
            .unwrap();
        insert
            .execute(params![remote, "/srv/ws", "D:/stale", "remote-row", "remote", "{}"])
            .unwrap();
    }
    let repo = TaskIndexRepo::new(&path);
    let local = repo
        .get_task_meta(&task_ref("D:/local", "local-mismatch"))
        .unwrap()
        .unwrap();
    assert_eq!(
        local.workspace_identity, None,
        "identity 与 key 不一致时不得投影"
    );
    let remote_meta = repo
        .get_task_meta(&TaskRef {
            workspace_path: "D:/ignored".to_owned(),
            workspace_identity: Some(remote.to_owned()),
            task_id: "remote-row".to_owned(),
        })
        .unwrap()
        .unwrap();
    assert_eq!(
        remote_meta.workspace_identity.as_deref(),
        Some(remote),
        "远端身份必须与主键一致"
    );
}

#[test]
fn sessions_by_automation_filters_deleted_and_orders_by_created() {
    let temp = temp_dir();
    let path = temp.0.join("tasks.sqlite");
    drop(migrated_db(&path));
    let repo = TaskIndexRepo::new(&path);

    for (task_id, created, deleted) in [("older", 10, false), ("newer", 20, false), ("gone", 30, true)] {
        let mut value = meta(task_id, "D:/ws", task_id, created);
        value.created_at = created;
        value.cron_automation_id = Some("auto-1".to_owned());
        repo.sync_task_meta(SyncTaskMetaParams {
            meta: value,
            pinned: None,
            archived: None,
            deleted: Some(deleted),
            title_overridden: None,
            searchable_text: None,
        })
        .unwrap();
    }

    let sessions = repo.list_sessions_by_automation("auto-1").unwrap();
    assert_eq!(
        sessions
            .iter()
            .map(|meta| meta.task_id.as_str())
            .collect::<Vec<_>>(),
        vec!["newer", "older"]
    );
}
