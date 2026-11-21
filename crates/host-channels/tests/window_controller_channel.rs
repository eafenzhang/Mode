//! `window-controller` 频道首切行为契约（LT-16.3：local 单源列表 + 变更面）。
//!
//! 真实临时 DB（task-index-repo）→ 频道调用 → mutation 语义断言。

use std::path::PathBuf;
use std::sync::Arc;

use rpc_host::{CancellationToken, ServerChannel as _};
use rpc_wire::Value;
use serde_json::{json, Value as Json};
use task_index_repo::{parse_mode_task_meta, ModeTaskMeta, SyncTaskMetaParams, TaskIndexRepo};

use host_channels::window_controller::{
    WindowControllerChannel, CONTROLLER_TASKS_INDEX_TOPIC, CONTROLLER_WORKSPACES_TOPIC,
    ON_DYNAMIC_CONTROLLER_FRAME, WINDOW_CONTROLLER_CHANNEL,
};

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
    call(
        &channel,
        "mutateTask",
        json!({
            "address": address("task-1"),
            "mutation": { "kind": "pin", "pinned": true },
        }),
    )
    .await;
    // archive。
    call(
        &channel,
        "mutateTask",
        json!({
            "address": address("task-1"),
            "mutation": { "kind": "archive", "archived": true },
        }),
    )
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
    call(
        &channel,
        "mutateTask",
        json!({
            "address": address("task-1"),
            "mutation": { "kind": "archive", "archived": false },
        }),
    )
    .await;
    call(
        &channel,
        "mutateTask",
        json!({
            "address": address("task-1"),
            "mutation": { "kind": "pin", "pinned": false },
        }),
    )
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
    call(
        &channel,
        "mutateTask",
        json!({
            "address": address("task-1"),
            "mutation": { "kind": "mark-unread" },
        }),
    )
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
    call(
        &channel,
        "mutateTask",
        json!({
            "address": address("task-1"),
            "mutation": { "kind": "mark-read", "expectedUnreadAt": 999 },
        }),
    )
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
    let opened = call(
        &channel,
        "mutateTask",
        json!({
            "address": address("task-1"),
            "mutation": { "kind": "open" },
        }),
    )
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
    call(
        &channel,
        "mutateTask",
        json!({
            "address": address("task-1"),
            "mutation": { "kind": "archive", "archived": true },
        }),
    )
    .await;
    let deleted = call(
        &channel,
        "deleteArchivedTask",
        json!({ "address": address("task-1") }),
    )
    .await;
    assert_eq!(deleted, json!(true));
    // 再次 deleteArchivedTask：已不存在 → false。
    let deleted = call(
        &channel,
        "deleteArchivedTask",
        json!({ "address": address("task-1") }),
    )
    .await;
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

/// 注册 `onDynamicControllerFrame` 监听者并收集帧。
fn listen_frames(
    channel: &WindowControllerChannel,
) -> (Arc<std::sync::Mutex<Vec<Json>>>, rpc_host::Subscription) {
    let frames = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&frames);
    let subscription = channel
        .listen(
            "ctx".into(),
            ON_DYNAMIC_CONTROLLER_FRAME.into(),
            Value::Json("{}".to_string()),
            Arc::new(move |value| {
                if let Value::Json(text) = value {
                    sink.lock()
                        .expect("frames")
                        .push(serde_json::from_str(&text).expect("frame JSON"));
                }
            }),
        )
        .expect("listen");
    (frames, subscription)
}

fn take_frames(frames: &Arc<std::sync::Mutex<Vec<Json>>>) -> Vec<Json> {
    std::mem::take(&mut *frames.lock().expect("frames"))
}

