//! `window-controller` 频道首切行为契约（LT-16.3：local 单源列表 + 变更面）。
//!
//! 真实临时 DB（task-index-repo）→ 频道调用 → mutation 语义断言。

use std::path::PathBuf;
use std::sync::Arc;

use rpc_host::{CancellationToken, ServerChannel as _};
use rpc_wire::Value;
use serde_json::{json, Value as Json};
use task_index_repo::{
    parse_mode_task_meta, ModeTaskMeta, SyncTaskMetaParams, TaskIndexRepo,
};

use host_channels::window_controller::{WindowControllerChannel, WINDOW_CONTROLLER_CHANNEL};

struct TempDb(PathBuf);

impl TempDb {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "window-controller-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        TempDb(dir)
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn repo(db: &TempDb) -> Arc<TaskIndexRepo> {
    Arc::new(TaskIndexRepo::new(db.0.join("tasks-index.sqlite")))
}

fn meta(task_id: &str, workspace_path: &str, created_at: i64) -> ModeTaskMeta {
    parse_mode_task_meta(&json!({
        "taskId": task_id,
        "traceId": format!("trace-{task_id}"),
        "title": format!("任务 {task_id}"),
        "workspacePath": workspace_path,
        "createdAt": created_at,
        "updatedAt": created_at,
        "mode": "build",
        "provider": "glm",
    }))
    .expect("meta")
}

fn seed(repo: &TaskIndexRepo, task_id: &str, workspace_path: &str, created_at: i64) {
    repo.sync_task_meta(SyncTaskMetaParams {
        meta: meta(task_id, workspace_path, created_at),
        pinned: None,
        archived: None,
        deleted: None,
        title_overridden: None,
        searchable_text: None,
    })
    .expect("seed");
}

async fn call(channel: &WindowControllerChannel, method: &str, arg: Json) -> Json {
    let value = channel
        .call(
            "ctx".into(),
            method.into(),
            Value::Json(arg.to_string()),
            CancellationToken::default(),
        )
        .await
        .expect("call");
    match value {
        Value::Json(text) => serde_json::from_str(&text).expect("JSON body"),
        other => panic!("期望 JSON body，实际 {other:?}"),
    }
}

fn address(task_id: &str) -> Json {
    json!({ "taskId": task_id, "workspacePath": "D:/work/app" })
}

#[tokio::test]
async fn list_task_list_projects_online_local_items() {
    let db = TempDb::new("list");
    let r = repo(&db);
    seed(&r, "task-1", "D:/work/app", 1_000);
    seed(&r, "task-2", "D:/work/app", 2_000);
    let channel = WindowControllerChannel::new(Arc::clone(&r));

    let result = call(
        &channel,
        "listTaskList",
        json!({
            "kind": "active",
            "workspaceScopes": [{ "workspacePath": "D:/work/app" }],
            "sortBy": "updated",
        }),
    )
    .await;
    assert_eq!(result["total"], 2);
    assert_eq!(result["hasMore"], false);
    assert_eq!(result["items"][0]["taskId"], "task-2");
    assert_eq!(result["items"][0]["sourceAvailability"], "online");
    assert_eq!(result["items"][0]["liveStatus"], "idle");

    // limit 生效。
    let result = call(
        &channel,
        "listTaskList",
        json!({
            "kind": "active",
            "workspaceScopes": [{ "workspacePath": "D:/work/app" }],
            "sortBy": "updated",
            "limit": 1,
        }),
    )
    .await;
    assert_eq!(result["items"].as_array().expect("items").len(), 1);
    assert_eq!(result["hasMore"], true);
}

#[tokio::test]
async fn mutations_map_to_index_patches() {
    let db = TempDb::new("mutations");
    let r = repo(&db);
    seed(&r, "task-1", "D:/work/app", 1_000);
    let channel = WindowControllerChannel::new(Arc::clone(&r));

    // pin。
    call(&channel, "mutateTask", json!({
        "address": address("task-1"),
        "mutation": { "kind": "pin", "pinned": true },
    }))
    .await;
    // archive。
    call(&channel, "mutateTask", json!({
        "address": address("task-1"),
        "mutation": { "kind": "archive", "archived": true },
    }))
    .await;
    let archived = call(
        &channel,
        "listTaskList",
        json!({
            "kind": "archived",
            "workspaceScopes": [{ "workspacePath": "D:/work/app" }],
            "sortBy": "updated",
        }),
    )
    .await;
    assert_eq!(archived["total"], 1);

    // unarchive + unpin：active 列表 SQL 排除 pinned 任务（pinned 只在 pinned 列表）。
    call(&channel, "mutateTask", json!({
        "address": address("task-1"),
        "mutation": { "kind": "archive", "archived": false },
    }))
    .await;
    call(&channel, "mutateTask", json!({
        "address": address("task-1"),
        "mutation": { "kind": "pin", "pinned": false },
    }))
    .await;
    let active = call(
        &channel,
        "listTaskList",
        json!({
            "kind": "active",
            "workspaceScopes": [{ "workspacePath": "D:/work/app" }],
            "sortBy": "updated",
        }),
    )
    .await;
    assert_eq!(active["total"], 1);

    // mark-unread → mark-read（期望值 CAS 语义：错误 expected 不清除）。
    call(&channel, "mutateTask", json!({
        "address": address("task-1"),
        "mutation": { "kind": "mark-unread" },
    }))
    .await;
    let meta = r
        .get_task_meta(&task_index_repo::TaskRef {
            workspace_path: "D:/work/app".to_string(),
            workspace_identity: None,
            task_id: "task-1".to_string(),
        })
        .expect("meta")
        .expect("exists");
    assert!(meta.unread_at.is_some());
    call(&channel, "mutateTask", json!({
        "address": address("task-1"),
        "mutation": { "kind": "mark-read", "expectedUnreadAt": 999 },
    }))
    .await;
    let meta = r
        .get_task_meta(&task_index_repo::TaskRef {
            workspace_path: "D:/work/app".to_string(),
            workspace_identity: None,
            task_id: "task-1".to_string(),
        })
        .expect("meta")
        .expect("exists");
    assert!(meta.unread_at.is_some(), "错误 expected 不清除 unread");
    call(&channel, "mutateTask", json!({
        "address": address("task-1"),
        "mutation": { "kind": "mark-read", "expectedUnreadAt": meta.unread_at.expect("unread") },
    }))
    .await;
    let meta = r
        .get_task_meta(&task_index_repo::TaskRef {
            workspace_path: "D:/work/app".to_string(),
            workspace_identity: None,
            task_id: "task-1".to_string(),
        })
        .expect("meta")
        .expect("exists");
    assert!(meta.unread_at.is_none(), "匹配 expected 清除 unread");

    // open/resume：地址校验（存在 → 返回 meta；不存在 → source 错误）。
    let opened = call(&channel, "mutateTask", json!({
        "address": address("task-1"),
        "mutation": { "kind": "open" },
    }))
    .await;
    assert_eq!(opened["taskId"], "task-1");
    let error = channel
        .call(
            "ctx".into(),
            "mutateTask".into(),
            Value::Json(
                json!({
                    "address": address("absent"),
                    "mutation": { "kind": "resume" },
                })
                .to_string(),
            ),
            CancellationToken::default(),
        )
        .await
        .expect_err("absent source");
    match error {
        rpc_host::ChannelError::Error { message, .. } => {
            assert!(message.contains("没有与任务地址匹配的 source"), "{message}");
        }
        other => panic!("期望 Error 变体，实际 {other:?}"),
    }

    // remote address：壳内 local 单源不可达。
    let error = channel
        .call(
            "ctx".into(),
            "mutateTask".into(),
            Value::Json(
                json!({
                    "address": { "taskId": "task-1", "workspacePath": "D:/work/app", "remoteSessionId": "r-1" },
                    "mutation": { "kind": "pin", "pinned": true },
                })
                .to_string(),
            ),
            CancellationToken::default(),
        )
        .await
        .expect_err("remote");
    match error {
        rpc_host::ChannelError::Error { message, .. } => {
            assert!(message.contains("远程 source"), "{message}");
        }
        other => panic!("期望 Error 变体，实际 {other:?}"),
    }
}

#[tokio::test]
async fn archived_deletion_single_and_batch() {
    let db = TempDb::new("deletion");
    let r = repo(&db);
    seed(&r, "task-1", "D:/work/app", 1_000);
    let channel = WindowControllerChannel::new(Arc::clone(&r));

    // 归档后删除单条。
    call(&channel, "mutateTask", json!({
        "address": address("task-1"),
        "mutation": { "kind": "archive", "archived": true },
    }))
    .await;
    let deleted = call(&channel, "deleteArchivedTask", json!({ "address": address("task-1") })).await;
    assert_eq!(deleted, json!(true));
    // 再次 deleteArchivedTask：已不存在 → false。
    let deleted = call(&channel, "deleteArchivedTask", json!({ "address": address("task-1") })).await;
    assert_eq!(deleted, json!(false));

    // 批量：未归档任务进入 skipped。
    seed(&r, "task-2", "D:/work/app", 2_000);
    let batch = call(
        &channel,
        "deleteArchivedTasks",
        json!({ "address": address("task-1"), "taskIds": ["task-2", "absent"] }),
    )
    .await;
    let skipped: Vec<&str> = batch["skippedTaskIds"]
        .as_array()
        .expect("skipped")
        .iter()
        .map(|value| value.as_str().expect("id"))
        .collect();
    assert_eq!(skipped.len(), 2, "未归档与不存在都进 skipped: {batch}");
    assert!(batch["deletedTaskIds"]
        .as_array()
        .expect("deleted")
        .is_empty());
}

#[tokio::test]
async fn controller_frame_subscriptions_report_missing_bridge() {
    let db = TempDb::new("frames");
    let channel = WindowControllerChannel::new(repo(&db));
    for method in [
        "subscribeControllerV4",
        "resyncControllerV4",
        "unsubscribeControllerV4",
    ] {
        let error = channel
            .call(
                "ctx".into(),
                method.into(),
                Value::Json("{}".to_string()),
                CancellationToken::default(),
            )
            .await
            .expect_err("frame bridge missing");
        match error {
            rpc_host::ChannelError::Error { message, .. } => {
                assert!(
                    message.contains("sessions-index frame bridge"),
                    "{method}: {message}"
                );
            }
            other => panic!("期望 Error 变体，实际 {other:?}"),
        }
    }
}

#[test]
fn channel_name_constant() {
    assert_eq!(WINDOW_CONTROLLER_CHANNEL, "window-controller");
}
