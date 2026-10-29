//! `file-watcher` 服务：目录监视、150ms 防抖与变更事件。
//!
//! 行为真源 `packages/services/src/fileWatcher/fileWatcherService.ts`：
//! - 每个 `watch` 一个底层 watcher；默认非递归，`recursive: true` 时递归；
//! - 同一防抖窗口内只收集路径：恰好一个明确路径且没有未知变更时才透传 `changedPath`；
//! - 底层 watcher 异常时先发一条 `{dirPath}` 最终事件，再清理实例；
//! - 订阅失效 id 不抛错（返回空事件流），只记一条告警。

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;

pub const DEBOUNCE_MS: u64 = 150;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileWatchEvent {
    pub dir_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWatcherError {
    pub message: String,
}

impl std::fmt::Display for FileWatcherError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for FileWatcherError {}

pub type ChangeListener = Arc<dyn Fn(&FileWatchEvent) + Send + Sync>;
pub type SubscriptionHandle = Box<dyn Fn() + Send + Sync>;
pub type WarnSink = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Default)]
struct Emitter {
    listeners: Mutex<Vec<(u64, ChangeListener)>>,
    next_token: AtomicU64,
}

impl Emitter {
    fn subscribe(self: &Arc<Self>, listener: ChangeListener) -> SubscriptionHandle {
        let token = self.next_token.fetch_add(1, Ordering::Relaxed) + 1;
        self.listeners.lock().unwrap().push((token, listener));
        let emitter = Arc::clone(self);
        Box::new(move || {
            emitter
                .listeners
                .lock()
                .unwrap()
                .retain(|(candidate, _)| *candidate != token);
        })
    }

    fn fire(&self, event: &FileWatchEvent) {
        let snapshot: Vec<ChangeListener> = self
            .listeners
            .lock()
            .unwrap()
            .iter()
            .map(|(_, listener)| Arc::clone(listener))
            .collect();
        for listener in snapshot {
            listener(event);
        }
    }

    fn dispose(&self) {
        self.listeners.lock().unwrap().clear();
    }
}

#[derive(Default)]
struct DebounceState {
    pending: BTreeSet<String>,
    unknown: bool,
}

struct Entry {
    watcher: RecommendedWatcher,
    emitter: Arc<Emitter>,
    tick_tx: mpsc::Sender<()>,
    thread: JoinHandle<()>,
}

struct Inner {
    entries: Mutex<HashMap<String, Entry>>,
    next_id: AtomicU64,
    on_warn: Option<WarnSink>,
}

impl Inner {
    fn warn(&self, message: &str) {
        if let Some(sink) = &self.on_warn {
            sink(message);
        }
    }

    fn remove(&self, id: &str) {
        let entry = self.entries.lock().unwrap().remove(id);
        if let Some(entry) = entry {
            finalize(entry);
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let entries: Vec<Entry> = {
            let mut guard = self.entries.lock().unwrap();
            guard.drain().map(|(_, entry)| entry).collect()
        };
        for entry in entries {
            finalize(entry);
        }
    }
}

fn finalize(entry: Entry) {
    let Entry {
        watcher,
        emitter,
        tick_tx,
        thread,
    } = entry;
    emitter.dispose();
    drop(tick_tx);
    drop(watcher);
    // 不 join：订阅回调跑在防抖线程上，回调里 unwatch 会自己等自己。
    // 断开 sender 后线程会在下一次 recv 立即退出。
    drop(thread);
}

#[derive(Clone)]
pub struct FileWatcherService {
    inner: Arc<Inner>,
}

impl FileWatcherService {
    pub fn new() -> Self {
        Self::with_warn_sink(None)
    }

    pub fn with_warn_sink(on_warn: Option<WarnSink>) -> Self {
        Self {
            inner: Arc::new(Inner {
                entries: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(0),
                on_warn,
            }),
        }
    }

    pub fn watch(&self, path: &str, recursive: bool) -> Result<String, FileWatcherError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst).to_string();
        let emitter = Arc::new(Emitter::default());
        let state = Arc::new(Mutex::new(DebounceState::default()));
        let (tick_tx, tick_rx) = mpsc::channel::<()>();

