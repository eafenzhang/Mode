//! 方法分派与帧形状：未知方法 `-32601`、非法 JSON 关闭连接、四类帧线上形状。

use std::sync::Arc;

use agent_server::{run_stdio, AgentServer, Frame, RequestId, ServerError};
use serde_json::json;

#[test]
fn unknown_method_returns_method_not_found() {
    let server = AgentServer::new();
    let error = server
        .dispatch("session/unknown", json!({}))
        .expect_err("骨架阶段没有已实现的方法");

    assert_eq!(error.code(), -32601);
    assert_eq!(error.message(), "Method not found");
    assert_eq!(error.data(), None);
}

#[test]
fn skeleton_does_not_fake_any_method() {
    let server = AgentServer::new();
    for method in [
        "",
        "session/create",
        "session/send",
        "session/read",
        "session/compact",
    ] {
        let error = server
            .dispatch(method, json!({}))
            .expect_err("C1 之前所有方法都必须是显式未实现");
        assert_eq!(error.code(), -32601, "method={method}");
    }
}

#[tokio::test]
async fn stdio_request_gets_an_error_frame() {
    let input = b"{\"id\":1,\"method\":\"session/unknown\",\"params\":{}}\n";
    let mut output = Vec::new();
    run_stdio(&input[..], &mut output, Arc::new(AgentServer::new()))
        .await
        .expect("EOF 必须干净退出");

    let line = output.strip_suffix(b"\n").expect("应答必须以 LF 结尾");
    let frame: Frame = serde_json::from_slice(line).expect("应答必须是合法帧");
    assert_eq!(
        frame,
        Frame::Error {
            id: RequestId::Number(1),
            code: -32601,
            message: "Method not found".to_string(),
            data: None,
        }
    );
}

#[tokio::test]
async fn invalid_json_closes_the_connection() {
    let input = b"{\"id\":1,\"method\":\n{\"id\":2,\"method\":\"session/unknown\"}\n";
    let mut output = Vec::new();
    let error = run_stdio(&input[..], &mut output, Arc::new(AgentServer::new()))
        .await
        .expect_err("非法 JSON 必须关闭连接");

    assert!(
        matches!(error, ServerError::InvalidFrame { .. }),
        "实际错误：{error:?}"
    );
    assert!(output.is_empty(), "协议错误后不得继续写出帧");
}

#[tokio::test]
async fn non_object_frame_closes_the_connection() {
    let mut output = Vec::new();
    let error = run_stdio(&b"[]\n"[..], &mut output, Arc::new(AgentServer::new()))
        .await
        .expect_err("非对象帧必须关闭连接");

    assert!(
        matches!(error, ServerError::InvalidFrame { .. }),
        "实际错误：{error:?}"
    );
    assert!(output.is_empty());
}

#[test]
fn frame_serialization_matches_wire_shapes() {
    let request = Frame::Request {
        id: RequestId::Number(1),
        method: "session/read".to_string(),
        params: json!({ "sessionId": "sess-1" }),
    };
    assert_eq!(
        serde_json::to_value(&request).expect("request 可序列化"),
        json!({ "id": 1, "method": "session/read", "params": { "sessionId": "sess-1" } })
    );

    let notification = Frame::Notification {
        method: "session/event".to_string(),
        params: json!({ "eventId": "ev-1" }),
    };
    assert_eq!(
        serde_json::to_value(&notification).expect("notification 可序列化"),
        json!({ "method": "session/event", "params": { "eventId": "ev-1" } })
    );

    let response = Frame::Response {
        id: RequestId::String("server-1".to_string()),
        result: json!({ "ok": true }),
    };
    assert_eq!(
        serde_json::to_value(&response).expect("response 可序列化"),
        json!({ "id": "server-1", "result": { "ok": true } })
    );

    let error = Frame::Error {
        id: RequestId::Number(3),
        code: -32601,
        message: "Method not found".to_string(),
        data: Some(json!({ "method": "session/unknown" })),
    };
    assert_eq!(
        serde_json::to_value(&error).expect("error 可序列化"),
        json!({
            "id": 3,
            "error": {
                "code": -32601,
                "message": "Method not found",
                "data": { "method": "session/unknown" }
            }
        })
    );
}

#[test]
fn frame_deserialization_covers_all_wire_shapes() {
    assert_eq!(
        serde_json::from_value::<Frame>(
            json!({ "id": 1, "method": "session/read", "params": { "sessionId": "s" } })
        )
        .expect("request 可解析"),
        Frame::Request {
            id: RequestId::Number(1),
            method: "session/read".to_string(),
            params: json!({ "sessionId": "s" }),
        }
    );
    assert_eq!(
        serde_json::from_value::<Frame>(json!({ "method": "session/event" }))
            .expect("notification 可解析"),
        Frame::Notification {
            method: "session/event".to_string(),
            params: json!(null),
        }
    );
    assert_eq!(
        serde_json::from_value::<Frame>(json!({ "id": "server-1", "result": null }))
            .expect("response 可解析"),
        Frame::Response {
            id: RequestId::String("server-1".to_string()),
            result: json!(null),
        }
    );
    assert_eq!(
        serde_json::from_value::<Frame>(
            json!({ "id": 2, "error": { "code": -32601, "message": "Method not found" } })
        )
        .expect("error 可解析"),
        Frame::Error {
            id: RequestId::Number(2),
            code: -32601,
            message: "Method not found".to_string(),
            data: None,
        }
    );
}

#[test]
fn malformed_frames_are_invalid_frames() {
    for value in [
        json!(null),
        json!([]),
        json!("text"),
        json!({}),
        json!({ "id": true, "method": "session/read" }),
        json!({ "id": 1 }),
        json!({ "id": 1, "error": { "code": "oops", "message": "bad" } }),
        json!({ "id": 1, "error": { "code": -1 } }),
    ] {
        let error = Frame::from_value(value.clone()).expect_err("非法帧形状必须报错");
        assert!(
            matches!(error, ServerError::InvalidFrame { .. }),
            "value={value}, error={error:?}"
        );
    }
}
