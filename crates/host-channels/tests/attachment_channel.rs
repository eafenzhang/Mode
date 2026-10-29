//! `prompt-attachment-transfer` 适配器：本地零拷贝语义。

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use host_channels::attachment_transfer::{
    ADOPT, CANCEL, CLEANUP, PROGRESS_EVENT, STAGE,
};
use host_channels::AttachmentTransferChannel;
use rpc_host::{CancellationToken, ServerChannel};
use rpc_wire::Value;

fn unique_file(name: &str, bytes: usize) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "mode-attachment-{name}-{}-{}-{}.bin",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
        bytes
    ));
    fs::write(&path, vec![b'x'; bytes]).expect("写临时附件");
    path
}

fn stage_arg(path: &std::path::Path, size_bytes: Option<u64>) -> Value {
    let mut params = serde_json::json!({
        "operationId": "op-1",
        "sessionId": "s-1",
        "workspacePath": "D:/w",
        "localPath": path.to_string_lossy(),
        "fileName": "a.bin",
        "mime": "application/octet-stream",
    });
    if let Some(size) = size_bytes {
        params["sizeBytes"] = serde_json::json!(size);
    }
    Value::Array(vec![Value::Json(params.to_string())])
}

fn json_body(value: &Value) -> serde_json::Value {
    match value {
        Value::Json(text) => serde_json::from_str(text).expect("JSON body"),
        other => panic!("期望 JSON body，实际 {other:?}"),
    }
}

#[tokio::test]
async fn stage_reports_zero_copy_reference_and_declared_size() {
    let file = unique_file("declared", 3);
    let channel = AttachmentTransferChannel;
    let body = channel
        .call(
            "ctx".into(),
            STAGE.into(),
            stage_arg(&file, Some(99)),
            CancellationToken::default(),
        )
        .await
        .expect("stage 成功");
    let payload = json_body(&body);
    assert_eq!(payload["operationId"], "op-1");
    assert_eq!(payload["ref"], file.to_string_lossy().as_ref());
    assert_eq!(payload["bytes"], 99, "显式 sizeBytes 优先于 stat");
    assert_eq!(payload["staged"], false);
}

#[tokio::test]
async fn stage_falls_back_to_stat_and_then_zero() {
    let file = unique_file("stat", 7);
    let channel = AttachmentTransferChannel;
    let body = channel
        .call(
            "ctx".into(),
            STAGE.into(),
            stage_arg(&file, None),
            CancellationToken::default(),
        )
        .await
        .expect("stage 成功");
    assert_eq!(json_body(&body)["bytes"], 7);

    let missing = std::env::temp_dir().join("mode-attachment-does-not-exist.bin");
    let _ = fs::remove_file(&missing);
    let body = channel
        .call(
            "ctx".into(),
            STAGE.into(),
            stage_arg(&missing, None),
            CancellationToken::default(),
        )
        .await
        .expect("stage 成功");
    assert_eq!(json_body(&body)["bytes"], 0, "stat 失败回落 0，与 TS 一致");
}

#[tokio::test]
async fn lifecycle_methods_are_noops() {
    let channel = AttachmentTransferChannel;
    for command in [ADOPT, CANCEL, CLEANUP] {
        let value = channel
            .call(
                "ctx".into(),
                command.into(),
                Value::Array(vec![Value::Str("op-1".into())]),
                CancellationToken::default(),
            )
            .await
            .expect("no-op 成功");
        assert_eq!(value, Value::Undefined);
    }
}

#[tokio::test]
async fn progress_event_has_a_subscription_but_never_fires() {
    let channel = AttachmentTransferChannel;
    let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&fired);
    let emit: rpc_host::EventCallback = std::sync::Arc::new(move |_| {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let subscription = channel
        .listen("ctx".into(), PROGRESS_EVENT.into(), Value::Undefined, emit)
        .expect("订阅成功");
    subscription();
    assert!(!fired.load(std::sync::atomic::Ordering::SeqCst));
    assert!(
        channel
            .listen("ctx".into(), "unknown".into(), Value::Undefined, std::sync::Arc::new(|_| {}))
            .is_err()
    );
}

#[tokio::test]
async fn rejects_missing_arguments() {
    let channel = AttachmentTransferChannel;
    let error = channel
        .call(
            "ctx".into(),
            STAGE.into(),
            Value::Undefined,
            CancellationToken::default(),
        )
        .await;
    assert!(error.is_err());
}
