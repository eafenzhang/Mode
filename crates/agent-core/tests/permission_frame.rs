//! `FramePermissionHandler`：审批 channel 与 stdio `interaction/requestPermission` 的映射。
//!
//! 形状与失败语义出处：`docs/specs/tauri-agent-runtime.md` §3.3「宿主帧映射」（2026-10-22）。
//! 进程内 channel 的半边另见 `permission_channel.rs`；这里只验证帧这一层。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread;

use serde_json::json;

use agent_core::permission::FramePermissionHandler;
use agent_core::provider::{ScriptStep, ScriptToolCall, ScriptedProvider};
use agent_core::session::Session;
use agent_core::{SessionEvent, SessionEventType};
use agent_protocol::frame::{
    ErrorObject, ErrorResponseFrame, Frame, RequestId, ResponseFrame, ReverseRequestIds,
};
use agent_protocol::permission::{PermissionRequest, REQUEST_PERMISSION_METHOD};
use agent_protocol::session_event::ToolUpdatedPayload;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mode-permission-frame-{tag}-{}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn provider(paths: &[String]) -> ScriptedProvider {
    ScriptedProvider::new(vec![
        ScriptStep::ToolCalls(
            paths
                .iter()
                .map(|path| ScriptToolCall {
                    name: "Write".to_string(),
                    input: json!({ "file_path": path, "content": "x\n" }),
                })
                .collect(),
        ),
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

/// 从宿主侧通道取一帧，并断言它是一条可解码的审批反向请求。
fn recv_request(frames: &Receiver<Frame>) -> (RequestId, PermissionRequest) {
    let frame = frames.recv().expect("应收到反向请求帧");
    let Frame::Request(request) = frame else {
        panic!("应是请求帧");
    };
    assert_eq!(request.method, REQUEST_PERMISSION_METHOD);
    let parsed = PermissionRequest::from_request_frame(&request).expect("params 应能解码");
    (request.id, parsed)
}

#[test]
fn frame_allow_round_trip_lets_the_tool_run() {
    let dir = temp_dir("allow");
    let path = dir.join("a.txt");
    let (handler, frames, replies) = FramePermissionHandler::pair();
    let worker = thread::spawn(move || {
        let (id, request) = recv_request(&frames);
        assert_eq!(id, RequestId::String("server-1".to_string()));
        assert_eq!(request.tool_name, "Write");
        assert_eq!(request.risk_level, "medium");
        assert_eq!(request.options.len(), 3);
        let reply = request.options[0]
            .response
            .to_response_frame(id)
            .expect("应能构造应答帧");
        replies.send(reply).expect("应能回 allow_once");
        request
    });

    let mut session = Session::new("sess_frame_allow", dir.to_string_lossy().to_string())
        .with_permission_handler(Box::new(handler));
    let mut sink = Vec::new();
    session
        .run_turn(
            "write it",
            &provider(&[path.to_string_lossy().to_string()]),
            &mut sink,
        )
        .expect("turn 应成功");

    let parsed = worker.join().expect("线程不应 panic");
    assert!(
        parsed.request_id.starts_with("perm_"),
        "业务 requestId 仍由会话分配"
    );
    assert!(path.exists(), "allow 后应写入文件");
}

#[test]
fn error_frame_fails_closed() {
    let dir = temp_dir("error");
    let path = dir.join("denied.txt");
    let (handler, frames, replies) = FramePermissionHandler::pair();
    let worker = thread::spawn(move || {
        let (id, _) = recv_request(&frames);
        replies
            .send(Frame::ErrorResponse(ErrorResponseFrame {
                id,
                error: ErrorObject {
                    code: -32000,
                    message: "policy denied".to_string(),
                    data: None,
                },
            }))
            .expect("应能回错误应答");
    });

    let mut session = Session::new("sess_frame_error", dir.to_string_lossy().to_string())
        .with_permission_handler(Box::new(handler));
    let mut sink = Vec::new();
    session
        .run_turn(
            "deny it",
            &provider(&[path.to_string_lossy().to_string()]),
            &mut sink,
        )
        .expect("turn 应成功");
    worker.join().expect("线程不应 panic");

    assert!(!path.exists(), "错误应答应按 deny 处理");
    assert_eq!(tool_kinds(&sink), vec!["scheduled", "batch"]);
}

#[test]
fn malformed_result_fails_closed() {
    let dir = temp_dir("malformed");
    let path = dir.join("malformed.txt");
    let (handler, frames, replies) = FramePermissionHandler::pair();
    let worker = thread::spawn(move || {
        let (id, _) = recv_request(&frames);
        replies
            .send(Frame::Response(ResponseFrame {
                id,
                result: json!({}),
            }))
            .expect("应能回畸形应答");
    });

    let mut session = Session::new("sess_frame_malformed", dir.to_string_lossy().to_string())
        .with_permission_handler(Box::new(handler));
    let mut sink = Vec::new();
    session
        .run_turn(
            "deny it",
            &provider(&[path.to_string_lossy().to_string()]),
            &mut sink,
        )
        .expect("turn 应成功");
    worker.join().expect("线程不应 panic");

    assert!(!path.exists(), "result 解不开时应按 deny 处理");
    assert_eq!(tool_kinds(&sink), vec!["scheduled", "batch"]);
}

#[test]
fn responses_with_other_ids_are_ignored() {
    let dir = temp_dir("other-id");
    let path = dir.join("other-id.txt");
    let (handler, frames, replies) = FramePermissionHandler::pair();
    let worker = thread::spawn(move || {
        let (id, request) = recv_request(&frames);
        // 先投一条属于别的 pending 请求的 allow，再投本条请求的 deny。
        replies
            .send(Frame::Response(ResponseFrame {
                id: RequestId::String("server-999".to_string()),
                result: serde_json::to_value(&request.options[0].response).unwrap(),
            }))
            .expect("应能投无关应答");
        replies
            .send(
                request.options[2]
                    .response
                    .to_response_frame(id)
                    .expect("应能构造 deny 应答帧"),
            )
            .expect("应能回 deny");
    });

    let mut session = Session::new("sess_frame_other_id", dir.to_string_lossy().to_string())
        .with_permission_handler(Box::new(handler));
    let mut sink = Vec::new();
    session
        .run_turn(
            "deny it",
            &provider(&[path.to_string_lossy().to_string()]),
            &mut sink,
        )
        .expect("turn 应成功");
    worker.join().expect("线程不应 panic");

    assert!(!path.exists(), "无关 id 的 allow 不得被当成本条请求的答复");
    assert_eq!(tool_kinds(&sink), vec!["scheduled", "batch"]);
}

#[test]
fn closed_frame_channel_fails_closed() {
    let dir = temp_dir("closed");
    let path = dir.join("closed.txt");
    let (handler, frames, _replies) = FramePermissionHandler::pair();
    drop(frames);

    let mut session = Session::new("sess_frame_closed", dir.to_string_lossy().to_string())
        .with_permission_handler(Box::new(handler));
    let mut sink = Vec::new();
    session
        .run_turn(
            "closed",
            &provider(&[path.to_string_lossy().to_string()]),
            &mut sink,
        )
        .expect("turn 应成功，不 panic");

    assert!(!path.exists(), "帧通道断开时应按 deny 处理");
    assert_eq!(tool_kinds(&sink), vec!["scheduled", "batch"]);
}

#[test]
fn request_ids_are_monotonic_across_a_batch() {
    let dir = temp_dir("batch");
    let first = dir.join("first.txt");
    let second = dir.join("second.txt");
    let (handler, frames, replies) = FramePermissionHandler::pair();
    let worker = thread::spawn(move || {
        for expected in ["server-1", "server-2"] {
            let (id, request) = recv_request(&frames);
            assert_eq!(id, RequestId::String(expected.to_string()));
            let reply = request.options[0]
                .response
                .to_response_frame(id)
                .expect("应能构造应答帧");
            replies.send(reply).expect("应能回 allow_once");
        }
    });

    let mut session = Session::new("sess_frame_batch", dir.to_string_lossy().to_string())
        .with_permission_handler(Box::new(handler));
    let mut sink = Vec::new();
    session
        .run_turn(
            "write both",
            &provider(&[
                first.to_string_lossy().to_string(),
                second.to_string_lossy().to_string(),
            ]),
            &mut sink,
        )
        .expect("turn 应成功");
    worker.join().expect("线程不应 panic");

    assert!(first.exists(), "第一条 allow 应写入");
    assert!(second.exists(), "第二条 allow 应写入");
}

fn run_session(name: &str, dir: &Path, handler: FramePermissionHandler, path: &Path) {
    let mut session = Session::new(name, dir.to_string_lossy().to_string())
        .with_permission_handler(Box::new(handler));
    let mut sink = Vec::new();
    session
        .run_turn(
            "write it",
            &provider(&[path.to_string_lossy().to_string()]),
            &mut sink,
        )
        .expect("turn 应成功");
}

#[test]
fn sessions_share_the_reverse_request_id_space() {
    let dir = temp_dir("shared-ids");
    let first_path = dir.join("first.txt");
    let second_path = dir.join("second.txt");
    let ids = Arc::new(ReverseRequestIds::default());

    let (first_handler, first_frames, first_replies) =
        FramePermissionHandler::pair_with_ids(ids.clone());
    let first_worker = thread::spawn(move || {
        let (id, request) = recv_request(&first_frames);
        assert_eq!(id, RequestId::String("server-1".to_string()));
        first_replies
            .send(
                request.options[0]
                    .response
                    .to_response_frame(id)
                    .expect("应能构造应答帧"),
            )
            .expect("应能回 allow_once");
    });
    run_session("sess_shared_1", &dir, first_handler, &first_path);
    first_worker.join().expect("first worker 不应 panic");

    let (second_handler, second_frames, second_replies) =
        FramePermissionHandler::pair_with_ids(ids);
    let second_worker = thread::spawn(move || {
        let (id, request) = recv_request(&second_frames);
        assert_eq!(id, RequestId::String("server-2".to_string()));
        second_replies
            .send(
                request.options[0]
                    .response
                    .to_response_frame(id)
                    .expect("应能构造应答帧"),
            )
            .expect("应能回 allow_once");
    });
    run_session("sess_shared_2", &dir, second_handler, &second_path);
    second_worker.join().expect("second worker 不应 panic");

    assert!(
        first_path.exists() && second_path.exists(),
        "两个会话的 allow 都应落盘"
    );
}
