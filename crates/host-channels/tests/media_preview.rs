//! `media-preview` 适配器：本地的 inline 分支。

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use host_channels::media_preview::{INLINE_MAX_BYTES, PREPARE};
use host_channels::MediaPreviewChannel;
use rpc_host::{CancellationToken, ServerChannel};
use rpc_wire::Value;

fn unique_file(name: &str, extension: &str, bytes: &[u8]) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "mode-media-{name}-{}-{}{extension}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&path, bytes).expect("写临时媒体文件");
    path
}

fn prepare_arg(path: &std::path::Path, kind: &str) -> Value {
    Value::Array(vec![Value::Json(
        serde_json::json!({ "path": path.to_string_lossy(), "expectedKind": kind }).to_string(),
    )])
}

fn body(value: &Value) -> serde_json::Value {
    match value {
        Value::Json(text) => serde_json::from_str(text).expect("JSON body"),
        other => panic!("期望 JSON body，实际 {other:?}"),
    }
}

#[tokio::test]
async fn prepares_inline_audio_with_base64_and_media_type() {
    let file = unique_file("audio", ".mp3", b"ID3hello");
    let value = MediaPreviewChannel
        .call(
            "ctx".into(),
            PREPARE.into(),
            prepare_arg(&file, "audio"),
            CancellationToken::default(),
        )
        .await
        .expect("prepare 成功");
    let payload = body(&value);
    assert_eq!(payload["kind"], "inline");
    assert_eq!(payload["mediaType"], "audio/mpeg");
    assert_eq!(payload["size"], 8);
    assert_eq!(payload["path"], file.to_string_lossy().as_ref());
    assert_eq!(payload["dataBase64"], "SUQzaGVsbG8=");
}

#[tokio::test]
async fn rejects_kind_mismatch_and_unsupported_extensions() {
    let file = unique_file("kind", ".mp4", b"x");
    let mismatch = MediaPreviewChannel
        .call(
            "ctx".into(),
            PREPARE.into(),
            prepare_arg(&file, "audio"),
            CancellationToken::default(),
        )
        .await;
    assert!(mismatch.is_err(), "kind 不匹配必须报错");

    let text = unique_file("text", ".txt", b"x");
    let unsupported = MediaPreviewChannel
        .call(
            "ctx".into(),
            PREPARE.into(),
            prepare_arg(&text, "video"),
            CancellationToken::default(),
        )
        .await;
    assert!(unsupported.is_err(), "不支持的扩展名必须报错");
}

#[tokio::test]
async fn rejects_files_over_the_inline_limit() {
    let file = unique_file("large", ".mp4", &vec![b'x'; INLINE_MAX_BYTES as usize + 1]);
    let error = MediaPreviewChannel
        .call(
            "ctx".into(),
            PREPARE.into(),
            prepare_arg(&file, "video"),
            CancellationToken::default(),
        )
        .await
        .expect_err("超过 8 MiB 必须报错");
    assert!(
        format!("{error:?}").contains("too large"),
        "错误文案应说明超限：{error:?}"
    );
}

#[tokio::test]
async fn rejects_directories_and_passes_through_unknown_method_errors() {
    let dir = std::env::temp_dir();
    let error = MediaPreviewChannel
        .call(
            "ctx".into(),
            PREPARE.into(),
            prepare_arg(&dir, "video"),
            CancellationToken::default(),
        )
        .await;
    // 目录没有媒体扩展名，先被扩展名校验拦下。
    assert!(error.is_err());

    assert!(
        MediaPreviewChannel
            .call(
                "ctx".into(),
                "refreshPlaybackUrl".into(),
                Value::Undefined,
                CancellationToken::default(),
            )
            .await
            .is_err(),
        "本地服务没有 host-range 方法"
    );
}

#[test]
fn extension_matching_is_case_insensitive_and_handles_backslashes() {
    // 通过 prepare 的扩展名判定间接验证：合法扩展名但文件不存在 → 报「无法 stat」而不是「不支持」。
    let channel = MediaPreviewChannel;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let error = runtime
        .block_on(channel.call(
            "ctx".into(),
            PREPARE.into(),
            Value::Array(vec![Value::Json(
                serde_json::json!({
                    "path": "C:\\Media\\Clip.MP4",
                    "expectedKind": "video",
                })
                .to_string(),
            )]),
            CancellationToken::default(),
        ))
        .expect_err("文件不存在");
    let text = format!("{error:?}");
    assert!(
        text.contains("Cannot stat") || text.contains("Unsupported"),
        "{text}"
    );
    assert!(!text.contains("Unsupported"), "大写扩展名与反斜杠都应被识别：{text}");
}