#[tokio::test]
async fn controller_frames_snapshot_then_contiguous_deltas() {
    let db = TempDb::new("frames");
    let r = repo(&db);
    seed(&r, "task-1", "D:/work/app", 1_000);
    seed(&r, "task-2", "D:/work/app", 2_000);
    let channel = WindowControllerChannel::new(Arc::clone(&r));
    let (frames, _listener) = listen_frames(&channel);

    let subscribed = call(
        &channel,
        "subscribeControllerV4",
        json!({ "topic": "controller/tasks-index", "visibility": "foreground" }),
    )
    .await;
    let ack = &subscribed["ack"];
    assert_eq!(ack["mode"], "snapshot");
    let log_epoch = ack["logEpoch"].as_str().expect("logEpoch").to_string();
    assert!(!log_epoch.is_empty());
    let subscription_id = ack["subscriptionId"]
        .as_str()
        .expect("subscriptionId")
        .to_string();

    // 订阅后立即推送 snapshot 帧。
    let captured = take_frames(&frames);
    assert_eq!(captured.len(), 1, "{captured:?}");
    let snapshot = &captured[0];
    assert_eq!(snapshot["topic"], "controller/tasks-index");
    assert_eq!(snapshot["subscriptionId"], subscription_id.as_str());
    assert_eq!(snapshot["logEpoch"], log_epoch.as_str());
    assert_eq!(snapshot["payload"]["kind"], "snapshot");
    assert_eq!(snapshot["payload"]["snapshot"]["protocolVersion"], 1);
    assert_eq!(
        snapshot["payload"]["snapshot"]["logEpoch"],
        log_epoch.as_str()
    );
    // updatedAt 降序。
    assert_eq!(
        snapshot["payload"]["snapshot"]["tasks"][0]["meta"]["taskId"],
        "task-2"
    );
    assert_eq!(
        snapshot["payload"]["snapshot"]["tasks"][1]["meta"]["taskId"],
        "task-1"
    );
    assert_eq!(
        snapshot["payload"]["snapshot"]["tasks"][0]["membership"]["pinned"],
        false
    );
    let snapshot_to_seq = snapshot["toSeq"].as_u64().expect("toSeq");

    // mutation → delta 帧：fromSeq 必须等于上一帧 toSeq（Renderer gap 判定）。
    call(
        &channel,
        "mutateTask",
        json!({
            "address": address("task-1"),
            "mutation": { "kind": "pin", "pinned": true },
        }),
    )
    .await;
    let captured = take_frames(&frames);
    assert_eq!(captured.len(), 1, "{captured:?}");
    let delta = &captured[0];
    assert_eq!(delta["payload"]["kind"], "deltas");
    assert_eq!(delta["fromSeq"], snapshot_to_seq);
    assert_eq!(delta["toSeq"], snapshot_to_seq + 1);
    assert_eq!(delta["logEpoch"], log_epoch.as_str());
    let deltas = delta["payload"]["deltas"].as_array().expect("deltas");
    assert_eq!(deltas.len(), 1, "{deltas:?}");
    assert_eq!(deltas[0]["op"], "task.upserted");
    assert_eq!(deltas[0]["task"]["address"]["taskId"], "task-1");
    assert_eq!(deltas[0]["task"]["membership"]["pinned"], true);

    // resync：同一订阅重推 snapshot（seq 指到当前水位）。
    let resynced = call(
        &channel,
        "resyncControllerV4",
        json!({
            "subscriptionId": subscription_id,
            "base": null,
            "forceSnapshot": true,
        }),
    )
    .await;
    assert_eq!(resynced["ack"]["logEpoch"], log_epoch.as_str());
    let captured = take_frames(&frames);
    assert_eq!(captured.len(), 1, "{captured:?}");
    assert_eq!(captured[0]["payload"]["kind"], "snapshot");
    assert_eq!(captured[0]["fromSeq"], 0);
    assert_eq!(captured[0]["toSeq"], snapshot_to_seq + 1);

    // unsubscribe：Promise<void>，之后不再收帧。
    let value = channel
        .call(
            "ctx".into(),
            "unsubscribeControllerV4".into(),
            Value::Json(json!({ "subscriptionId": subscription_id }).to_string()),
            CancellationToken::default(),
        )
        .await
        .expect("unsubscribe");
    assert!(matches!(value, Value::Undefined), "{value:?}");
    call(
        &channel,
        "mutateTask",
        json!({
            "address": address("task-1"),
            "mutation": { "kind": "pin", "pinned": false },
        }),
    )
    .await;
    assert!(take_frames(&frames).is_empty(), "退订后不应再收帧");
}

