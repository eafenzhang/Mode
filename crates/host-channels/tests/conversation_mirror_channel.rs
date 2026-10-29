//! `conversation-mirror` 适配器：方法映射与 origin 规则透传。

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use conversation_mirror::{ConversationMirrorService, Message, Record, Snapshot};
use host_channels::conversation_mirror::{GET_IDENTITY, PUBLISH, READ_WORKSPACE, WRITE_LOCAL};
use host_channels::ConversationMirrorChannel;
use rpc_host::{CancellationToken, ServerChannel};
use rpc_wire::Value;

fn unique_root(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "mode-mirror-channel-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("建临时根");
    root
}

fn body(value: &Value) -> serde_json::Value {
    match value {
        Value::Json(text) => serde_json::from_str(text).expect("JSON body"),
        other => panic!("期望 JSON body，实际 {other:?}"),
    }
}

fn snapshot_value(origin: &str) -> serde_json::Value {
    let record = Record {
        session_id: "s1".into(),
        title: "t".into(),
        created_at: 1,
        updated_at: 2,
        message_count: 1,
        truncated: false,
        messages: vec![Message {
            role: "user".into(),
            text: "hi".into(),
            timestamp: Some(2),
        }],
    };
    serde_json::to_value(Snapshot {
        version: 1,
        origin_device_id: origin.into(),
        origin_label: "peer".into(),
        generated_at: 3,
        workspace_path: "D:/w".into(),
        records: vec![record],
    })
    .expect("序列化")
}

fn with_snapshot(method: &str, snapshot: serde_json::Value) -> (String, Value) {
    (
        method.to_string(),
        Value::Array(vec![Value::Json(
            serde_json::json!({ "snapshot": snapshot }).to_string(),
        )]),
    )
}

#[tokio::test]
async fn exposes_identity_and_enforces_origin_rules() {
    let root = unique_root("rules");
    let service = ConversationMirrorService::new(&root);
    let identity = service.get_mirror_identity().expect("身份");
    let channel = ConversationMirrorChannel::new(Arc::clone(&service));

    let identity_body = body(
        &channel
            .call(
                "ctx".into(),
                GET_IDENTITY.into(),
                Value::Array(Vec::new()),
                CancellationToken::default(),
            )
            .await
            .expect("身份可用"),
    );
    assert_eq!(identity_body["deviceId"], identity.device_id);

    let (method, args) = with_snapshot(PUBLISH, snapshot_value("other-device"));
    let stored = body(&channel.call("ctx".into(), method, args, CancellationToken::default()).await.expect("发布"));
    assert_eq!(stored["stored"], true);
    assert_eq!(stored["recordCount"], 1);

    let (method, args) = with_snapshot(WRITE_LOCAL, snapshot_value(&identity.device_id));
    let local = body(&channel.call("ctx".into(), method, args, CancellationToken::default()).await.expect("本机写入"));
    assert_eq!(local["stored"], true);

    let (method, args) = with_snapshot(PUBLISH, snapshot_value(&identity.device_id));
    let echo = body(&channel.call("ctx".into(), method, args, CancellationToken::default()).await.expect("回声"));
    assert_eq!(echo["reason"], "self-origin");
}

#[tokio::test]
async fn reads_workspace_snapshots_through_the_channel() {
    let root = unique_root("read");
    let service = ConversationMirrorService::new(&root);
    let channel = ConversationMirrorChannel::new(Arc::clone(&service));
    let (method, args) = with_snapshot(PUBLISH, snapshot_value("peer-1"));
    channel
        .call("ctx".into(), method, args, CancellationToken::default())
        .await
        .expect("发布");

    let read = body(
        &channel
            .call(
                "ctx".into(),
                READ_WORKSPACE.into(),
                Value::Array(vec![Value::Json(
                    serde_json::json!({ "workspacePath": "D:/w" }).to_string(),
                )]),
                CancellationToken::default(),
            )
            .await
            .expect("读取"),
    );
    assert_eq!(read["snapshots"].as_array().map(Vec::len), Some(1));
    assert_eq!(read["snapshots"][0]["originDeviceId"], "peer-1");
}