        let handler = {
            let inner = Arc::downgrade(&self.inner);
            let emitter = Arc::clone(&emitter);
            let state = Arc::clone(&state);
            let tick_tx = tick_tx.clone();
            let watched_path = path.to_string();
            let watched_id = id.clone();
            move |result: notify::Result<notify::Event>| match result {
                Ok(event) => {
                    let mut guard = state.lock().unwrap();
                    for candidate in &event.paths {
                        match changed_path(&watched_path, candidate) {
                            Some(changed) => {
                                guard.pending.insert(changed);
                            }
                            None => guard.unknown = true,
                        }
                    }
                    drop(guard);
                    let _ = tick_tx.send(());
                }
                Err(error) => {
                    let message = format!(
                        "[file-watcher] 文件监听器异常，清理 watcher id={watched_id} path={watched_path}: {error}"
                    );
                    if let Some(inner) = inner.upgrade() {
                        inner.warn(&message);
                    }
                    emitter.fire(&FileWatchEvent {
                        dir_path: watched_path.clone(),
                        changed_path: None,
                    });
                    let cleanup_inner = inner.clone();
                    let cleanup_id = watched_id.clone();
                    // 不能在通知回调线程里 drop 正在回调的 watcher；交给独立线程清理。
                    std::thread::spawn(move || {
                        if let Some(inner) = cleanup_inner.upgrade() {
                            inner.remove(&cleanup_id);
                        }
                    });
                }
            }
        };

        let mut watcher = RecommendedWatcher::new(handler, notify::Config::default())
            .map_err(|error| watch_error(path, &error))?;
        let mode = if recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        watcher
            .watch(Path::new(path), mode)
            .map_err(|error| watch_error(path, &error))?;

        let thread = spawn_debouncer(
            tick_rx,
            Arc::clone(&state),
            Arc::clone(&emitter),
            path.to_string(),
        );
        self.inner.entries.lock().unwrap().insert(
            id.clone(),
            Entry {
                watcher,
                emitter,
                tick_tx,
                thread,
            },
        );
        Ok(id)
    }

    pub fn unwatch(&self, id: &str) {
        self.inner.remove(id);
    }

    pub fn dispose_all(&self) {
        let entries: Vec<Entry> = {
            let mut guard = self.inner.entries.lock().unwrap();
            guard.drain().map(|(_, entry)| entry).collect()
        };
        for entry in entries {
            finalize(entry);
        }
    }

    /// 订阅指定 watcher 的变更；id 已失效时返回 `None`（对应 TS 的 `Event.None`）。
    pub fn subscribe(&self, id: &str, listener: ChangeListener) -> Option<SubscriptionHandle> {
        let emitter = {
            let guard = self.inner.entries.lock().unwrap();
            guard.get(id).map(|entry| Arc::clone(&entry.emitter))
        };
        match emitter {
            Some(emitter) => Some(emitter.subscribe(listener)),
            None => {
                self.inner
                    .warn(&format!("[file-watcher] 忽略已失效的文件监听订阅 id={id}"));
                None
            }
        }
    }
}

impl Default for FileWatcherService {
    fn default() -> Self {
        Self::new()
    }
}

fn watch_error(path: &str, error: &notify::Error) -> FileWatcherError {
    FileWatcherError {
        message: format!("无法监视目录 '{path}': {error}"),
    }
}

fn spawn_debouncer(
    tick_rx: mpsc::Receiver<()>,
    state: Arc<Mutex<DebounceState>>,
    emitter: Arc<Emitter>,
    dir_path: String,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        loop {
            if tick_rx.recv().is_err() {
                return;
            }
            loop {
                match tick_rx.recv_timeout(Duration::from_millis(DEBOUNCE_MS)) {
                    Ok(()) => continue,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let event = {
                            let mut guard = state.lock().unwrap();
                            let changed_path =
                                if !guard.unknown && guard.pending.len() == 1 {
                                    guard.pending.iter().next().cloned()
                                } else {
                                    None
                                };
                            guard.pending.clear();
                            guard.unknown = false;
                            FileWatchEvent {
                                dir_path: dir_path.clone(),
                                changed_path,
                            }
                        };
                        emitter.fire(&event);
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        }
    })
}

/// 把 notify 的完整路径折回 `path.resolve(watchedDir, fileName)` 形态；
/// 无法归到被监视目录内（含目录自身）时视为「未知变更」。
fn changed_path(watched_dir: &str, candidate: &Path) -> Option<String> {
    let candidate_text = candidate.to_string_lossy();
    if candidate_text.trim().is_empty() {
        return None;
    }
    let watched = Path::new(watched_dir);
    let absolute_watched = if watched.is_absolute() {
        watched.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(watched)
    };

    let relative = candidate
        .strip_prefix(&absolute_watched)
        .or_else(|_| candidate.strip_prefix(watched))
        .ok()?;
    if relative.as_os_str().is_empty() {
        return None;
    }
    Some(
        absolute_watched
            .join(relative)
            .to_string_lossy()
            .into_owned(),
    )
}

/// 供测试/诊断：当前打开的 watcher 数。
pub fn open_watcher_count(service: &FileWatcherService) -> usize {
    service.inner.entries.lock().unwrap().len()
}
