//! `remote-desktop` 频道契约（LT-16.4b 首切）。
//!
//! 真源 `packages/services/src/remote-desktop/remoteDesktopService.ts` 与
//! `packages/desktop/src/host/remoteDesktopServer.ts`。覆盖：设置面读写往返、
//! 逐设备授权与撤销写同一记录字段、编码能力白名单降级、对端探测 offline 不抛、
//! 会话面未装配端口时显式报错。

use std::path::PathBuf;
use std::sync::Arc;

use credential_store::CredentialStore;
use host_channels::remote_desktop::{RemoteDesktopChannel, REMOTE_DESKTOP_CHANNEL};
use rpc_host::{CancellationToken, ServerChannel};
use rpc_wire::Value;
use serde_json::{json, Value as Json};

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "rd-channel-it-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        TempDir(dir)
    }

    fn config_dir(&self) -> PathBuf {
        self.0.join("v2")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn channel(dir: &TempDir) -> RemoteDesktopChannel {
    let credentials = Arc::new(CredentialStore::with_secret(&dir.0, "rd-channel-it-secret"));
    RemoteDesktopChannel::new(dir.0.clone(), credentials)
}

fn args(value: Json) -> Value {
    Value::Json(value.to_string())
}

async fn call(channel: &RemoteDesktopChannel, method: &str, args_json: Json) -> Json {
    let value = channel
        .call(
            "ctx".into(),
            method.into(),
            args(args_json),
            CancellationToken::default(),
        )
        .await
        .expect("call");
    match value {
        Value::Json(text) => serde_json::from_str(&text).expect("json body"),
        other => panic!("expected JSON body for {method}, got {other:?}"),
    }
}

async fn call_err(channel: &RemoteDesktopChannel, method: &str, args_json: Json) -> String {
    let error = channel
        .call(
            "ctx".into(),
            method.into(),
            args(args_json),
            CancellationToken::default(),
        )
        .await
        .expect_err("expected error");
    format!("{error:?}")
}

/// 直接用凭据写入 LAN 客户端记录（LAN server 是唯一写者；这里模拟它的结果）。
fn seed_clients(dir: &TempDir, records: &str) {
    let credentials = CredentialStore::with_secret(&dir.0, "rd-channel-it-secret");
    credentials
        .save("lan:access:clients", records)
        .expect("seed clients");
}

#[tokio::test]
async fn channel_name_matches_the_ts_service_channel() {
    assert_eq!(REMOTE_DESKTOP_CHANNEL, "remote-desktop");
}

#[tokio::test]
async fn get_server_state_defaults_to_disabled_without_touching_credentials() {
    let dir = TempDir::new("state");
    let channel = channel(&dir);
    let state = call(&channel, "getServerState", Json::Array(vec![])).await;
    assert_eq!(state["enabled"], false);
    assert_eq!(state["authorizedClients"], json!([]));
    // 无编码能力缓存 = unknown（不猜）。
    assert_eq!(state["encoderCapability"], "unknown");
}

#[tokio::test]
async fn set_server_enabled_round_trips_and_disable_deletes_the_key() {
    let dir = TempDir::new("toggle");
    let channel = channel(&dir);

    let state = call(&channel, "setServerEnabled", json!({ "enabled": true })).await;
    assert_eq!(state["enabled"], true);
    assert_eq!(
        call(&channel, "getServerState", Json::Array(vec![])).await["enabled"],
        true
    );

    let state = call(&channel, "setServerEnabled", json!({ "enabled": false })).await;
    assert_eq!(state["enabled"], false);

    // 关 = 删键：凭据里不应残留 "0"（否则「从未开启」与「开过又关」无法区分）。
    let credentials = CredentialStore::with_secret(&dir.0, "rd-channel-it-secret");
    assert!(credentials
        .load("remote-desktop:server-enabled")
        .expect("load")
        .is_none());
}

#[tokio::test]
async fn authorize_and_revoke_operate_on_the_shared_lan_client_record() {
    let dir = TempDir::new("authorize");
    seed_clients(
        &dir,
        r#"[{"id":"client-a","label":"Desk","createdAt":1,"lastUsedAt":null},
            {"id":"client-b","label":"Laptop","createdAt":2,"lastUsedAt":null}]"#,
    );
    let channel = channel(&dir);

    let state = call(
        &channel,
        "authorizeClient",
        json!({ "clientId": "client-a" }),
    )
    .await;
    assert_eq!(
        state["authorizedClients"],
        json!([{ "id": "client-a", "label": "Desk" }])
    );

    let state = call(&channel, "revokeClient", json!({ "clientId": "client-a" })).await;
    assert_eq!(state["authorizedClients"], json!([]));

    // 撤销必须删字段而不是置 false：否则记录会永久带上 `remoteDesktopAuthorized:false`。
    let credentials = CredentialStore::with_secret(&dir.0, "rd-channel-it-secret");
    let raw = credentials
        .load("lan:access:clients")
        .expect("load")
        .expect("present");
    assert!(!raw.contains("remoteDesktopAuthorized"), "{raw}");

    // 未知 clientId 幂等：不报错、不动其他记录。
    let state = call(
        &channel,
        "authorizeClient",
        json!({ "clientId": "missing" }),
    )
    .await;
    assert_eq!(state["authorizedClients"], json!([]));
}

#[tokio::test]
async fn encoder_capability_reads_only_whitelisted_values() {
    let dir = TempDir::new("capability");
    let channel = channel(&dir);
    std::fs::create_dir_all(dir.config_dir()).expect("config dir");
    let path = dir.config_dir().join("rd-encoder-capability.json");

    std::fs::write(&path, r#"{"capability":"software"}"#).unwrap();
    let state = call(&channel, "getServerState", Json::Array(vec![])).await;
    assert_eq!(state["encoderCapability"], "software");

    // 未知枚举值按无缓存处理（不猜、也不报错）。
    std::fs::write(&path, r#"{"capability":"turbo"}"#).unwrap();
    let state = call(&channel, "getServerState", Json::Array(vec![])).await;
    assert_eq!(state["encoderCapability"], "unknown");
}

#[tokio::test]
async fn get_peer_capability_is_offline_for_unpaired_peers() {
    let dir = TempDir::new("peer");
    let channel = channel(&dir);
    let capability = call(
        &channel,
        "getPeerCapability",
        json!({ "serverId": "never-paired" }),
    )
    .await;
    // 失败归 offline，且**不抛**（设置页据此灰掉入口，不需要 try/catch）。
    assert_eq!(capability["online"], false);
    assert_eq!(capability["remoteDesktop"], false);
    assert_eq!(capability["remoteDesktopNative"], false);
}

#[tokio::test]
async fn session_methods_require_the_host_port_and_never_fake_success() {
    let dir = TempDir::new("session");
    let channel = channel(&dir);

    // 未装配端口：被控态恒 false（本机没有活跃会话），不是错误。
    let controlled = call(&channel, "getControlledState", Json::Array(vec![])).await;
    assert_eq!(controlled["controlled"], false);

    // 会话方法必须显式报错，不能返回假成功。
    for method in [
        "startSession",
        "sendSignal",
        "endSession",
        "respondConsent",
        "setLocalDisplays",
    ] {
        let error = call_err(&channel, method, json!({})).await;
        assert!(
            error.contains("requires the host session port"),
            "{method}: {error}"
        );
    }
}

#[tokio::test]
async fn unknown_command_is_rejected() {
    let dir = TempDir::new("unknown");
    let channel = channel(&dir);
    let error = call_err(&channel, "nope", json!({})).await;
    assert!(error.contains("unknown command"), "{error}");
}
