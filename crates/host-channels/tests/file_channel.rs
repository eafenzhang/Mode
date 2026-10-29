//! `file` 频道适配器：参数形状、顶层二进制返回与错误映射。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use file_service::{FileService, FileServiceOptions};
use host_channels::file::{
    LIST_WORKSPACE_FILES_LENGTH, LIST_WORKSPACE_FILES_RANGE, READDIR, READ_FILE_RANGE,
    READ_TEXT_FILE, READ_WORKSPACE_FILE_SEARCH_IGNORE, STAT, WRITE_WORKSPACE_FILE_SEARCH_IGNORE,
};
use host_channels::FileChannel;
use rpc_host::{CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-file-channel-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn channel(root: &Path) -> FileChannel {
    FileChannel::new(Arc::new(FileService::new(FileServiceOptions {
        data_root: root.to_path_buf(),
        home: root.to_path_buf(),
        on_warn: None,
    })))
}

async fn call(
    channel: &FileChannel,
    method: &str,
    arg: Value,
) -> Result<Value, ChannelError> {
    channel
        .call("ctx".into(), method.into(), arg, CancellationToken::default())
        .await
}

fn object(value: serde_json::Value) -> Value {
    Value::Array(vec![Value::Json(value.to_string())])
}

fn json(value: &Value) -> serde_json::Value {
    match value {
        Value::Json(text) => serde_json::from_str(text).expect("JSON body"),
        other => panic!("期望 JSON body，实际 {other:?}"),
    }
}

fn path_of(path: &Path) -> serde_json::Value {
    serde_json::Value::String(path.to_string_lossy().into_owned())
}

#[tokio::test]
async fn readdir_and_stat_return_expected_shapes() {
    let root = TempRoot::new("readdir");
    fs::create_dir(root.path().join("dir")).unwrap();
    fs::write(root.path().join("a.txt"), "hello").unwrap();
    let channel = channel(root.path());

    let entries = json(
        &call(
            &channel,
            READDIR,
            object(serde_json::json!({ "path": path_of(root.path()) })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(entries[0]["name"], "dir");
    assert_eq!(entries[0]["type"], "directory");
    assert_eq!(entries[1]["name"], "a.txt");
    assert_eq!(entries[1]["type"], "file");
    assert_eq!(entries[1]["isSymbolicLink"], false);

    let stat = json(
        &call(
            &channel,
            STAT,
            object(serde_json::json!({ "path": path_of(&root.path().join("a.txt")) })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(stat["type"], "file");
    assert_eq!(stat["size"], 5);
}

#[tokio::test]
async fn read_text_file_uses_camel_case_fields() {
    let root = TempRoot::new("text");
    fs::write(root.path().join("a.txt"), "hello").unwrap();
    let channel = channel(root.path());
    let slice = json(
        &call(
            &channel,
            READ_TEXT_FILE,
            object(serde_json::json!({ "path": path_of(&root.path().join("a.txt")) })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(slice["content"], "hello");
    assert_eq!(slice["bytesRead"], 5);
    assert_eq!(slice["totalBytes"], 5);
    assert_eq!(slice["isBinary"], false);
    assert_eq!(slice["truncated"], false);
}

#[tokio::test]
async fn read_file_range_returns_top_level_bytes() {
    let root = TempRoot::new("range");
    fs::write(root.path().join("a.bin"), [0u8, 1, 2, 3, 4]).unwrap();
    let channel = channel(root.path());
    let value = call(
        &channel,
        READ_FILE_RANGE,
        object(serde_json::json!({
            "path": path_of(&root.path().join("a.bin")),
            "offset": 1,
            "length": 2
        })),
    )
    .await
    .unwrap();
    match value {
        Value::Bytes(bytes) => assert_eq!(bytes, vec![1, 2]),
        other => panic!("顶层必须是二进制，实际 {other:?}"),
    }
}

#[tokio::test]
async fn workspace_index_methods_return_int_and_string() {
    let root = TempRoot::new("index");
    fs::write(root.path().join("a.txt"), "").unwrap();
    fs::write(
        root.path().join(file_service::WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME),
        "",
    )
    .unwrap();
    let channel = channel(root.path());
    let root_path = path_of(root.path());

    let length = call(
        &channel,
        LIST_WORKSPACE_FILES_LENGTH,
        object(serde_json::json!({ "rootPath": root_path })),
    )
    .await
    .unwrap();
    let length_number = match length {
        Value::Int(number) => number as i64,
        Value::Json(text) => text.parse::<i64>().unwrap(),
        other => panic!("长度必须是数字，实际 {other:?}"),
    };
    assert!(length_number > 0);

    let packed = call(
        &channel,
        LIST_WORKSPACE_FILES_RANGE,
        object(serde_json::json!({
            "rootPath": path_of(root.path()),
            "offset": 0,
            "length": 10_000
        })),
    )
    .await
    .unwrap();
    match packed {
        Value::Str(text) => {
            assert!(text.contains("file\ta.txt"), "{text}");
            assert_eq!(text.encode_utf16().count() as i64, length_number);
        }
        other => panic!("分块必须是裸字符串，实际 {other:?}"),
    }
}

#[tokio::test]
async fn ignore_read_and_write_round_trip() {
    let root = TempRoot::new("ignore");
    let channel = channel(root.path());
    let root_path = path_of(root.path());

    call(
        &channel,
        WRITE_WORKSPACE_FILE_SEARCH_IGNORE,
        object(serde_json::json!({
            "rootPath": root_path,
            "content": "node_modules/\n"
        })),
    )
    .await
    .unwrap();

    let read = json(
        &call(
            &channel,
            READ_WORKSPACE_FILE_SEARCH_IGNORE,
            object(serde_json::json!({ "rootPath": path_of(root.path()) })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(read["content"], "node_modules/\n");
    assert_eq!(read["source"], "file");
}

#[tokio::test]
async fn malformed_arguments_and_unknown_commands_are_errors() {
    let root = TempRoot::new("errors");
    let channel = channel(root.path());
    assert!(call(&channel, READDIR, Value::Array(Vec::new())).await.is_err());
    assert!(call(&channel, STAT, Value::Array(vec![Value::Json("{}".into())]))
        .await
        .is_err());
    let error = call(&channel, "truncateWorkspace", Value::Array(Vec::new()))
        .await
        .unwrap_err();
    match error {
        ChannelError::Error { message, .. } => assert!(message.contains("unknown command")),
        other => panic!("应当是 Error 变体，实际 {other:?}"),
    }
}

#[tokio::test]
async fn service_errors_keep_their_message() {
    let root = TempRoot::new("service-errors");
    let channel = channel(root.path());
    let error = call(
        &channel,
        READ_TEXT_FILE,
        object(serde_json::json!({ "path": path_of(root.path()) })),
    )
    .await
    .unwrap_err();
    match error {
        ChannelError::Error { message, .. } => {
            assert!(message.starts_with("Path is not a file:"), "{message}")
        }
        other => panic!("应当是 Error 变体，实际 {other:?}"),
    }
}
