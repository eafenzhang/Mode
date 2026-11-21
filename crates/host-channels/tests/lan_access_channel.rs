//! `lan-access` 频道首切行为契约（LT-16.4a：对端配对 registry）。
//!
//! 真实临时凭据库（credential-store，文件锁）→ 频道调用 → registry 语义断言。

use std::path::PathBuf;
use std::sync::Arc;

use credential_store::CredentialStore;
use rpc_host::{CancellationToken, ServerChannel as _};
use rpc_wire::Value;
use serde_json::{json, Value as Json};

use host_channels::lan_access::{LanAccessChannel, LAN_ACCESS_CHANNEL};

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "lan-access-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn store(dir: &TempDir) -> Arc<CredentialStore> {
    Arc::new(CredentialStore::with_secret(&dir.0, "lan-access-test-secret"))
}

async fn call(channel: &LanAccessChannel, method: &str, arg: Json) -> Json {
    let value = channel
        .call(
            "ctx".into(),
            method.into(),
            Value::Json(arg.to_string()),
            CancellationToken::default(),
        )
        .await
        .expect("call");
    match value {
        Value::Json(text) => serde_json::from_str(&text).expect("JSON body"),
        other => panic!("期望 JSON body，实际 {other:?}"),
    }
}

async fn call_void(channel: &LanAccessChannel, method: &str, arg: Json) {
    let value = channel
        .call(
            "ctx".into(),
            method.into(),
            Value::Json(arg.to_string()),
            CancellationToken::default(),
        )
        .await
        .expect("call");
    assert!(matches!(value, Value::Undefined), "{method}: {value:?}");
}

fn seed(store: &CredentialStore, server_id: &str, meta: Option<Json>) {
    store
        .save(&format!("lan:peer:{server_id}:token"), "peer-token")
        .expect("token");
    if let Some(meta) = meta {
        store
            .save(&format!("lan:peer:{server_id}:meta"), &meta.to_string())
            .expect("meta");
    }
}

#[tokio::test]
async fn list_paired_peers_projects_meta_and_migrates_v1() {
    let dir = TempDir::new("list");
    let credentials = store(&dir);
    seed(
        &credentials,
        "peer-a",
        Some(json!({
            "endpoints": [
                { "scheme": "http", "host": "10.0.0.5", "port": 45880, "kind": "lan" },
                { "scheme": "https", "host": "rd.example.com", "port": 443, "kind": "public", "path": "/d/t1" }
            ],
            "name": "书房",
            "pairedAt": 2000
        })),
    );
    seed(
        &credentials,
        "peer-b",
        Some(json!({ "host": "192.168.1.9", "port": 45880, "pairedAt": 1000 })),
    );
    seed(&credentials, "peer-c", None);
    let channel = LanAccessChannel::new(credentials);

    let peers = call(&channel, "listPairedPeers", json!({})).await;
    let peers = peers.as_array().expect("peers");
    assert_eq!(peers.len(), 3, "{peers:?}");
    // pairedAt 降序；缺 meta 的旧配对保留一行（可按 serverId 删除）。
    assert_eq!(peers[0]["serverId"], "peer-a");
    assert_eq!(peers[0]["name"], "书房");
    assert_eq!(peers[0]["pairedAt"], 2000);
    // 公网优先：primary 投影为主端点。
    assert_eq!(peers[0]["host"], "rd.example.com");
    assert_eq!(peers[0]["port"], 443);
    assert_eq!(peers[0]["endpoints"][0]["kind"], "public");
    assert_eq!(peers[0]["endpoints"][1]["kind"], "lan");
    // v1 元数据读取时迁移为 LAN http 端点。
    assert_eq!(peers[1]["serverId"], "peer-b");
    assert_eq!(peers[1]["host"], "192.168.1.9");
    assert_eq!(peers[1]["endpoints"][0]["scheme"], "http");
    assert_eq!(peers[1]["endpoints"][0]["kind"], "lan");
    // 无 meta：地址未知但保留。
    assert_eq!(peers[2]["serverId"], "peer-c");
    assert_eq!(peers[2]["host"], "");
    assert_eq!(peers[2]["port"], 0);
    assert!(peers[2].get("endpoints").is_none());
}

#[tokio::test]
async fn list_drops_insecure_public_endpoints() {
    let dir = TempDir::new("insecure");
    let credentials = store(&dir);
    seed(
        &credentials,
        "peer-a",
        Some(json!({
            "endpoints": [
                { "scheme": "http", "host": "relay.example.com", "port": 80, "kind": "public" },
                { "scheme": "http", "host": "10.0.0.5", "port": 45880, "kind": "lan" }
            ]
        })),
    );
    let channel = LanAccessChannel::new(credentials);
    let peers = call(&channel, "listPairedPeers", json!({})).await;
    let endpoints = peers[0]["endpoints"].as_array().expect("endpoints");
    assert_eq!(endpoints.len(), 1, "{endpoints:?}");
    assert_eq!(endpoints[0]["kind"], "lan");
}

