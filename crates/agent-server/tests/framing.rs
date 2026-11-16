//! stdio 分帧：LF、CRLF、U+2028/U+2029、空行与 EOF 残留帧。
//!
//! 分帧行为真源：`docs/specs/mode-protocol-client.md`「帧」。

use std::sync::Arc;

use agent_server::{run_stdio, AgentServer, Frame, RequestId, ServerError};
use serde_json::{json, Value};

fn server() -> Arc<AgentServer> {
    Arc::new(AgentServer::new())
}

fn request_line(id: Value, method: &str, params: Value) -> String {
    serde_json::to_string(&json!({ "id": id, "method": method, "params": params }))
        .expect("请求帧必须可序列化")
}

async fn run(input: impl AsRef<[u8]>) -> (Result<(), ServerError>, Vec<Frame>) {
    let mut output = Vec::new();
    let result = run_stdio(input.as_ref(), &mut output, server()).await;
    let frames = output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice::<Frame>(line).expect("出站帧必须是合法 JSON"))
        .collect();
    (result, frames)
}

fn assert_method_not_found(frame: &Frame, expected_id: RequestId) {
    match frame {
        Frame::Error {
            id,
            code,
            message,
            data,
        } => {
            assert_eq!(*id, expected_id);
            assert_eq!(*code, -32601);
            assert_eq!(message, "Method not found");
            assert_eq!(*data, None);
        }
        other => panic!("期望 -32601 错误应答，实际 {other:?}"),
    }
}

#[tokio::test]
async fn lf_frames_are_dispatched_one_per_line() {
    let input = format!(
        "{}\n{}\n",
        request_line(json!(1), "session/unknown", json!({})),
        request_line(json!(2), "session/other", json!({ "x": 1 }))
    );
    let (result, frames) = run(input).await;

    assert!(result.is_ok(), "EOF 必须干净退出，实际 {result:?}");
    assert_eq!(frames.len(), 2);
    assert_method_not_found(&frames[0], RequestId::Number(1));
    assert_method_not_found(&frames[1], RequestId::Number(2));
}

#[tokio::test]
async fn crlf_is_tolerated() {
    let input = format!(
        "{}\r\n",
        request_line(json!("req-1"), "session/unknown", json!({}))
    );
    let (result, frames) = run(input).await;

    assert!(result.is_ok(), "CRLF 不应被当成非法 JSON：{result:?}");
    assert_eq!(frames.len(), 1);
    assert_method_not_found(&frames[0], RequestId::String("req-1".to_string()));
}

#[tokio::test]
async fn u2028_and_u2029_are_not_frame_boundaries() {
    let text = "before\u{2028}middle\u{2029}after";
    let input = format!(
        "{}\n{}\n",
        request_line(json!(1), "session/unknown", json!({ "text": text })),
        request_line(json!(2), "session/unknown", json!({}))
    );
    let (result, frames) = run(input).await;

    assert!(
        result.is_ok(),
        "U+2028/U+2029 是字符串内容，不能切帧：{result:?}"
    );
    assert_eq!(frames.len(), 2);
    assert_method_not_found(&frames[0], RequestId::Number(1));
    assert_method_not_found(&frames[1], RequestId::Number(2));
}

#[test]
fn u2028_and_u2029_survive_frame_parsing() {
    let text = "a\u{2028}b\u{2029}c";
    let line = request_line(json!(7), "session/unknown", json!({ "text": text }));
    let frame: Frame = serde_json::from_str(&line).expect("帧必须可解析");

    match frame {
        Frame::Request { id, params, .. } => {
            assert_eq!(id, RequestId::Number(7));
            assert_eq!(params["text"], text);
        }
        other => panic!("期望请求帧，实际 {other:?}"),
    }
}

#[tokio::test]
async fn blank_lines_are_ignored() {
    let input = format!(
        "\n\r\n\n{}\n\r\n",
        request_line(json!(7), "session/unknown", json!({}))
    );
    let (result, frames) = run(input).await;

    assert!(result.is_ok(), "空行必须忽略：{result:?}");
    assert_eq!(frames.len(), 1);
    assert_method_not_found(&frames[0], RequestId::Number(7));
}

#[tokio::test]
async fn eof_dispatches_residual_frame_without_newline() {
    let input = request_line(json!(9), "session/unknown", json!({}));
    let (result, frames) = run(input).await;

    assert!(result.is_ok(), "EOF 残留帧必须被处理：{result:?}");
    assert_eq!(frames.len(), 1);
    assert_method_not_found(&frames[0], RequestId::Number(9));
}

#[tokio::test]
async fn eof_without_input_exits_cleanly() {
    let (result, frames) = run("").await;

    assert!(result.is_ok(), "空输入 EOF 必须干净退出：{result:?}");
    assert!(frames.is_empty());
}
