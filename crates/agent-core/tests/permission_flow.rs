//! 审批接线（M2 Task 6 Step 2）：allow / deny / allow_project 在会话循环里的形状。
//!
//! 期望来自 spec §3.3：allow 会继续 started→result；deny 只有 scheduled→batch
//! （`errorCount:1`，没有 started/result/error）；`allow_project` 之后的同路径不再请求。

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

use agent_core::permission::PermissionHandler;
use agent_core::provider::{ScriptStep, ScriptToolCall, ScriptedProvider};
use agent_core::session::Session;
use agent_core::CancelToken;
use agent_core::{SessionEvent, SessionEventType};
use agent_protocol::permission::{PermissionDecision, PermissionRequest, PermissionResponse};
use agent_protocol::session_event::ToolBatchPayload;
use agent_protocol::ExtraFields;

fn temp_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("mode-permission-flow-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn event_types(events: &[SessionEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect()
}

/// `permission.requested` 的**事件载荷**（C1 Task 11 起与反向请求 params 不同形：
/// 事件里没有 `sessionId`/`turnId`、多一个 `fullAccessSupported`，所以按 `Value` 断言）。
fn permission_requests(events: &[SessionEvent]) -> Vec<Value> {
    events
        .iter()
        .filter(|event| event.event_type == SessionEventType::PermissionRequested)
        .filter_map(|event| event.payload.clone())
        .collect()
}

fn batch(events: &[SessionEvent]) -> ToolBatchPayload {
    events
        .iter()
        .filter(|event| event.event_type == SessionEventType::ToolUpdated)
        .find_map(|event| {
            event
                .payload_as::<agent_protocol::session_event::ToolUpdatedPayload>()
                .expect("能解码")
                .and_then(|payload| match payload {
                    agent_protocol::session_event::ToolUpdatedPayload::Batch(batch) => Some(batch),
                    _ => None,
                })
        })
        .expect("应有 batch")
}

struct DenyAll;

impl PermissionHandler for DenyAll {
    fn request(&mut self, _request: &PermissionRequest) -> PermissionResponse {
        PermissionResponse {
            decision: PermissionDecision::Deny,
            reason: Some("denied by test".to_string()),
            modified_input: None,
            permission_updates: None,
            extra: ExtraFields::new(),
        }
    }
}

struct ProjectOnce;

impl PermissionHandler for ProjectOnce {
    fn request(&mut self, request: &PermissionRequest) -> PermissionResponse {
        // 每次都回 allow_project（第二项）：本用例只验证「同路径规则生效、异路径仍问」。
        request.options[1].response.clone()
    }
}

fn write_call(path: &str, content: &str) -> ScriptToolCall {
    ScriptToolCall {
        name: "Write".to_string(),
        input: json!({ "file_path": path, "content": content }),
    }
}

#[tokio::test]
async fn allow_runs_the_tool_after_the_permission_round_trip() {
    let dir = temp_dir("allow");
    let path = dir.join("a.txt");
    let provider = ScriptedProvider::new(vec![
        ScriptStep::ToolCalls(vec![write_call(&path.to_string_lossy(), "a\n")]),
        ScriptStep::Text("done".to_string()),
    ]);
    let mut session = Session::new("sess_perm", dir.to_string_lossy().to_string());
    let mut sink = Vec::new();

    session
        .run_turn("write it", &provider, &mut sink, &CancelToken::new())
        .await
        .expect("turn 应成功");

    assert_eq!(
        event_types(&sink),
        vec![
            "turn.started",
            "model.streaming",
            "model.streaming",
            "model.streaming",
            "model.streaming",
            "tool.updated",
            "permission.requested",
            "permission.resolved",
            "tool.updated",
            "tool.updated",
            "tool.updated",
            // C1.1-#2：写类工具成功后的 checkpoint（batch 之后、下一轮请求之前）。
            "checkpoint.created",
            "model.streaming",
            "turn.completed",
        ]
    );
    assert_eq!(fs::read_to_string(&path).expect("文件应存在"), "a\n");
    let requests = permission_requests(&sink);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["riskLevel"], "medium");
    // 事件载荷的差异形状（C1.1-#8）：没有 sessionId/turnId，`fullAccessSupported` 恒 true。
    assert!(requests[0].get("sessionId").is_none());
    assert!(requests[0].get("turnId").is_none());
    assert_eq!(requests[0]["fullAccessSupported"], json!(true));
    assert_eq!(
        requests[0]["options"]
            .as_array()
            .expect("options 是数组")
            .iter()
            .map(|option| option["optionId"].as_str().expect("optionId"))
            .collect::<Vec<_>>(),
        vec!["allow_once", "allow_project", "deny"]
    );
    assert_eq!(batch(&sink).success_count, 1);
}

