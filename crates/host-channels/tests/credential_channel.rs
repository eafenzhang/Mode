//! `credential` 频道适配器：位置参数、null 语义、错误码透传。

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use credential_store::CredentialStore;
use host_channels::credential::{DELETE, LIST, LOAD, SAVE};
use host_channels::CredentialChannel;
use rpc_host::{CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;

fn unique_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "mode-credential-channel-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn channel(dir: &std::path::Path) -> CredentialChannel {
    CredentialChannel::new(Arc::new(CredentialStore::with_secret(
        dir,
        "channel-test-secret",
    )))
}

fn strings(values: &[&str]) -> Value {
    Value::Array(values.iter().map(|value| Value::Str((*value).into())).collect())
}

async fn call(channel: &CredentialChannel, method: &str, arg: Value) -> Result<Value, ChannelError> {
    channel
        .call("ctx".into(), method.into(), arg, CancellationToken::default())
        .await
}

fn json(value: &Value) -> serde_json::Value {
    match value {
        Value::Json(text) => serde_json::from_str(text).expect("JSON body"),
        other => panic!("期望 JSON body，实际 {other:?}"),
    }
}

#[tokio::test]
async fn save_load_delete_and_list_round_trip() {
    let dir = unique_dir("roundtrip");
    let channel = channel(&dir);

    assert_eq!(
        call(&channel, SAVE, strings(&["token", "secret-value"]))
            .await
            .unwrap(),
        Value::Undefined
    );

    // load 返回 JSON `"secret-value"`（不是 undefined）。
    assert_eq!(
        json(&call(&channel, LOAD, strings(&["token"])).await.unwrap()),
        serde_json::Value::String("secret-value".into())
    );
    // 缺失键必须返回 JSON `null`。
    assert_eq!(
        json(&call(&channel, LOAD, strings(&["missing"])).await.unwrap()),
        serde_json::Value::Null
    );

    assert_eq!(
        call(&channel, LIST, strings(&[""])).await.unwrap(),
        Value::Array(vec![Value::Str("token".into())])
    );
    assert_eq!(
        call(&channel, LIST, strings(&["missing:"])).await.unwrap(),
        Value::Array(Vec::new())
    );

    assert_eq!(
        call(&channel, DELETE, strings(&["token"])).await.unwrap(),
        Value::Undefined
    );
    assert_eq!(
        call(&channel, LIST, strings(&[""])).await.unwrap(),
        Value::Array(Vec::new())
    );
}

#[tokio::test]
async fn missing_arguments_are_rejected() {
    let dir = unique_dir("missing-args");
    let channel = channel(&dir);

    assert!(call(&channel, LOAD, Value::Array(Vec::new())).await.is_err());
    assert!(call(&channel, SAVE, strings(&["only-key"])).await.is_err());
    assert!(call(&channel, DELETE, Value::Array(Vec::new())).await.is_err());
}

#[tokio::test]
async fn decrypt_error_exposes_ts_code_and_prefix() {
    let dir = unique_dir("decrypt");
    fs::write(dir.join("credentials.json"), r#"{"k":"enc:v1:broken"}"#).unwrap();
    let channel = channel(&dir);

    let error = call(&channel, LOAD, strings(&["k"])).await.unwrap_err();
    let ChannelError::Error { message, extra, .. } = error else {
        panic!("应当是 Error 变体");
    };
    assert!(message.starts_with("凭据解密失败："), "消息: {message}");
    assert!(
        extra.iter().any(|(key, value)| {
            key == "code" && *value == Value::Str("MODE_CREDENTIAL_DECRYPT_FAILED".into())
        }),
        "必须带上 TS 的 code: {extra:?}"
    );
}

#[tokio::test]
async fn corrupt_store_is_reported_without_overwriting() {
    let dir = unique_dir("corrupt");
    let original = "{not json";
    fs::write(dir.join("credentials.json"), original).unwrap();
    let channel = channel(&dir);

    let error = call(&channel, SAVE, strings(&["k", "v"])).await.unwrap_err();
    match error {
        ChannelError::Error { message, .. } => {
            assert!(message.contains("Mode credentials are corrupt"), "{message}");
            assert!(!message.contains("not json"), "错误消息不得回显文件内容");
        }
        other => panic!("应当是 Error 变体，实际 {other:?}"),
    }
    assert_eq!(
        fs::read_to_string(dir.join("credentials.json")).unwrap(),
        original
    );
}

#[tokio::test]
async fn unknown_command_is_an_error() {
    let dir = unique_dir("unknown");
    let channel = channel(&dir);
    assert!(call(&channel, "wipe", Value::Array(Vec::new())).await.is_err());
}
