//! 端到端管线：真实 RPC 帧 → FrameRouter → ChannelServer → 回帧。
//!
//! 覆盖「壳内接线」除 Tauri 命令层以外的全部环节（`host_send` 只做一次 decide/dispatch）。

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use host_channels::{MemoryChannel, MEMORY_CHANNEL};
use project_memory::ProjectMemoryService;
use rpc_host::{ChannelServer, FrameRoute, FrameRouter};
use rpc_wire::{
    decode_rpc_frame, encode_rpc_message, encode_scoped_frame, RequestHeader, ResponseHeader,
    RpcFrame, RpcMessage, Value,
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
    let memory_dir = root
        .join("cli/memories/projects/ws-0123456789abcdef/memory");
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