#[tokio::test]
async fn controller_resync_unknown_subscription_and_topic_errors() {
    let db = TempDb::new("frame-errors");
    let channel = WindowControllerChannel::new(repo(&db));

    let error = channel
        .call(
            "ctx".into(),
            "resyncControllerV4".into(),
            Value::Json(json!({ "subscriptionId": "absent", "base": null }).to_string()),
            CancellationToken::default(),
        )
        .await
        .expect_err("unknown subscription");
    match error {
        rpc_host::ChannelError::Error { message, .. } => {
            assert!(
                message.contains("未找到 Controller subscription"),
                "{message}"
            );
        }
        other => panic!("期望 Error 变体，实际 {other:?}"),
    }

    let error = channel
        .call(
            "ctx".into(),
            "subscribeControllerV4".into(),
            Value::Json(json!({ "topic": "controller/unknown" }).to_string()),
            CancellationToken::default(),
        )
        .await
        .expect_err("unknown topic");
    match error {
        rpc_host::ChannelError::Error { message, .. } => {
            assert!(message.contains("unknown controller topic"), "{message}");
        }
        other => panic!("期望 Error 变体，实际 {other:?}"),
    }

    let error = match channel.listen(
        "ctx".into(),
        "onUnknownEvent".into(),
        Value::Json("{}".to_string()),
        Arc::new(|_| {}),
    ) {
        Ok(_) => panic!("unknown event 应当报错"),
        Err(error) => error,
    };
    match error {
        rpc_host::ChannelError::Error { message, .. } => {
            assert!(message.contains("unknown event"), "{message}");
        }
        other => panic!("期望 Error 变体，实际 {other:?}"),
    }
}

#[tokio::test]
async fn controller_workspaces_snapshot_lists_index_workspaces() {
    let db = TempDb::new("workspaces");
    let r = repo(&db);
    seed(&r, "task-1", "D:/work/app", 1_000);
    r.sync_task_meta(SyncTaskMetaParams {
        meta: parse_mode_task_meta(&json!({
            "taskId": "task-2",
            "traceId": "trace-task-2",
            "title": "远程任务",
            "workspacePath": "D:/work/other",
            "workspaceIdentity": "remote-1",
            "createdAt": 2_000,
            "updatedAt": 2_000,
            "mode": "build",
            "provider": "glm",
        }))
        .expect("meta"),
        pinned: None,
        archived: None,
        deleted: None,
        title_overridden: None,
        searchable_text: None,
    })
    .expect("seed remote identity");
    let channel = WindowControllerChannel::new(Arc::clone(&r));
    let (frames, _listener) = listen_frames(&channel);

    let subscribed = call(
        &channel,
        "subscribeControllerV4",
        json!({ "topic": "controller/workspaces" }),
    )
    .await;
    assert_eq!(subscribed["ack"]["mode"], "snapshot");
    let captured = take_frames(&frames);
    assert_eq!(captured.len(), 1, "{captured:?}");
    let frame = &captured[0];
    assert_eq!(frame["topic"], "controller/workspaces");
    assert_eq!(frame["payload"]["kind"], "snapshot");
    let workspaces = frame["payload"]["snapshot"]["workspaces"]
        .as_array()
        .expect("workspaces");
    assert_eq!(workspaces.len(), 2, "{workspaces:?}");
    assert!(workspaces.iter().all(|fact| {
        fact["sourceAvailability"] == "online" && fact["connectionState"] == "online"
    }));
    assert!(workspaces.iter().any(|fact| {
        fact["workspacePath"] == "D:/work/other" && fact["workspaceIdentity"] == "remote-1"
    }));
}

