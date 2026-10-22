//! 审批帧映射：`interaction/requestPermission` 的请求/应答往返与失败语义。
//!
//! 形状出处：`docs/specs/tauri-agent-runtime.md` §3.3「宿主帧映射」（2026-10-22）。
//! 这里只测编解码，不碰会话循环——会话侧的行为在 `agent-core` 的
//! `tests/permission_frame.rs` 里对拍。

use agent_protocol::frame::{Frame, RequestFrame, RequestId, ResponseFrame, ReverseRequestIds};
use agent_protocol::permission::{
    PermissionDecision, PermissionFrameError, PermissionOption, PermissionOptionKind,
    PermissionRequest, PermissionResponse, PermissionRuleValue, PermissionUpdate,
    PermissionUpdateType, RuleBehavior, REQUEST_PERMISSION_METHOD,
};
use serde_json::json;

fn sample_request() -> PermissionRequest {
    PermissionRequest {
        input: json!({ "file_path": "C:\\ws\\a.txt", "content": "x\n" }),
        reason: "Tool has side effects and requires approval".to_string(),
        request_id: "perm_1".to_string(),
        risk_level: "medium".to_string(),
        session_id: "sess_1".to_string(),
        options: vec![
            PermissionOption {
                option_id: "allow_once".to_string(),
                kind: PermissionOptionKind::AllowOnce,
                name: "Allow once".to_string(),
                description: None,
                response: PermissionResponse {
                    decision: PermissionDecision::Allow,
                    reason: Some("Approved once".to_string()),
                    modified_input: None,
                    permission_updates: None,
                    extra: Default::default(),
                },
                extra: Default::default(),
            },
            PermissionOption {
                option_id: "allow_project".to_string(),
                kind: PermissionOptionKind::AllowAlways,
                name: "Always allow in this project".to_string(),
                description: Some("Do not ask again in this project".to_string()),
                response: PermissionResponse {
                    decision: PermissionDecision::Allow,
                    reason: Some("Approved for this project".to_string()),
                    modified_input: None,
                    permission_updates: Some(vec![PermissionUpdate {
                        update_type: PermissionUpdateType::AddRules,
                        behavior: RuleBehavior::Allow,
                        rules: vec![PermissionRuleValue {
                            tool_name: "Write".to_string(),
                            rule_content: "C:\\ws\\a.txt".to_string(),
                            extra: Default::default(),
                        }],
                        extra: Default::default(),
                    }]),
                    extra: Default::default(),
                },
                extra: Default::default(),
            },
            PermissionOption {
                option_id: "deny".to_string(),
                kind: PermissionOptionKind::Deny,
                name: "Deny".to_string(),
                description: None,
                response: PermissionResponse {
                    decision: PermissionDecision::Deny,
                    reason: Some("denied".to_string()),
                    modified_input: None,
                    permission_updates: None,
                    extra: Default::default(),
                },
                extra: Default::default(),
            },
        ],
        tool_call_id: "toolu_1".to_string(),
        tool_name: "Write".to_string(),
        turn_id: "turn_1".to_string(),
        extra: Default::default(),
    }
}

#[test]
fn request_frame_round_trips_params_and_id() {
    let request = sample_request();
    let id = RequestId::String("server-1".to_string());
    let frame = request
        .to_request_frame(id.clone())
        .expect("应能构造请求帧");
    let Frame::Request(wire) = &frame else {
        panic!("应是请求帧");
    };
    assert_eq!(wire.id, id);
    assert_eq!(wire.method, REQUEST_PERMISSION_METHOD);
    // 线上是 camelCase：与 spec §3.3 的逐字形状一致。
    let params = wire.params.as_ref().expect("应有 params");
    assert_eq!(params["riskLevel"], json!("medium"));
    assert_eq!(params["toolName"], json!("Write"));
    assert_eq!(params["options"].as_array().map(Vec::len), Some(3));

    let decoded = PermissionRequest::from_request_frame(wire).expect("应能解码");
    assert_eq!(decoded, request);
}

#[test]
fn response_frame_round_trips_minimal_reply() {
    // 观察到的 host 应答可以只带 decision（driver 的默认应答就是 `{decision:"allow"}`）。
    let response: PermissionResponse =
        serde_json::from_value(json!({ "decision": "allow" })).expect("最小应答应可解码");
    let id = RequestId::String("server-2".to_string());
    let frame = response
        .to_response_frame(id.clone())
        .expect("应能构造应答帧");
    let Frame::Response(wire) = &frame else {
        panic!("应是应答帧");
    };
    assert_eq!(wire.id, id);
    assert_eq!(wire.result, json!({ "decision": "allow" }));
    assert_eq!(
        PermissionResponse::from_response_frame(wire).expect("应能解码"),
        response
    );
}

#[test]
fn wrong_method_is_rejected() {
    let frame = RequestFrame {
        id: RequestId::String("server-3".to_string()),
        method: "interaction/requestUserInput".to_string(),
        params: Some(json!({})),
        trace: None,
    };
    match PermissionRequest::from_request_frame(&frame) {
        Err(PermissionFrameError::UnexpectedMethod(method)) => {
            assert_eq!(method, "interaction/requestUserInput");
        }
        other => panic!("应是 UnexpectedMethod，实际 {other:?}"),
    }
}

#[test]
fn missing_and_invalid_params_are_rejected() {
    let missing = RequestFrame {
        id: RequestId::String("server-4".to_string()),
        method: REQUEST_PERMISSION_METHOD.to_string(),
        params: None,
        trace: None,
    };
    assert!(matches!(
        PermissionRequest::from_request_frame(&missing),
        Err(PermissionFrameError::MissingParams)
    ));

    let invalid = RequestFrame {
        id: RequestId::String("server-5".to_string()),
        method: REQUEST_PERMISSION_METHOD.to_string(),
        params: Some(json!({ "toolName": "Write" })),
        trace: None,
    };
    assert!(matches!(
        PermissionRequest::from_request_frame(&invalid),
        Err(PermissionFrameError::InvalidParams(_))
    ));
}

#[test]
fn invalid_result_is_rejected() {
    let frame = ResponseFrame {
        id: RequestId::String("server-6".to_string()),
        result: json!({ "decision": "maybe" }),
    };
    assert!(matches!(
        PermissionResponse::from_response_frame(&frame),
        Err(PermissionFrameError::InvalidResponse(_))
    ));
}

#[test]
fn reverse_request_ids_are_connection_scoped() {
    let ids = ReverseRequestIds::default();
    assert_eq!(ids.next(), RequestId::String("server-1".to_string()));
    assert_eq!(ids.next(), RequestId::String("server-2".to_string()));
    // 分配器按连接共享：放进 Arc 给多个会话后仍取同一个序列。
    let shared = std::sync::Arc::new(ids);
    let first = shared.next();
    let second = shared.next();
    assert_eq!(first, RequestId::String("server-3".to_string()));
    assert_eq!(second, RequestId::String("server-4".to_string()));
}