#[tokio::test]
async fn rename_updates_meta_name_only() {
    let dir = TempDir::new("rename");
    let credentials = store(&dir);
    seed(
        &credentials,
        "peer-a",
        Some(json!({
            "host": "10.0.0.5",
            "port": 45880,
            "pairedAt": 2000
        })),
    );
    seed(&credentials, "peer-c", None);
    let channel = LanAccessChannel::new(Arc::clone(&credentials));

    call_void(
        &channel,
        "renamePairedPeer",
        json!({ "serverId": "peer-a", "name": "  新名字  " }),
    )
    .await;
    let peers = call(&channel, "listPairedPeers", json!({})).await;
    assert_eq!(peers[0]["name"], "新名字");
    // 地址/时间不动（读取时迁移后的 v2 形态落盘，重命名不会丢端点）。
    assert_eq!(peers[0]["host"], "10.0.0.5");
    assert_eq!(peers[0]["pairedAt"], 2000);

    for bad in [json!(""), json!("   "), json!("x".repeat(65))] {
        let error = channel
            .call(
                "ctx".into(),
                "renamePairedPeer".into(),
                Value::Json(json!({ "serverId": "peer-a", "name": bad }).to_string()),
                CancellationToken::default(),
            )
            .await
            .expect_err("invalid name");
        match error {
            rpc_host::ChannelError::Error { message, .. } => {
                assert!(message.contains("名称长度必须为 1–64 个字符"), "{message}");
            }
            other => panic!("期望 Error 变体，实际 {other:?}"),
        }
    }

    let error = channel
        .call(
            "ctx".into(),
            "renamePairedPeer".into(),
            Value::Json(json!({ "serverId": "peer-c", "name": "名称" }).to_string()),
            CancellationToken::default(),
        )
        .await
        .expect_err("missing meta");
    match error {
        rpc_host::ChannelError::Error { message, .. } => {
            assert!(message.contains("该配对缺少元数据，无法重命名"), "{message}");
        }
        other => panic!("期望 Error 变体，实际 {other:?}"),
    }
}

#[tokio::test]
async fn remove_deletes_token_and_meta_idempotently() {
    let dir = TempDir::new("remove");
    let credentials = store(&dir);
    seed(
        &credentials,
        "peer-a",
        Some(json!({ "host": "10.0.0.5", "port": 45880 })),
    );
    let channel = LanAccessChannel::new(Arc::clone(&credentials));

    call_void(&channel, "removePairedPeer", json!({ "serverId": "peer-a" })).await;
    assert!(credentials
        .load("lan:peer:peer-a:token")
        .expect("load")
        .is_none());
    assert!(credentials
        .load("lan:peer:peer-a:meta")
        .expect("load")
        .is_none());
    // 幂等：缺任意一个都不影响删除。
    call_void(&channel, "removePairedPeer", json!({ "serverId": "peer-a" })).await;
    assert!(call(&channel, "listPairedPeers", json!({}))
        .await
        .as_array()
        .expect("peers")
        .is_empty());

    let error = channel
        .call(
            "ctx".into(),
            "removePairedPeer".into(),
            Value::Json(json!({ "serverId": "   " }).to_string()),
            CancellationToken::default(),
        )
        .await
        .expect_err("empty serverId");
    match error {
        rpc_host::ChannelError::Error { message, .. } => {
            assert!(message.contains("缺少 serverId"), "{message}");
        }
        other => panic!("期望 Error 变体，实际 {other:?}"),
    }
}

#[tokio::test]
async fn server_supervisor_methods_report_missing_owner() {
    let dir = TempDir::new("supervisor");
    let channel = LanAccessChannel::new(store(&dir));

    for method in [
        "getState",
        "setEnabled",
        "createPairCode",
        "removeClient",
        "resetTokens",
        "discoverPeers",
        "pairPeer",
        "getPeerWorkspaces",
    ] {
        let error = channel
            .call(
                "ctx".into(),
                method.into(),
                Value::Json("{}".to_string()),
                CancellationToken::default(),
            )
            .await
            .expect_err("server supervisor missing");
        match error {
            rpc_host::ChannelError::Error { message, .. } => {
                assert!(message.contains("LAN server supervisor"), "{method}: {message}");
            }
            other => panic!("期望 Error 变体，实际 {other:?}"),
        }
    }

    let error = match channel.listen(
        "ctx".into(),
        "onDidChange".into(),
        Value::Undefined,
        Arc::new(|_| {}),
    ) {
        Ok(_) => panic!("onDidChange 应报错"),
        Err(error) => error,
    };
    match error {
        rpc_host::ChannelError::Error { message, .. } => {
            assert!(message.contains("LAN server supervisor"), "{message}");
        }
        other => panic!("期望 Error 变体，实际 {other:?}"),
    }
}

#[test]
fn channel_name_constant() {
    assert_eq!(LAN_ACCESS_CHANNEL, "lan-access");
}
