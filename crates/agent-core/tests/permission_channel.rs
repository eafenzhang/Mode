//! `ChannelPermissionHandler`：宿主线程应答 allow / deny / 通道断开。
//!
//! 这是 stdio `interaction/requestPermission` 的进程内半边；帧映射（反向请求/应答与
//! id 关联）见 `permission_frame.rs`。

use std::fs;
use std::path::PathBuf;
use std::thread;

use serde_json::json;

use agent_core::permission::ChannelPermissionHandler;
use agent_core::provider::{ScriptStep, ScriptToolCall, ScriptedProvider};
use agent_core::session::Session;
use agent_core::{SessionEvent, SessionEventType};
use agent_protocol::session_event::ToolUpdatedPayload;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mode-permission-channel-{tag}-{}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn provider(path: &str) -> ScriptedProvider {
    ScriptedProvider::new(vec![
        ScriptStep::ToolCalls(vec![ScriptToolCall {
            name: "Write".to_string(),
            input: json!({ "file_path": path, "content": "x\n" }),
        }]),
        ScriptStep::Text("done".to_string()),
    ])
}

fn tool_kinds(events: &[SessionEvent]) -> Vec<&'static str> {
    events
        .iter()
        .filter(|event| event.event_type == SessionEventType::ToolUpdated)
        .map(|event| {
            event
                .payload_as::<ToolUpdatedPayload>()
                .expect("能解码")
                .map(|payload| match payload {
                    ToolUpdatedPayload::Scheduled(_) => "scheduled",
                    ToolUpdatedPayload::Started(_) => "started",
                    ToolUpdatedPayload::Result(_) => "result",
                    ToolUpdatedPayload::Error(_) => "error",
                    ToolUpdatedPayload::Progress(_) => "progress",
                    ToolUpdatedPayload::Batch(_) => "batch",
                })
                .unwrap_or("other")
        })
        .collect()
}

#[test]
fn host_reply_allow_lets_the_tool_run() {
    let dir = temp_dir("allow");
    let path = dir.join("a.txt");
    let path_text = path.to_string_lossy().to_string();
    let (handler, requests, responses) = ChannelPermissionHandler::pair();
    let worker = thread::spawn(move || {
        let request = requests.recv().expect("应收到审批请求");
        responses
            .send(request.options[0].response.clone())
            .expect("应能回 allow_once");
        request
    });

    let mut session = Session::new("sess_channel", dir.to_string_lossy().to_string())
        .with_permission_handler(Box::new(handler));
    let mut sink = Vec::new();
    session
        .run_turn("write it", &provider(&path_text), &mut sink)
        .expect("turn 应成功");

    let request = worker.join().expect("线程不应 panic");
    assert_eq!(request.tool_name, "Write");
    assert_eq!(request.risk_level, "medium");
    assert!(path.exists(), "allow 后应写入文件");
}

#[test]
fn host_reply_deny_keeps_the_observed_shape() {
    let dir = temp_dir("deny");
    let path = dir.join("denied.txt");
    let path_text = path.to_string_lossy().to_string();
    let (handler, requests, responses) = ChannelPermissionHandler::pair();
    let worker = thread::spawn(move || {
        let request = requests.recv().expect("应收到审批请求");
        responses
            .send(request.options[2].response.clone())
            .expect("应能回 deny");
    });

    let mut session = Session::new("sess_channel", dir.to_string_lossy().to_string())
        .with_permission_handler(Box::new(handler));
    let mut sink = Vec::new();
    session
        .run_turn("deny it", &provider(&path_text), &mut sink)
        .expect("turn 应成功");
    worker.join().expect("线程不应 panic");

    assert!(!path.exists(), "deny 不应写文件");
    assert_eq!(tool_kinds(&sink), vec!["scheduled", "batch"]);
}

#[test]
fn closed_channel_fails_closed() {
    let dir = temp_dir("closed");
    let path = dir.join("closed.txt");
    let path_text = path.to_string_lossy().to_string();
    let (handler, requests, _responses) = ChannelPermissionHandler::pair();
    drop(requests);

    let mut session = Session::new("sess_channel", dir.to_string_lossy().to_string())
        .with_permission_handler(Box::new(handler));
    let mut sink = Vec::new();
    session
        .run_turn("closed", &provider(&path_text), &mut sink)
        .expect("turn 应成功，不 panic");

    assert!(!path.exists(), "通道断开时应按 deny 处理");
    assert_eq!(tool_kinds(&sink), vec!["scheduled", "batch"]);
}
