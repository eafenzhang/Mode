//! 端到端管线：真实 RPC 帧 → FrameRouter → ChannelServer → 回帧。
//!
//! 覆盖「壳内接线」除 Tauri 命令层以外的全部环节（`host_send` 只做一次 decide/dispatch）。

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use host_channels::{MEMORY_CHANNEL, MemoryChannel};
use project_memory::ProjectMemoryService;
use rpc_host::{ChannelServer, FrameRoute, FrameRouter};
use rpc_wire::{
    RequestHeader, ResponseHeader, RpcFrame, RpcMessage, Value, decode_rpc_frame,
    encode_rpc_message, encode_scoped_frame,
};

fn unique_root(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "mode-pipeline-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("建临时根");
    root
}

fn list_frame(id: i32) -> Vec<u8> {
    encode_rpc_message(&RpcMessage {
        header: RequestHeader::Promise {
            id,
            channel: MEMORY_CHANNEL.into(),
            method: "listProjectMemories".into(),
        },
        body: Value::Array(Vec::new()),
    })
}

#[tokio::test]
async fn base_request_reaches_memory_and_returns_a_success_frame() {
    let root = unique_root("roundtrip");
    let memory_dir = root.join("cli/memories/projects/ws-0123456789abcdef/memory");
    fs::create_dir_all(&memory_dir).expect("建 memory 目录");
    fs::write(memory_dir.join("MEMORY.md"), "# index").expect("写文件");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut server = ChannelServer::new(tx, "mode-host-sidecar", Duration::from_millis(200), true);
    server.register_channel(
        MEMORY_CHANNEL,
        Arc::new(MemoryChannel::new(Arc::new(ProjectMemoryService::new(
            root.clone(),
        )))),
    );
    let router = FrameRouter::base_scope_only([MEMORY_CHANNEL]);

    let payload = match router.route(&list_frame(7)) {
        FrameRoute::Local {
            payload, scope_id, ..
        } => {
            assert_eq!(scope_id, None);
            payload
        }
        other => panic!("期望 Local，实际 {other:?}"),
    };
    server.handle_frame(&payload);

    let response = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("等回帧超时")
        .expect("出站通道关闭");
    match decode_rpc_frame(&response).expect("回帧可解码") {
        RpcFrame::Response(response) => {
            assert_eq!(response.header, ResponseHeader::PromiseSuccess { id: 7 });
            let Value::Json(text) = &response.body else {
                panic!("body 应为 JSON：{:?}", response.body);
            };
            let payload: serde_json::Value = serde_json::from_str(text).expect("合法 JSON");
            assert_eq!(payload[0]["id"], "ws-0123456789abcdef");
            assert_eq!(payload[0]["files"][0]["name"], "MEMORY.md");
        }
        RpcFrame::Request(_) => panic!("服务端不应发请求帧"),
    }
}

#[tokio::test]
async fn scoped_requests_stay_with_the_node_host() {
    let router = FrameRouter::base_scope_only([MEMORY_CHANNEL]);
    let scoped = encode_scoped_frame(9, &list_frame(8)).expect("编码 scoped 帧");
    assert_eq!(router.route(&scoped), FrameRoute::Legacy);
}

#[tokio::test]
async fn local_scope_serves_scoped_requests_and_wraps_replies() {
    // W2 scoped attachment 对拍（spec docs/specs/w2-transport-cutover.md §4）：
    // scope-ready 登记后，同一 scope 上的请求由本地 ChannelServer 应答，回帧
    // 经 registry.wrap_response 加 `0xFF+scopeId` 前缀；detach 后登记表不再包装
    // （未挂接 scope 的回落转发门禁在壳侧 decide_route，见 inprocess.rs）。
    use rpc_host::LocalScopeRegistry;
    use rpc_wire::{DecodedFrame, decode_scope_frame};

    let root = unique_root("scoped");
    let memory_dir = root.join("cli/memories/projects/ws-0123456789abcdef/memory");
    fs::create_dir_all(&memory_dir).expect("建 memory 目录");
    fs::write(memory_dir.join("MEMORY.md"), "# index").expect("写文件");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut server = ChannelServer::new(tx, "mode-host-sidecar", Duration::from_millis(200), true);
    server.register_channel(
        MEMORY_CHANNEL,
        Arc::new(MemoryChannel::new(Arc::new(ProjectMemoryService::new(
            root.clone(),
        )))),
    );
    let router = FrameRouter::new([MEMORY_CHANNEL]);
    let mut scopes = LocalScopeRegistry::new();

    // 模拟页面桥：attach 发起后，Node Host 下发 scope-ready 控制帧 → 壳登记。
    scopes.attach(9);

    // scoped 请求 → 本地应答。
    let scoped = encode_scoped_frame(9, &list_frame(21)).expect("编码 scoped 帧");
    let payload = match router.route(&scoped) {
        FrameRoute::Local {
            payload, scope_id, ..
        } => {
            assert_eq!(scope_id, Some(9));
            payload
        }
        other => panic!("期望 Local，实际 {other:?}"),
    };
    server.handle_frame(&payload);
    let response = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("等回帧超时")
        .expect("出站通道关闭");

    // 回帧按 scope 包装：0xFF+scopeId 前缀，内层仍是该请求的应答。
    let wrapped = scopes.wrap_response(9, &response).expect("scope 已登记");
    match decode_scope_frame(&wrapped).expect("包装帧可解码") {
        DecodedFrame::Scoped { scope_id, payload } => {
            assert_eq!(scope_id, 9);
            match decode_rpc_frame(payload).expect("内层可解码") {
                RpcFrame::Response(response) => {
                    assert_eq!(response.header, ResponseHeader::PromiseSuccess { id: 21 });
                }
                RpcFrame::Request(_) => panic!("服务端不应发请求帧"),
            }
        }
        other => panic!("期望 Scoped 帧，实际 {other:?}"),
    }

    // detach 后登记表不再包装：回落转发的门禁由壳侧 decide_route 施加
    // （路由层只按频道名分流，不感知挂接状态）。
    scopes.detach(9);
    assert!(
        scopes.wrap_response(9, &response).is_none(),
        "detach 后同 scope 的回帧不应再由壳内包装"
    );
}

#[tokio::test]
async fn unknown_methods_return_an_error_frame() {
    let root = unique_root("unknown");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut server = ChannelServer::new(tx, "mode-host-sidecar", Duration::from_millis(200), true);
    server.register_channel(
        MEMORY_CHANNEL,
        Arc::new(MemoryChannel::new(Arc::new(ProjectMemoryService::new(
            root,
        )))),
    );

    let frame = encode_rpc_message(&RpcMessage {
        header: RequestHeader::Promise {
            id: 11,
            channel: MEMORY_CHANNEL.into(),
            method: "nope".into(),
        },
        body: Value::Undefined,
    });
    server.handle_frame(&frame);

    let response = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("等回帧超时")
        .expect("出站通道关闭");
    match decode_rpc_frame(&response).expect("回帧可解码") {
        RpcFrame::Response(response) => {
            assert_eq!(response.header, ResponseHeader::PromiseError { id: 11 });
        }
        RpcFrame::Request(_) => panic!("服务端不应发请求帧"),
    }
}
