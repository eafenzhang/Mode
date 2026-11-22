//! `lan-access` 频道首切行为契约（LT-16.4a：对端配对 registry）。
//!
//! 真实临时凭据库（credential-store，文件锁）→ 频道调用 → registry 语义断言。

use std::path::PathBuf;
use std::sync::Arc;

use credential_store::CredentialStore;
use rpc_host::{CancellationToken, ServerChannel as _};
use rpc_wire::Value;
use serde_json::{json, Value as Json};

use host_channels::lan_access::{
    LanAccessChannel, LanAccessHostPort, LAN_ACCESS_CHANNEL, ON_DID_CHANGE,
};

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
        "setEnabled",
        "createPairCode",
        "removeClient",
        "resetTokens",
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

#[tokio::test]
async fn get_state_reads_clients_from_credentials() {
    let dir = TempDir::new("state");
    let credentials = store(&dir);
    credentials
        .save(
            "lan:access:clients",
            &json!([
                {
                    "id": "c1",
                    "label": "手机",
                    "createdAt": 10,
                    "lastUsedAt": 20,
                    "lastWorkspacePath": "D:/work/app",
                    "remoteDesktopAuthorized": true
                },
                { "id": "", "label": "bad" },
                { "label": "missing-id" },
                { "id": "c2" }
            ])
            .to_string(),
        )
        .expect("clients");
    let channel = LanAccessChannel::new(Arc::clone(&credentials));

    let state = call(&channel, "getState", json!({})).await;
    assert_eq!(state["enabled"], false);
    assert!(state["port"].is_null());
    assert_eq!(state["addresses"], json!([]));
    assert!(state["pairCode"].is_null());
    assert_eq!(state["activeWebSessions"], 0);
    assert!(state["publicRelayUrl"].is_null());
    let clients = state["clients"].as_array().expect("clients");
    assert_eq!(clients.len(), 2, "{clients:?}");
    assert_eq!(clients[0]["id"], "c1");
    assert_eq!(clients[0]["label"], "手机");
    assert_eq!(clients[0]["lastUsedAt"], 20);
    assert_eq!(clients[0]["lastWorkspacePath"], "D:/work/app");
    assert_eq!(clients[0]["remoteDesktopAuthorized"], true);
    // 缺字段按 TS 默认：label ""、createdAt now、lastUsedAt null；
    // 授权只认显式 true。
    assert_eq!(clients[1]["id"], "c2");
    assert_eq!(clients[1]["label"], "");
    assert!(clients[1]["createdAt"].as_i64().expect("createdAt") > 0);
    assert!(clients[1]["lastUsedAt"].is_null());
    assert!(clients[1].get("lastWorkspacePath").is_none());
    assert!(clients[1].get("remoteDesktopAuthorized").is_none());

    // 坏 JSON / 非数组 = 空 clients。
    credentials
        .save("lan:access:clients", "not-json")
        .expect("bad json");
    let state = call(&channel, "getState", json!({})).await;
    assert!(state["clients"].as_array().expect("clients").is_empty());
    credentials
        .save("lan:access:clients", "{}")
        .expect("non-array");
    let state = call(&channel, "getState", json!({})).await;
    assert!(state["clients"].as_array().expect("clients").is_empty());
}

/// supervisor 端口测试替身：状态可变、记录调用、广播 onDidChange。
struct FakeHost {
    state: std::sync::Mutex<Json>,
    listeners:
        Arc<std::sync::Mutex<std::collections::HashMap<String, rpc_host::EventCallback>>>,
    calls: std::sync::Mutex<Vec<String>>,
}

