//! 插件宿主 NDJSON 协议的往返与拒收（M3 生态域 1 · Task 2）。

use serde_json::json;

use plugin_host::protocol::{decode_line, encode, Frame, FrameError, Method, RpcError};

#[test]
fn request_notification_response_and_error_round_trip() {
    let request = Frame::Request {
        id: 7,
        method: Method::CallTool,
        params: json!({ "toolName": "echo", "input": { "text": "hi" } }),
    };
    assert_eq!(decode_line(&encode(&request)).expect("请求往返"), request);

    let notification = Frame::Notification {
        method: Method::Hello,
        params: json!({ "protocolVersion": 1 }),
    };
    assert_eq!(
        decode_line(&encode(&notification)).expect("通知往返"),
        notification
    );

    let response = Frame::Response {
        id: 7,
        result: json!({ "content": "hi" }),
    };
    assert_eq!(decode_line(&encode(&response)).expect("应答往返"), response);

    let error = Frame::Error {
        id: 7,
        error: RpcError {
            code: -32601,
            message: "method not found".to_string(),
            data: None,
        },
    };
    assert_eq!(decode_line(&encode(&error)).expect("错误往返"), error);
}

#[test]
fn encoded_frames_never_contain_raw_newlines() {
    let frame = Frame::Request {
        id: 1,
        method: Method::CallTool,
        params: json!({ "toolName": "echo", "input": { "text": "line1\nline2" } }),
    };
    let encoded = encode(&frame);
    assert!(!encoded.contains('\n'), "参数里的换行必须被 JSON 转义");
    assert_eq!(decode_line(&encoded).expect("往返"), frame);
}

#[test]
fn rejects_invalid_shapes_instead_of_guessing() {
    assert!(matches!(
        decode_line("{\"id\":1}"),
        Err(FrameError::InvalidShape)
    ));
    assert!(matches!(
        decode_line("{\"id\":1,\"method\":\"host/hello\",\"result\":{}}"),
        Err(FrameError::UnexpectedFields)
    ));
    assert!(matches!(
        decode_line("{\"id\":1,\"method\":\"plugin/nope\",\"params\":{}}"),
        Err(FrameError::UnknownMethod(_))
    ));
    assert!(matches!(
        decode_line("{\"result\":{}}"),
        Err(FrameError::MissingId)
    ));
    assert!(matches!(decode_line("   "), Err(FrameError::EmptyLine)));
    assert!(matches!(
        decode_line("not json"),
        Err(FrameError::InvalidJson(_))
    ));
}

#[test]
fn method_names_are_the_wire_values() {
    for (method, wire) in [
        (Method::Hello, "host/hello"),
        (Method::ListTools, "plugin/listTools"),
        (Method::CallTool, "plugin/callTool"),
        (Method::Health, "plugin/health"),
        (Method::Shutdown, "host/shutdown"),
    ] {
        assert_eq!(method.as_str(), wire);
        assert_eq!(Method::parse(wire), Some(method));
    }
    assert_eq!(Method::parse("plugin/nope"), None);
}
