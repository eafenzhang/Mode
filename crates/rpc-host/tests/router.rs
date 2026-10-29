//! 帧路由的归属判定：base/scoped、控制帧、本地频道与遗留转发。

use rpc_host::{FrameRouter, FrameRoute};
use rpc_wire::{
    encode_control_frame, encode_rpc_message, encode_scoped_frame, serialize, AttachmentControl,
    RequestHeader, RpcMessage, Value,
};

fn promise(id: i32, channel: &str, method: &str) -> Vec<u8> {
    encode_rpc_message(&RpcMessage {
        header: RequestHeader::Promise {
            id,
            channel: channel.into(),
            method: method.into(),
        },
        body: Value::Undefined,
    })
}

#[test]
fn owned_base_request_routes_local() {
    let router = FrameRouter::new(["setting", "file"]);
    let frame = promise(1, "setting", "get");
    match router.route(&frame) {
        FrameRoute::Local { scope_id, request, payload } => {
            assert!(!payload.is_empty(), "Local 必须带原始 payload");
            assert_eq!(scope_id, None);
            assert_eq!(request.header.channel(), Some("setting"));
        }
        other => panic!("期望 Local，实际 {other:?}"),
    }
}

#[test]
fn owned_scoped_request_keeps_scope_id() {
    let router = FrameRouter::new(["setting"]);
    let frame = encode_scoped_frame(7, &promise(2, "setting", "get")).unwrap();
    match router.route(&frame) {
        FrameRoute::Local { scope_id, request, payload } => {
            assert!(!payload.is_empty(), "Local 必须带原始 payload");
            assert_eq!(scope_id, Some(7));
            assert_eq!(request.header.channel(), Some("setting"));
        }
        other => panic!("期望 Local，实际 {other:?}"),
    }
}

#[test]
fn unowned_requests_go_legacy_with_or_without_scope() {
    let router = FrameRouter::new(["setting"]);
    assert_eq!(
        router.route(&promise(3, "mode-task", "create")),
        FrameRoute::Legacy
    );
    let scoped = encode_scoped_frame(9, &promise(4, "mode-task", "create")).unwrap();
    assert_eq!(router.route(&scoped), FrameRoute::Legacy);
}

#[test]
fn control_frames_are_split_out() {
    let router = FrameRouter::new(["setting"]);
    let ready = encode_control_frame(&AttachmentControl::ScopeReady(3)).unwrap();
    assert_eq!(
        router.route(&ready),
        FrameRoute::Control(AttachmentControl::ScopeReady(3))
    );
    let detach = encode_control_frame(&AttachmentControl::ScopeDetach(4)).unwrap();
    assert_eq!(
        router.route(&detach),
        FrameRoute::Control(AttachmentControl::ScopeDetach(4))
    );
}

#[test]
fn rejects_malformed_and_non_request_frames() {
    let router = FrameRouter::new(["setting"]);
    // scoped marker 但不足 5 字节
    assert_eq!(router.route(&[0xff, 0, 0]), FrameRoute::Rejected);
    // 空帧
    assert_eq!(router.route(&[]), FrameRoute::Rejected);
    // 垃圾字节
    assert_eq!(router.route(&[0xff, 0xff]), FrameRoute::Rejected);
    // 回帧（200 Initialize）不是页面来的请求
    let mut response_header = serialize(&Value::Array(vec![Value::Int(200)]));
    response_header.extend(serialize(&Value::Undefined));
    assert_eq!(router.route(&response_header), FrameRoute::Rejected);
}

#[test]
fn cancel_and_dispose_are_forwarded_even_when_channel_is_owned() {
    let router = FrameRouter::new(["setting"]);
    for header in [
        RequestHeader::PromiseCancel { id: 5 },
        RequestHeader::EventDispose { id: 6 },
    ] {
        let frame = encode_rpc_message(&RpcMessage {
            header,
            body: Value::Undefined,
        });
        assert_eq!(router.route(&frame), FrameRoute::Legacy);
    }
}

#[test]
fn ownership_is_fixed_at_construction() {
    let router = FrameRouter::new(["setting"]);
    assert!(router.owns("setting"));
    assert!(!router.owns("file"));
    assert_eq!(router.owned_channels().count(), 1);
}

#[test]
fn base_scope_only_router_forwards_scoped_owned_requests() {
    let inner = promise(8, "setting", "get");
    let scoped = encode_scoped_frame(3, &inner).unwrap();
    assert!(
        matches!(
            FrameRouter::new(["setting"]).route(&scoped),
            FrameRoute::Local { scope_id: Some(3), .. }
        ),
        "默认 router 允许接管 scoped 帧"
    );
    assert_eq!(
        FrameRouter::base_scope_only(["setting"]).route(&scoped),
        FrameRoute::Legacy,
        "只服务 base scope 的 router 必须把 scoped 帧留给 Node Host"
    );
}
