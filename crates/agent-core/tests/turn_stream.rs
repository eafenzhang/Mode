//! 流式 turn 循环（M2 Task 5A）：事件顺序与终态统计。
//!
//! 期望顺序来自 `docs/specs/tauri-agent-runtime.md` §3.5 的实测；本用例只比
//! `turn.started` / `model.streaming` / `tool.updated` / `turn.completed`——
//! oracle 的 `session.updated` 与 `streamRecovery.updated` 尚未产出，差异已登记。

use std::fs;
use std::path::PathBuf;

use serde_json::json;

use agent_core::provider::{ScriptStep, ScriptToolCall, ScriptedProvider};
use agent_core::session::Session;
use agent_core::{SessionEvent, SessionEventType};
use agent_protocol::session_event::{
    ModelStreamingKind, ModelStreamingPayload, TurnCompletedPayload,
};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mode-turn-stream-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn event_kinds(events: &[SessionEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect()
}

fn completed(events: &[SessionEvent]) -> TurnCompletedPayload {
    events
        .iter()
        .find(|event| event.event_type == SessionEventType::TurnCompleted)
        .and_then(|event| event.payload_as::<TurnCompletedPayload>().expect("能解码"))
        .expect("应有 turn.completed")
}

fn streaming(events: &[SessionEvent]) -> Vec<ModelStreamingPayload> {
    events
        .iter()
        .filter(|event| event.event_type == SessionEventType::ModelStreaming)
        .filter_map(|event| event.payload_as::<ModelStreamingPayload>().expect("能解码"))
        .collect()
}

#[test]
fn text_turn_streams_the_model_and_completes() {
    let dir = temp_dir("text");
    let provider = ScriptedProvider::new(vec![ScriptStep::Text("hello from mock".to_string())]);
    let mut session = Session::new("sess_stream", dir.to_string_lossy().to_string());
    let mut sink = Vec::new();

    session
        .run_turn("probe text turn", &provider, &mut sink)
        .expect("turn 应成功");

    assert_eq!(
        event_kinds(&sink),
        vec!["turn.started", "model.streaming", "turn.completed"]
    );
    let streaming = streaming(&sink);
    assert_eq!(streaming.len(), 1);
    assert_eq!(streaming[0].kind, ModelStreamingKind::TextDelta);
    assert_eq!(streaming[0].delta.as_deref(), Some("hello from mock"));

    let payload = completed(&sink);
    assert_eq!(payload.response, "hello from mock");
    assert_eq!(payload.token_count, 22, "10 input + 12 output");
    assert_eq!(payload.tool_call_count, 0);
    assert_eq!(payload.history_round_count, Some(1));
    let usage = payload.usage.expect("应有 usage");
    assert_eq!(usage["inputTokens"], json!(10));
    assert_eq!(usage["outputTokens"], json!(12));
    assert_eq!(usage["modelRequestCount"], json!(1));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn tool_turn_interleaves_model_streaming_and_tool_events() {
    let dir = temp_dir("tool");
    fs::write(dir.join("hello.txt"), "hello\n").expect("写种子文件");
    let path = dir.join("hello.txt");
    let provider = ScriptedProvider::new(vec![
        ScriptStep::ToolCalls(vec![ScriptToolCall {
            name: "Read".to_string(),
            input: json!({ "file_path": path.to_string_lossy() }),
        }]),
        ScriptStep::Text("tool turn done".to_string()),
    ]);
    let mut session = Session::new("sess_stream", dir.to_string_lossy().to_string());
    let mut sink = Vec::new();

    session
        .run_turn("probe tool turn", &provider, &mut sink)
        .expect("turn 应成功");

    assert_eq!(
        event_kinds(&sink),
        vec![
            "turn.started",
            "model.streaming",
            "model.streaming",
            "model.streaming",
            "model.streaming",
            "tool.updated",
            "tool.updated",
            "tool.updated",
            "tool.updated",
            "model.streaming",
            "turn.completed",
        ]
    );

    let streaming_kinds: Vec<ModelStreamingKind> = streaming(&sink)
        .into_iter()
        .map(|payload| payload.kind)
        .collect();
    assert_eq!(
        streaming_kinds,
        vec![
            ModelStreamingKind::ToolInputStart,
            ModelStreamingKind::ToolInputDelta,
            ModelStreamingKind::ToolInputEnd,
            ModelStreamingKind::ToolCall,
            ModelStreamingKind::TextDelta,
        ]
    );

    let payload = completed(&sink);
    assert_eq!(payload.response, "tool turn done");
    assert_eq!(payload.token_count, 52, "两轮模型请求合计");
    assert_eq!(payload.tool_call_count, 1);
    assert_eq!(payload.history_round_count, Some(2));
    assert_eq!(
        payload.usage.expect("应有 usage")["modelRequestCount"],
        json!(2)
    );

    // 历史：User → Assistant(tool_calls) → ToolResult → Assistant(text)
    assert_eq!(session.messages().len(), 4);

    let _ = fs::remove_dir_all(&dir);
}
