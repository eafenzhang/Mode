//! `memory` 频道适配器：方法名、位置参数与错误码映射。

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use host_channels::{MemoryChannel, LIST_MEMORIES, MEMORY_CHANNEL, READ_MEMORY_FILE};
use project_memory::ProjectMemoryService;
use rpc_host::{ChannelError, ServerChannel};
use rpc_wire::Value;

fn unique_root(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "mode-memory-channel-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("建临时根");
    root
}

fn write_memory(root: &std::path::Path, workspace: &str, file: &str, content: &str) {
    let dir = root.join("cli/memories/projects").join(workspace).join("memory");
    fs::create_dir_all(&dir).expect("建 memory 目录");
    fs::write(dir.join(file), content).expect("写文件");
}

fn channel(root: &std::path::Path) -> MemoryChannel {
    MemoryChannel::new(Arc::new(ProjectMemoryService::new(root)))
}

fn json_body(value: &Value) -> serde_json::Value {
    match value {
        Value::Json(text) => serde_json::from_str(text).expect("body 必须是 JSON"),
        other => panic!("期望 JSON body，实际 {other:?}"),
    }
}

#[tokio::test]
async fn lists_memories_through_positional_arguments() {
    let root = unique_root("list");
    write_memory(&root, "ws-0123456789abcdef", "MEMORY.md", "# index");
    let channel = channel(&root);

    let body = channel
        .call(
            "ctx".into(),
            LIST_MEMORIES.into(),
            Value::Array(Vec::new()),
            rpc_host::CancellationToken::default(),
        )
        .await
        .expect("调用成功");
    let payload = json_body(&body);
    assert_eq!(payload[0]["id"], "ws-0123456789abcdef");
    assert_eq!(payload[0]["label"], "ws");
    assert_eq!(payload[0]["files"][0]["kind"], "index");
}

#[tokio::test]
async fn reads_a_file_with_object_argument() {
    let root = unique_root("read");
    write_memory(&root, "ws-0123456789abcdef", "MEMORY.md", "# hello");
    let channel = channel(&root);

    let argument = Value::Json(
        serde_json::json!({ "workspaceId": "ws-0123456789abcdef", "fileName": "MEMORY.md" })
            .to_string(),
    );
    let body = channel
        .call(
            "ctx".into(),
            READ_MEMORY_FILE.into(),
            Value::Array(vec![argument]),
            rpc_host::CancellationToken::default(),
        )
        .await
        .expect("调用成功");
    assert_eq!(json_body(&body)["content"], "# hello");
}

#[tokio::test]
async fn maps_error_codes_to_channel_error_extras() {
    let root = unique_root("error");
    let dir = root.join("cli/memories/projects/ws-0123456789abcdef/memory");
    fs::create_dir_all(&dir).expect("建目录");
    fs::write(dir.join("MEMORY.md"), vec![b'x'; 5 * 1024 * 1024 + 16]).expect("写大文件");
    let channel = channel(&root);

    let argument = Value::Json(
        serde_json::json!({ "workspaceId": "ws-0123456789abcdef", "fileName": "MEMORY.md" })
            .to_string(),
    );
    let error = channel
        .call(
            "ctx".into(),
            READ_MEMORY_FILE.into(),
            Value::Array(vec![argument]),
            rpc_host::CancellationToken::default(),
        )
        .await
        .expect_err("应报错");
    match error {
        ChannelError::Error { extra, .. } => {
            assert!(
                extra
                    .iter()
                    .any(|(key, value)| key == "code" && value == &Value::Str("PROJECT_MEMORY_PREVIEW_LIMIT_EXCEEDED".into())),
                "必须带 TS 对齐的错误码：{extra:?}"
            );
        }
        other => panic!("期望 Error 形态，实际 {other:?}"),
    }
}

#[tokio::test]
async fn rejects_missing_arguments_and_unknown_commands() {
    let root = unique_root("invalid");
    let channel = channel(&root);

    let missing = channel
        .call(
            "ctx".into(),
            READ_MEMORY_FILE.into(),
            Value::Undefined,
            rpc_host::CancellationToken::default(),
        )
        .await;
    assert!(missing.is_err());

    let unknown = channel
        .call(
            "ctx".into(),
            "notAMethod".into(),
            Value::Undefined,
            rpc_host::CancellationToken::default(),
        )
        .await;
    assert!(unknown.is_err());
    assert_eq!(MEMORY_CHANNEL, "memory");
}