#[tokio::test]
async fn deny_leaves_only_scheduled_and_batch() {
    let dir = temp_dir("deny");
    let path = dir.join("denied.txt");
    let provider = ScriptedProvider::new(vec![
        ScriptStep::ToolCalls(vec![write_call(&path.to_string_lossy(), "no\n")]),
        ScriptStep::Text("after deny".to_string()),
    ]);
    let mut session = Session::new("sess_perm", dir.to_string_lossy().to_string())
        .with_permission_handler(Box::new(DenyAll));
    let mut sink = Vec::new();

    session
        .run_turn("deny it", &provider, &mut sink, &CancelToken::new())
        .await
        .expect("turn 仍应成功");

    let tool_kinds: Vec<&str> = sink
        .iter()
        .filter(|event| event.event_type == SessionEventType::ToolUpdated)
        .map(|event| {
            event
                .payload_as::<agent_protocol::session_event::ToolUpdatedPayload>()
                .expect("能解码")
                .map(|payload| match payload {
                    agent_protocol::session_event::ToolUpdatedPayload::Scheduled(_) => "scheduled",
                    agent_protocol::session_event::ToolUpdatedPayload::Started(_) => "started",
                    agent_protocol::session_event::ToolUpdatedPayload::Result(_) => "result",
                    agent_protocol::session_event::ToolUpdatedPayload::Error(_) => "error",
                    agent_protocol::session_event::ToolUpdatedPayload::Progress(_) => "progress",
                    agent_protocol::session_event::ToolUpdatedPayload::Batch(_) => "batch",
                })
                .unwrap_or("other")
        })
        .collect();
    assert_eq!(tool_kinds, vec!["scheduled", "batch"]);
    assert!(!path.exists(), "deny 不应写文件");
    assert_eq!(batch(&sink).success_count, 0);
    assert_eq!(batch(&sink).error_count, 1);
    assert_eq!(
        event_types(&sink)
            .iter()
            .filter(|kind| **kind == "turn.completed")
            .count(),
        1
    );
}

#[tokio::test]
async fn allow_project_stops_asking_for_the_same_path() {
    let dir = temp_dir("project");
    let a = dir.join("a.txt");
    let b = dir.join("b.txt");
    let provider = ScriptedProvider::new(vec![
        ScriptStep::ToolCalls(vec![write_call(&a.to_string_lossy(), "a\n")]),
        ScriptStep::ToolCalls(vec![write_call(&a.to_string_lossy(), "a2\n")]),
        ScriptStep::ToolCalls(vec![write_call(&b.to_string_lossy(), "b\n")]),
        ScriptStep::Text("done".to_string()),
    ]);
    let mut session = Session::new("sess_perm", dir.to_string_lossy().to_string())
        .with_permission_handler(Box::new(ProjectOnce));
    let mut sink = Vec::new();

    session
        .run_turn("write three", &provider, &mut sink, &CancelToken::new())
        .await
        .expect("turn 应成功");

    let requests = permission_requests(&sink);
    assert_eq!(requests.len(), 2, "a.txt 第二次不应再请求，b.txt 应请求");
    let requested_paths: Vec<&Value> = requests
        .iter()
        .map(|request| &request["input"]["file_path"])
        .collect();
    assert_eq!(
        requested_paths,
        vec![&json!(a.to_string_lossy()), &json!(b.to_string_lossy())]
    );
    assert_eq!(fs::read_to_string(&a).expect("a 应存在"), "a2\n");
    assert_eq!(fs::read_to_string(&b).expect("b 应存在"), "b\n");
}