impl FakeHost {
    fn new() -> Self {
        Self {
            state: std::sync::Mutex::new(json!({
                "enabled": true,
                "port": 45880,
                "addresses": ["10.0.0.5"],
                "pairCode": null,
                "clients": [],
                "activeWebSessions": 0,
                "publicRelayUrl": null,
            })),
            listeners: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn record(&self, call: impl Into<String>) {
        self.calls.lock().expect("calls").push(call.into());
    }

    fn notify(&self) {
        let text = self.state.lock().expect("state").to_string();
        let listeners: Vec<rpc_host::EventCallback> = self
            .listeners
            .lock()
            .expect("listeners")
            .values()
            .cloned()
            .collect();
        for emit in listeners {
            emit(Value::Json(text.clone()));
        }
    }

    fn snapshot(&self) -> Json {
        self.state.lock().expect("state").clone()
    }
}

impl LanAccessHostPort for FakeHost {
    fn state(&self) -> Result<Json, String> {
        self.record("getState");
        Ok(self.snapshot())
    }

    fn set_enabled(&self, enabled: bool) -> Result<Json, String> {
        self.record(format!("setEnabled:{enabled}"));
        self.state.lock().expect("state")["enabled"] = json!(enabled);
        self.notify();
        Ok(self.snapshot())
    }

    fn create_pair_code(&self) -> Result<Json, String> {
        self.record("createPairCode");
        self.state.lock().expect("state")["pairCode"] =
            json!({ "code": "ABCDEF", "expiresAt": 2_000_000 });
        Ok(self.snapshot())
    }

    fn remove_client(&self, client_id: &str) -> Result<Json, String> {
        self.record(format!("removeClient:{client_id}"));
        Ok(self.snapshot())
    }

    fn reset_tokens(&self) -> Result<Json, String> {
        self.record("resetTokens");
        Ok(self.snapshot())
    }

    fn subscribe_changes(&self, emit: rpc_host::EventCallback) -> rpc_host::Subscription {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed).to_string();
        self.listeners
            .lock()
            .expect("listeners")
            .insert(id.clone(), Arc::clone(&emit));
        // 首个订阅者补当前快照（TS 幂等事实语义）。
        let text = self.snapshot().to_string();
        emit(Value::Json(text));
        let listeners = Arc::clone(&self.listeners);
        Box::new(move || {
            listeners.lock().expect("listeners").remove(&id);
        })
    }
}

#[tokio::test]
async fn host_port_serves_management_state_and_events() {
    let dir = TempDir::new("host");
    let host = Arc::new(FakeHost::new());
    let channel = LanAccessChannel::with_host(store(&dir), host.clone());

    let state = call(&channel, "getState", json!({})).await;
    assert_eq!(state["enabled"], true);
    assert_eq!(state["port"], 45880);
    assert_eq!(state["addresses"], json!(["10.0.0.5"]));

    // onDidChange：订阅即补当前快照；后续动作广播。
    let frames = Arc::new(std::sync::Mutex::new(Vec::<Json>::new()));
    let sink = Arc::clone(&frames);
    let _sub = channel
        .listen(
            "ctx".into(),
            ON_DID_CHANGE.into(),
            Value::Undefined,
            Arc::new(move |value| {
                if let Value::Json(text) = value {
                    sink.lock()
                        .expect("frames")
                        .push(serde_json::from_str(&text).expect("state JSON"));
                }
            }),
        )
        .expect("listen onDidChange");
    assert_eq!(frames.lock().expect("frames").len(), 1);

    let state = call(&channel, "setEnabled", json!({ "enabled": false })).await;
    assert_eq!(state["enabled"], false);
    {
        let frames = frames.lock().expect("frames");
        assert_eq!(frames.len(), 2, "{frames:?}");
        assert_eq!(frames[1]["enabled"], false);
    }

    let state = call(&channel, "createPairCode", json!({})).await;
    assert_eq!(state["pairCode"]["code"], "ABCDEF");
    call(&channel, "removeClient", json!({ "clientId": "c1" })).await;
    call(&channel, "resetTokens", json!({})).await;

    let calls = host.calls.lock().expect("calls");
    assert!(calls.contains(&"setEnabled:false".to_string()), "{calls:?}");
    assert!(calls.contains(&"createPairCode".to_string()), "{calls:?}");
    assert!(calls.contains(&"removeClient:c1".to_string()), "{calls:?}");
    assert!(calls.contains(&"resetTokens".to_string()), "{calls:?}");
}

#[tokio::test]
async fn discover_peers_returns_announcement_shaped_array() {
    let dir = TempDir::new("discover");
    let channel = LanAccessChannel::new(store(&dir));
    // 纯 UDP 探测：无对端时返回空数组，有对端时每行是 announcement 形状。
    let peers = call(&channel, "discoverPeers", json!({})).await;
    let peers = peers.as_array().expect("peers array");
    if let Some(peer) = peers.first() {
        assert_eq!(peer["magic"], "mode-lan");
        assert_eq!(peer["protocolVersion"], 1);
        assert!(peer["serverId"].is_string());
        assert!(peer["host"].is_string());
        assert!(peer["port"].as_u64().unwrap_or(0) > 0);
        assert!(peer["requiresPairing"].is_boolean());
    }
}

#[test]
fn channel_name_constant() {
    assert_eq!(LAN_ACCESS_CHANNEL, "lan-access");
}
