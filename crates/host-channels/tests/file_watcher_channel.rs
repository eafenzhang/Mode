//! `file-watcher` 频道适配器：方法、动态事件订阅与失效 id。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use file_watcher::FileWatcherService;
use host_channels::file_watcher::{DISPOSE_ALL, ON_DYNAMIC_CHANGE, UNWATCH, WATCH};
use host_channels::FileWatcherChannel;
use rpc_host::{CancellationToken, EventCallback, ServerChannel};
use rpc_wire::Value;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-file-watcher-channel-{label}-{}-{}",
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

fn channel() -> (FileWatcherChannel, Arc<FileWatcherService>) {
    let service = Arc::new(FileWatcherService::new());
    (FileWatcherChannel::new(Arc::clone(&service)), service)
}

async fn call(channel: &FileWatcherChannel, method: &str, arg: Value) -> Result<Value, rpc_host::ChannelError> {
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

#[tokio::test]
async fn watch_returns_id_and_dispose_all_succeeds() {
    let root = TempRoot::new("watch");
    let (channel, service) = channel();

    let body = json(
        &call(
            &channel,
            WATCH,
            object(serde_json::json!({ "path": root.path().to_string_lossy() })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(body["id"], "0");
    assert_eq!(file_watcher::open_watcher_count(&service), 1);

    assert_eq!(
        call(&channel, UNWATCH, object(serde_json::json!({ "id": "0" })))
            .await
            .unwrap(),
        Value::Undefined
    );
    assert_eq!(file_watcher::open_watcher_count(&service), 0);

    assert_eq!(
        call(&channel, DISPOSE_ALL, Value::Array(Vec::new()))
            .await
            .unwrap(),
        Value::Undefined
    );
}

#[tokio::test]
async fn dynamic_change_emits_json_events() {
    let root = TempRoot::new("events");
    let (channel, _service) = channel();
    let body = json(
        &call(
            &channel,
            WATCH,
            object(serde_json::json!({ "path": root.path().to_string_lossy() })),
        )
        .await
        .unwrap(),
    );
    let id = body["id"].as_str().unwrap().to_string();

    let (tx, rx) = mpsc::channel();
    let emit: EventCallback = Arc::new(move |value: Value| {
        let _ = tx.send(value);
    });
    let _subscription = channel
        .listen("ctx".into(), ON_DYNAMIC_CHANGE.into(), Value::Str(id.clone()), emit)
        .expect("订阅必须成功");

    fs::write(root.path().join("a.txt"), "x").unwrap();
    let value = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("必须收到变更事件");
    let event = json(&value);
    assert_eq!(event["dirPath"], root.path().to_string_lossy().as_ref());
    assert!(event["changedPath"]
        .as_str()
        .unwrap_or_default()
        .ends_with("a.txt"));
}

#[tokio::test]
async fn stale_watcher_subscription_yields_no_events() {
    let (channel, _service) = channel();
    let (tx, rx) = mpsc::channel();
    let emit: EventCallback = Arc::new(move |value: Value| {
        let _ = tx.send(value);
    });
    let _subscription = channel
        .listen(
            "ctx".into(),
            ON_DYNAMIC_CHANGE.into(),
            Value::Str("missing".into()),
            emit,
        )
        .expect("失效 id 不报错（对应 Event.None）");
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
}

#[tokio::test]
async fn missing_directory_and_unknown_commands_are_errors() {
    let root = TempRoot::new("errors");
    let (channel, _service) = channel();
    let missing = root.path().join("nope");
    let error = call(
        &channel,
        WATCH,
        object(serde_json::json!({ "path": missing.to_string_lossy() })),
    )
    .await
    .unwrap_err();
    match error {
        rpc_host::ChannelError::Error { message, .. } => {
            assert!(message.starts_with("无法监视目录 '"), "{message}")
        }
        other => panic!("应当是 Error 变体，实际 {other:?}"),
    }

    assert!(call(&channel, "watchAll", Value::Array(Vec::new()))
        .await
        .is_err());
    assert!(channel
        .listen(
            "ctx".into(),
            "onOther".into(),
            Value::Str("0".into()),
            Arc::new(|_| {})
        )
        .is_err());
}

#[tokio::test]
async fn listen_accepts_json_encoded_id() {
    let root = TempRoot::new("json-id");
    let (channel, _service) = channel();
    let body = json(
        &call(
            &channel,
            WATCH,
            object(serde_json::json!({ "path": root.path().to_string_lossy() })),
        )
        .await
        .unwrap(),
    );
    let id = body["id"].as_str().unwrap().to_string();

    let (tx, rx) = mpsc::channel();
    let emit: EventCallback = Arc::new(move |value: Value| {
        let _ = tx.send(value);
    });
    let _subscription = channel
        .listen(
            "ctx".into(),
            ON_DYNAMIC_CHANGE.into(),
            Value::Json(serde_json::to_string(&id).unwrap()),
            emit,
        )
        .unwrap();
    fs::write(root.path().join("b.txt"), "x").unwrap();
    assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
}