#[tokio::test]
async fn task_event_bridge_refreshes_frames_after_external_write() {
    let db = TempDb::new("bridge");
    let r = repo(&db);
    seed(&r, "task-1", "D:/work/app", 1_000);
    let events = Arc::new(host_channels::mode_task::TaskEventBus::new());
    let channel = WindowControllerChannel::with_task_events(Arc::clone(&r), Arc::clone(&events));
    let (frames, _listener) = listen_frames(&channel);

    let subscribed = call(
        &channel,
        "subscribeControllerV4",
        json!({ "topic": "controller/tasks-index" }),
    )
    .await;
    assert_eq!(subscribed["ack"]["mode"], "snapshot");
    let captured = take_frames(&frames);
    assert_eq!(captured.len(), 1);
    let snapshot_to_seq = captured[0]["toSeq"].as_u64().expect("toSeq");
    assert_eq!(
        captured[0]["payload"]["snapshot"]["tasks"]
            .as_array()
            .expect("tasks")
            .len(),
        1
    );

    // 外部写入索引（模拟 mode-task createTask）+ 事件通知 → 桥触发 refresh。
    seed(&r, "task-2", "D:/work/app", 2_000);
    events.notify_task_list(&host_channels::mode_task::WorkspaceTaskListChanged::new(
        "D:/work/app",
        "task_created",
    ));
    let captured = take_frames(&frames);
    assert_eq!(captured.len(), 1, "{captured:?}");
    let delta = &captured[0];
    assert_eq!(delta["payload"]["kind"], "deltas");
    assert_eq!(delta["fromSeq"], snapshot_to_seq);
    assert_eq!(delta["toSeq"], snapshot_to_seq + 1);
    let deltas = delta["payload"]["deltas"].as_array().expect("deltas");
    assert_eq!(deltas.len(), 1, "{deltas:?}");
    assert_eq!(deltas[0]["op"], "task.upserted");
    assert_eq!(deltas[0]["task"]["address"]["taskId"], "task-2");
}

#[tokio::test]
async fn mutations_publish_workspace_task_events() {
    let db = TempDb::new("emit");
    let r = repo(&db);
    seed(&r, "task-1", "D:/work/app", 1_000);
    let events = Arc::new(host_channels::mode_task::TaskEventBus::new());
    let channel = WindowControllerChannel::with_task_events(Arc::clone(&r), Arc::clone(&events));
    let received = Arc::new(std::sync::Mutex::new(Vec::<Json>::new()));
    let sink = Arc::clone(&received);
    let _sub = events.subscribe_task_list(
        host_channels::mode_task::WorkspaceEventScope::default(),
        Arc::new(move |value| {
            if let Value::Json(text) = value {
                sink.lock()
                    .expect("events")
                    .push(serde_json::from_str(&text).expect("event JSON"));
            }
        }),
    );

    // pin → task_pinned（带 meta）。
    call(&channel, "mutateTask", json!({
        "address": address("task-1"),
        "mutation": { "kind": "pin", "pinned": true },
    }))
    .await;
    {
        let received = received.lock().expect("events");
        assert_eq!(received.len(), 1, "{received:?}");
        assert_eq!(received[0]["type"], "workspace_task_list_changed");
        assert_eq!(received[0]["workspacePath"], "D:/work/app");
        assert_eq!(received[0]["taskId"], "task-1");
        assert_eq!(received[0]["reason"], "task_pinned");
        assert_eq!(received[0]["taskMeta"]["taskId"], "task-1");
    }

    // mark-unread → task_meta_changed；CAS 未命中的 mark-read 不广播。
    call(&channel, "mutateTask", json!({
        "address": address("task-1"),
        "mutation": { "kind": "mark-unread" },
    }))
    .await;
    call(&channel, "mutateTask", json!({
        "address": address("task-1"),
        "mutation": { "kind": "mark-read", "expectedUnreadAt": 999 },
    }))
    .await;
    {
        let received = received.lock().expect("events");
        assert_eq!(received.len(), 2, "{received:?}");
        assert_eq!(received[1]["reason"], "task_meta_changed");
    }

    // 归档删除 → task_deleted。
    call(&channel, "mutateTask", json!({
        "address": address("task-1"),
        "mutation": { "kind": "archive", "archived": true },
    }))
    .await;
    call(&channel, "deleteArchivedTask", json!({ "address": address("task-1") })).await;
    {
        let received = received.lock().expect("events");
        let last = received.last().expect("event");
        assert_eq!(last["reason"], "task_deleted");
        assert_eq!(last["taskId"], "task-1");
    }
}

