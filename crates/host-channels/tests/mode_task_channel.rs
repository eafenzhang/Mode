//! `mode-task` 频道首切行为契约（LT-16.1：读面 + 分组管理写面）。
//!
//! 真实临时 DB（`task-index-repo`）→ 频道调用 → 形状与语义断言。

use std::path::PathBuf;
use std::sync::Arc;

use rpc_host::{CancellationToken, ServerChannel as _};
use rpc_wire::Value;
use serde_json::{json, Value as Json};
use task_index_repo::{parse_mode_task_meta, ModeTaskMeta, SyncTaskMetaParams, TaskIndexRepo};

use host_channels::mode_task::{ModeTaskChannel, MODE_TASK_CHANNEL};

struct TempDb(PathBuf);

impl TempDb {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "mode-task-channel-{label}-{}-{}",
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

fn meta(task_id: &str, title: &str, workspace_path: &str, created_at: i64) -> ModeTaskMeta {
    parse_mode_task_meta(&json!({
        "taskId": task_id,
        "traceId": format!("trace-{task_id}"),
        "title": title,
        "workspacePath": workspace_path,
        "createdAt": created_at,
        "updatedAt": created_at,
        "mode": "build",
        // 与生产任务一致（TS mode-task 面按 provider=glm 过滤）。
        "provider": "glm",
    }))
    .expect("meta 合法")
}

fn seed_task(repo: &TaskIndexRepo, task_id: &str, title: &str, workspace_path: &str, created_at: i64) {
    let meta = meta(task_id, title, workspace_path, created_at);
    repo.sync_task_meta_at_grouped_top(SyncTaskMetaParams {
        meta,
        pinned: None,
        archived: None,
        deleted: None,
        title_overridden: None,
        searchable_text: Some(title.to_string()),
    })
    .expect("seed");
}

async fn call(channel: &ModeTaskChannel, method: &str, arg: Json) -> Json {
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

#[tokio::test]
async fn list_tasks_and_task_list_read_surface() {
    let db = TempDb::new("read");
    let r = repo(&db);
    seed_task(&r, "task-1", "实现登录页", "D:/work/app", 1_000);
    seed_task(&r, "task-2", "修复滚动条", "D:/work/app", 2_000);
    let channel = ModeTaskChannel::new(Arc::clone(&r), None, None);

    // listTasks：workspace 全量。
    let tasks = call(&channel, "listTasks", json!({ "workspacePath": "D:/work/app" })).await;
    assert_eq!(tasks.as_array().expect("array").len(), 2);
    assert_eq!(tasks[0]["taskId"], "task-2", "updated 降序");

    // listTaskList：active 列表（含 items/total/hasMore 形状）。
    let list = call(
        &channel,
        "listTaskList",
        json!({
            "kind": "active",
            "workspaceScopes": [{ "workspacePath": "D:/work/app" }],
            "sortBy": "updated",
            "limit": 10,
        }),
    )
    .await;
    assert_eq!(list["total"], 2);
    assert_eq!(list["hasMore"], false);
    assert_eq!(list["items"][0]["taskId"], "task-2");

    // 搜索命中 title。
    let list = call(
        &channel,
        "listTaskList",
        json!({
            "kind": "active",
            "workspaceScopes": [{ "workspacePath": "D:/work/app" }],
            "sortBy": "updated",
            "search": "登录",
        }),
    )
    .await;
    assert_eq!(list["total"], 1);
    assert_eq!(list["items"][0]["taskId"], "task-1");

    // listPinnedTaskIds：空。
    let pinned = call(&channel, "listPinnedTaskIds", json!({})).await;
    assert_eq!(pinned, json!([]));

    // listDeletedTaskIds：空。
    let deleted = call(
        &channel,
        "listDeletedTaskIds",
        json!({ "workspacePath": "D:/work/app" }),
    )
    .await;
    assert_eq!(deleted, json!([]));
}

#[tokio::test]
async fn group_crud_and_view_structure() {
    let db = TempDb::new("group");
    let r = repo(&db);
    seed_task(&r, "task-1", "实现登录页", "D:/work/app", 1_000);
    let channel = ModeTaskChannel::new(Arc::clone(&r), None, None);

    // 分组 CRUD。
    let group = call(
        &channel,
        "createTaskGroup",
        json!({ "title": "前端", "color": "blue" }),
    )
    .await;
    let group_id = group["id"].as_str().expect("group id").to_string();
    assert_eq!(group["title"], "前端");
    assert_eq!(group["color"], "blue");

    let renamed = call(
        &channel,
        "renameTaskGroup",
        json!({ "groupId": group_id, "title": "前端任务" }),
    )
    .await;
    assert_eq!(renamed["title"], "前端任务");

    let recolored = call(
        &channel,
        "updateTaskGroupColor",
        json!({ "groupId": group_id, "color": "red" }),
    )
    .await;
    assert_eq!(recolored["color"], "red");

    // 顶层排序：task 置顶 + group。
    let view = call(
        &channel,
        "applyGroupedTaskViewOrder",
        json!({
            "workspaceScopes": [{ "workspacePath": "D:/work/app" }],
            "topLevelNodes": [
                { "type": "group", "groupId": group_id },
                { "type": "task", "task": { "workspacePath": "D:/work/app", "taskId": "task-1" } },
            ],
            "groups": [
                { "groupId": group_id, "taskRefs": [ { "workspacePath": "D:/work/app", "taskId": "task-1" } ] },
            ],
        }),
    )
    .await;
    let nodes = view["nodes"].as_array().expect("nodes");
    assert!(matches!(nodes[0]["type"], Json::String(ref kind) if kind == "group"));

    // 结构读面：分组与成员可见。
    let structure = call(
        &channel,
        "listGroupedTaskViewStructure",
        json!({ "workspaceScopes": [{ "workspacePath": "D:/work/app" }] }),
    )
    .await;
    assert_eq!(structure["groups"][0]["id"], group_id);
    let members = structure["members"].as_array().expect("members");
    assert!(
        members
            .iter()
            .any(|member| member["taskId"] == "task-1" && member["groupId"] == group_id),
        "成员应含 task-1: {members:?}"
    );

    // 删除分组（TS 返回 void → wire Undefined）。
    let deleted = channel
        .call(
            "ctx".into(),
            "deleteTaskGroup".into(),
            Value::Json(json!({ "groupId": group_id }).to_string()),
            CancellationToken::default(),
        )
        .await
        .expect("delete");
    assert!(matches!(deleted, Value::Undefined));
    let structure = call(
        &channel,
        "listGroupedTaskViewStructure",
        json!({ "workspaceScopes": [{ "workspacePath": "D:/work/app" }] }),
    )
    .await;
    assert_eq!(structure["groups"].as_array().expect("groups").len(), 0);
}

#[tokio::test]
async fn archive_workspace_tasks_patches_and_lists() {
    let db = TempDb::new("archive-ws");
    let r = repo(&db);
    seed_task(&r, "task-1", "实现登录页", "D:/work/app", 1_000);
    seed_task(&r, "task-2", "修复滚动条", "D:/work/app", 2_000);
    let channel = ModeTaskChannel::new(Arc::clone(&r), None, None);

    let archived = call(
        &channel,
        "archiveWorkspaceTasks",
        json!({ "workspacePath": "D:/work/app" }),
    )
    .await;
    let mut ids: Vec<&str> = archived
        .as_array()
        .expect("array")
        .iter()
        .map(|meta| meta["taskId"].as_str().expect("id"))
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["task-1", "task-2"]);

    // 归档后 archived=true 列表可读、active 列表为空。
    let archived_list = call(
        &channel,
        "listArchivedTasks",
        json!({ "workspacePath": "D:/work/app" }),
    )
    .await;
    assert_eq!(archived_list.as_array().expect("array").len(), 2);
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
    assert_eq!(active["total"], 0);
}

#[tokio::test]
async fn unknown_or_lifecycle_methods_error_explicitly() {
    let db = TempDb::new("unknown");
    let channel = ModeTaskChannel::new(repo(&db), None, None);
    // 编排方法（16.2a）在无 agent 控制面装配时显式报错（可识别文案）。
    for method in ["createTask", "sendPrompt"] {
        let error = channel
            .call(
                "ctx".into(),
                method.into(),
                Value::Json(json!({ "workspacePath": "D:/work/app" }).to_string()),
                CancellationToken::default(),
            )
            .await
            .expect_err("no control plane assembled");
        match error {
            rpc_host::ChannelError::Error { message, .. } => {
                assert!(
                    message.contains("requires the agent control plane"),
                    "{method} 应显式 unsupported: {message}"
                );
            }
            other => panic!("期望 Error 变体，实际 {other:?}"),
        }
    }
    // 尚未迁移的编排面（16.2c）与未知方法同样显式报错。
    for method in ["getTaskSnapshot", "notARealMethod"] {
        let error = channel
            .call(
                "ctx".into(),
                method.into(),
                Value::Json("{}".to_string()),
                CancellationToken::default(),
            )
            .await
            .expect_err("not in this slice");
        match error {
            rpc_host::ChannelError::Error { message, .. } => {
                assert!(
                    message.contains("unknown or not-yet-migrated"),
                    "{method} 应显式 unsupported: {message}"
                );
            }
            other => panic!("期望 Error 变体，实际 {other:?}"),
        }
    }
}

#[test]
fn channel_name_constant() {
    assert_eq!(MODE_TASK_CHANNEL, "mode-task");
}
