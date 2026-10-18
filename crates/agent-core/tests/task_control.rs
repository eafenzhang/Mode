//! M2 Task 4b：`TaskOutput` / `TaskStop` 在**会话事件层**的顺序。
//!
//! 观察（`docs/specs/tauri-agent-runtime.md` §3.4，`probe-task-control.mts`）：
//! - `TaskOutput` 找不到任务：`scheduled` → `error`，**没有** `started`（启动前检查）；
//! - `TaskStop` 缺 `task_id`：`scheduled` → `started` → `error`（错误在 handler 内）；
//! - `TaskStop` 的 `started` 是 `readOnly:false` / `sideEffectScope:"session"`。

use serde_json::json;

use agent_core::session::{Session, ToolCallRequest};
use agent_core::{SessionEvent, ToolUpdatedPayload};

fn session() -> Session {
    Session::new(
        "sess_task_control",
        std::env::temp_dir().to_string_lossy().to_string(),
    )
}

/// 从事件流里折出 `tool.updated` 的 `kind` 序列。
fn kinds(events: &[SessionEvent]) -> Vec<&'static str> {
    let mut result = Vec::new();
    for event in events {
        let Some(payload) = event
            .payload_as::<ToolUpdatedPayload>()
            .expect("载荷能解码")
        else {
            continue;
        };
        result.push(match payload {
            ToolUpdatedPayload::Scheduled(_) => "scheduled",
            ToolUpdatedPayload::Started(_) => "started",
            ToolUpdatedPayload::Progress(_) => "progress",
            ToolUpdatedPayload::Result(_) => "result",
            ToolUpdatedPayload::Error(_) => "error",
            ToolUpdatedPayload::Batch(_) => "batch",
        });
    }
    result
}

#[test]
fn task_output_unknown_id_fails_before_started() {
    let mut session = session();
    session.begin_turn("task output unknown");
    let events = session.run_tool_batch(&[ToolCallRequest {
        tool_call_id: "call_1".to_string(),
        name: "TaskOutput".to_string(),
        input: json!({ "task_id": "exec_missing" }),
    }]);
    assert_eq!(kinds(&events), vec!["scheduled", "error", "batch"]);
}

#[test]
fn task_stop_missing_id_runs_then_errors_with_session_scope() {
    let mut session = session();
    session.begin_turn("task stop missing");
    let events = session.run_tool_batch(&[ToolCallRequest {
        tool_call_id: "call_2".to_string(),
        name: "TaskStop".to_string(),
        input: json!({}),
    }]);
    assert_eq!(
        kinds(&events),
        vec!["scheduled", "started", "error", "batch"]
    );

    let started = events
        .iter()
        .find_map(|event| {
            match event
                .payload_as::<ToolUpdatedPayload>()
                .expect("载荷能解码")
            {
                Some(ToolUpdatedPayload::Started(started)) => Some(started),
                _ => None,
            }
        })
        .expect("TaskStop 应有 started");
    assert_eq!(started.read_only, Some(false));
    assert_eq!(started.side_effect_scope.as_deref(), Some("session"));
}
