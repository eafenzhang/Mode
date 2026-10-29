//! `file-watcher` 服务行为（真实文件系统 + notify）。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use file_watcher::{open_watcher_count, FileWatchEvent, FileWatcherService};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-file-watcher-{label}-{}-{}",
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

fn subscribe(
    service: &FileWatcherService,
    id: &str,
) -> (mpsc::Receiver<FileWatchEvent>, Box<dyn Fn()>) {
    let (tx, rx) = mpsc::channel();
    let subscription = service
        .subscribe(
            id,
            Arc::new(move |event: &FileWatchEvent| {
                let _ = tx.send(event.clone());
            }),
        )
        .expect("watcher 必须存在");
    (rx, subscription)
}

fn wait_for_changed_path(rx: &mpsc::Receiver<FileWatchEvent>) -> FileWatchEvent {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "等待变更事件超时");
        match rx.recv_timeout(remaining) {
            Ok(event) if event.changed_path.is_some() => return event,
            Ok(_) => continue,
            Err(error) => panic!("等待变更事件失败: {error}"),
        }
    }
}

#[test]
fn single_file_change_reports_changed_path() {
    let root = TempRoot::new("single");
    let service = FileWatcherService::new();
    let id = service.watch(&root.path().to_string_lossy(), false).unwrap();
    assert_eq!(id, "0");
    assert_eq!(open_watcher_count(&service), 1);

    let (rx, subscription) = subscribe(&service, &id);
    fs::write(root.path().join("a.txt"), "x").unwrap();

    let event = wait_for_changed_path(&rx);
    assert_eq!(event.dir_path, root.path().to_string_lossy());
    let changed = event.changed_path.unwrap();
    assert!(changed.ends_with("a.txt"), "{changed}");

    subscription();
    service.unwatch(&id);
    assert_eq!(open_watcher_count(&service), 0);
}

#[test]
fn recursive_watch_reports_nested_changes() {
    let root = TempRoot::new("recursive");
    fs::create_dir_all(root.path().join("nested/deep")).unwrap();
    let service = FileWatcherService::new();
    let id = service.watch(&root.path().to_string_lossy(), true).unwrap();

    let (rx, _subscription) = subscribe(&service, &id);
    fs::write(root.path().join("nested/deep/b.txt"), "x").unwrap();

    let event = wait_for_changed_path(&rx);
    let changed = event.changed_path.unwrap();
    assert!(changed.ends_with("b.txt"), "{changed}");
    service.unwatch(&id);
}

#[test]
fn unwatch_stops_events() {
    let root = TempRoot::new("unwatch");
    let service = FileWatcherService::new();
    let id = service.watch(&root.path().to_string_lossy(), false).unwrap();
    let (rx, _subscription) = subscribe(&service, &id);

    service.unwatch(&id);
    assert_eq!(open_watcher_count(&service), 0);
    fs::write(root.path().join("after.txt"), "x").unwrap();
    std::thread::sleep(Duration::from_millis(600));
    assert!(rx.try_recv().is_err(), "unwatch 后不应再有事件");
}

#[test]
fn dispose_all_stops_every_watcher() {
    let root = TempRoot::new("dispose");
    let service = FileWatcherService::new();
    let id = service.watch(&root.path().to_string_lossy(), false).unwrap();
    let (rx, _subscription) = subscribe(&service, &id);

    service.dispose_all();
    assert_eq!(open_watcher_count(&service), 0);
    fs::write(root.path().join("after.txt"), "x").unwrap();
    std::thread::sleep(Duration::from_millis(600));
    assert!(rx.try_recv().is_err(), "disposeAll 后不应再有事件");
}

#[test]
fn multiple_subscribers_receive_the_same_event() {
    let root = TempRoot::new("multi");
    let service = FileWatcherService::new();
    let id = service.watch(&root.path().to_string_lossy(), false).unwrap();
    let (first_rx, _first) = subscribe(&service, &id);
    let (second_rx, _second) = subscribe(&service, &id);

    fs::write(root.path().join("a.txt"), "x").unwrap();
    let first = wait_for_changed_path(&first_rx);
    let second = wait_for_changed_path(&second_rx);
    assert_eq!(first.dir_path, second.dir_path);
    service.unwatch(&id);
}

#[test]
fn missing_directory_reports_ts_error_message() {
    let root = TempRoot::new("missing");
    let service = FileWatcherService::new();
    let missing = root.path().join("does-not-exist");
    let error = service.watch(&missing.to_string_lossy(), false).unwrap_err();
    assert!(
        error.message.starts_with("无法监视目录 '"),
        "{}",
        error.message
    );
}

#[test]
fn stale_subscription_is_none_and_warns() {
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&warnings);
    let service = FileWatcherService::with_warn_sink(Some(Arc::new(move |message: &str| {
        sink.lock().unwrap().push(message.to_string());
    })));

    assert!(service.subscribe("nope", Arc::new(|_| {})).is_none());
    let warnings = warnings.lock().unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("忽略已失效的文件监听订阅"));
}

#[test]
fn watcher_ids_are_monotonic() {
    let root = TempRoot::new("ids");
    let service = FileWatcherService::new();
    let first = service.watch(&root.path().to_string_lossy(), false).unwrap();
    let second = service.watch(&root.path().to_string_lossy(), false).unwrap();
    assert_eq!(first, "0");
    assert_eq!(second, "1");
    service.dispose_all();
}