#[tokio::test]
async fn session_overlays_enrich_rows_and_frames() {
    let db = TempDb::new("overlay");
    let r = repo(&db);
    // DB 里是运行的残留状态 + 普通标题：无 overlay 时 liveStatus 必须回 idle。
    r.sync_task_meta(SyncTaskMetaParams {
        meta: parse_mode_task_meta(&json!({
            "taskId": "task-1",
            "traceId": "trace-task-1",
            "title": "DB 标题",
            "workspacePath": "D:/work/app",
            "createdAt": 1_000,
            "updatedAt": 1_000,
            "mode": "build",
            "provider": "glm",
            "status": "running",
        }))
        .expect("meta"),
        pinned: None,
        archived: None,
        deleted: None,
        title_overridden: None,
        searchable_text: None,
    })
    .expect("seed");
    let channel = WindowControllerChannel::new(Arc::clone(&r));
    let (frames, _listener) = listen_frames(&channel);

    call(
        &channel,
        "subscribeControllerV4",
        json!({ "topic": "controller/tasks-index" }),
    )
    .await;
    let captured = take_frames(&frames);
    assert_eq!(
        captured[0]["payload"]["snapshot"]["tasks"][0]["liveStatus"],
        "idle"
    );
    assert_eq!(
        captured[0]["payload"]["snapshot"]["tasks"][0]["meta"]["title"],
        "DB 标题"
    );

    channel
        .replace_session_overlays(
            "D:/work/app",
            None,
            vec![json!({
                "taskId": "task-1",
                "liveStatus": "waiting",
                "title": "实时标题",
                "titleSource": "custom",
                "updatedAt": 3_000,
                "pendingInteraction": {
                    "interactionId": "i1",
                    "kind": "permission",
                    "toolName": "bash"
                },
                "activity": {
                    "phase": "completedSuccess",
                    "lastActivityAt": 3_000,
                    "hasBackgroundWork": false
                }
            })],
        )
        .expect("replace overlays");
    let captured = take_frames(&frames);
    assert_eq!(captured.len(), 1, "{captured:?}");
    let deltas = captured[0]["payload"]["deltas"].as_array().expect("deltas");
    assert_eq!(deltas.len(), 1, "{deltas:?}");
    assert_eq!(deltas[0]["op"], "task.upserted");
    let row = &deltas[0]["task"];
    assert_eq!(row["liveStatus"], "waiting");
    assert_eq!(row["meta"]["title"], "实时标题");
    assert_eq!(row["meta"]["titleOverridden"], true);
    assert_eq!(row["meta"]["updatedAt"], 3_000);
    assert_eq!(row["meta"]["status"], "completed");
    assert_eq!(row["meta"]["pendingInteraction"]["interactionId"], "i1");
    assert_eq!(row["activity"]["phase"], "completedSuccess");

    // 列表投影同样富化。
    let listed = call(
        &channel,
        "listTaskList",
        json!({
            "kind": "active",
            "workspaceScopes": [{ "workspacePath": "D:/work/app" }],
            "sortBy": "updated",
        }),
    )
    .await;
    assert_eq!(listed["items"][0]["liveStatus"], "waiting");
    assert_eq!(listed["items"][0]["title"], "实时标题");
    assert_eq!(listed["items"][0]["activity"]["phase"], "completedSuccess");

    // 清空 overlay：回到 DB 值（running 视为残留 → idle）。
    channel
        .replace_session_overlays("D:/work/app", None, Vec::new())
        .expect("clear overlays");
    let listed = call(
        &channel,
        "listTaskList",
        json!({
            "kind": "active",
            "workspaceScopes": [{ "workspacePath": "D:/work/app" }],
            "sortBy": "updated",
        }),
    )
    .await;
    assert_eq!(listed["items"][0]["liveStatus"], "idle");
    assert_eq!(listed["items"][0]["title"], "DB 标题");
    assert!(listed["items"][0].get("activity").is_none());
}

#[test]
fn channel_name_constant() {
    assert_eq!(WINDOW_CONTROLLER_CHANNEL, "window-controller");
    assert_eq!(ON_DYNAMIC_CONTROLLER_FRAME, "onDynamicControllerFrame");
    assert_eq!(CONTROLLER_TASKS_INDEX_TOPIC, "controller/tasks-index");
    assert_eq!(CONTROLLER_WORKSPACES_TOPIC, "controller/workspaces");
}
