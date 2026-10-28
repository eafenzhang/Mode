//! 终端 PTY 会话管理（spec `terminal-pty-in-shell.md`）：PTY 进程下沉到 Tauri 壳。
//!
//! 动机：终端此前由 Host 进程用 node-pty 托管，Host 崩溃/重生（`taskkill /T`）会带走
//! 全部终端会话。进程所有权移到壳后，终端在 Host 的整个生命周期之外存活。
//!
//! ## 职责切分
//!
//! - **本模块**：PTY 进程（spawn/write/resize/kill）、输出泵线程、增量 UTF-8 解码；
//! - **Host**：profile（shell 选择/字体/主题/cwd 回退）——设置与字体探测逻辑在 TS 侧
//!   （`terminalProfile.ts`），Rust 复制必然漂移；壳 create 的输入就是 Host 算好的
//!   profile（shell + 已回退的 cwd）；
//! - **渲染层**：组装（Host profile + 壳 spawn）与消费（Tauri event 按 id 过滤）。
//!
//! 事件出口是可注入回调（[`TerminalEmit`]）：命令面桥到 `app.emit`，测试收进共享缓冲。
//! 本模块不依赖 Tauri 类型，用例在本机 gnu 路线可直接跑。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

/// 事件出口：Data 随输出块、Exit 随进程退出调用。回调必须快速返回
/// （命令面把它桥到 `app.emit`，阻塞会拖垮输出泵）。
pub type TerminalEmit = Arc<dyn Fn(TerminalEvent) + Send + Sync>;

#[derive(Debug, Clone)]
pub enum TerminalEvent {
    Data { id: String, data: String },
    Exit { id: String, code: i64 },
}

/// 读缓冲的软上限：单次攒下的未解码字节数超过它就丢最老的字节并留痕。
/// 终端输出是流，正常永远到不了这个值；到达只可能是输出泵被卡住的异常态。
const READ_BUFFER_LIMIT: usize = 1024 * 1024;

struct Session {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
    /// 跨整页 reload reattach 的输出回放（有界；见 `HISTORY_LIMIT`）。
    history: Arc<Mutex<Vec<u8>>>,
}

/// 输出回放上限：只保证 reattach 后能看到当前屏附近的内容，不承诺完整 scrollback。
const HISTORY_LIMIT: usize = 256 * 1024;

/// `create_with_key` 的返回：`reattached=true` 时 `replay` 是已有会话的输出快照。
pub struct TerminalCreateResult {
    pub id: String,
    pub replay: String,
    pub reattached: bool,
}

/// 稳定 key → 会话 id 的双向索引；进程退出/dispose 时按 id 清理，避免 key 指向死会话。
#[derive(Default)]
struct KeyIndex {
    by_key: HashMap<String, String>,
    by_id: HashMap<String, String>,
}

impl KeyIndex {
    fn get(&self, key: &str) -> Option<&str> {
        self.by_key.get(key).map(String::as_str)
    }

    fn insert(&mut self, key: &str, id: &str) {
        self.by_key.insert(key.to_string(), id.to_string());
        self.by_id.insert(id.to_string(), key.to_string());
    }

    fn remove_id(&mut self, id: &str) {
        if let Some(key) = self.by_id.remove(id) {
            self.by_key.remove(&key);
        }
    }
}

/// 终端会话表。`create/write/resize/dispose` 均可从多线程调用（命令面）。
pub struct TerminalSessionManager {
    sessions: Mutex<HashMap<String, Session>>,
    keys: Mutex<KeyIndex>,
    next_id: AtomicU64,
    emit: TerminalEmit,
}

impl TerminalSessionManager {
    pub fn new(emit: TerminalEmit) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            keys: Mutex::new(KeyIndex::default()),
            next_id: AtomicU64::new(1),
            emit,
        }
    }

    /// 起一个 PTY 会话并启动输出泵/退出监视线程。`cwd` 由调用方解析好
    /// （Host profile 已做失效回退）；解析失败与 spawn 失败都以错误串上抛。
    ///
    /// `&Arc<Self>`：退出监视线程持有 manager 的 Arc，进程退出后从表里移除会话。
    pub fn create(
        self: &Arc<Self>,
        cols: u16,
        rows: u16,
        cwd: Option<&str>,
        shell: &str,
    ) -> Result<String, String> {
        self.create_with_key(cols, rows, cwd, shell, None)
            .map(|result| result.id)
    }

    /// 带稳定 key 的创建/重挂接：key 命中活会话时返回原 id + 有界输出回放；
    /// 否则按普通 create 起新会话并登记 key。key 只用于壳内 reattach，不参与进程语义。
    pub fn create_with_key(
        self: &Arc<Self>,
        cols: u16,
        rows: u16,
        cwd: Option<&str>,
        shell: &str,
        key: Option<&str>,
    ) -> Result<TerminalCreateResult, String> {
        if let Some(key) = key {
            let existing_id = lock(&self.keys).get(key).map(str::to_string);
            if let Some(id) = existing_id {
                let history = lock(&self.sessions)
                    .get(&id)
                    .map(|session| Arc::clone(&session.history));
                if let Some(history) = history {
                    let replay = String::from_utf8_lossy(&lock(&history)).into_owned();
                    return Ok(TerminalCreateResult {
                        id,
                        replay,
                        reattached: true,
                    });
                }
                // 会话已退出但索引还没清：清掉陈旧 key，走新建。
                lock(&self.keys).remove_id(&id);
            }
        }

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| format!("openpty failed: {error}"))?;

        let mut command = CommandBuilder::new(shell);
        if let Some(cwd) = cwd {
            command.cwd(cwd);
        }
        let slave = pair.slave;
        let child = slave
            .spawn_command(command)
            .map_err(|error| format!("spawn {shell} failed: {error}"))?;
        drop(slave); // 母端持有即可；子进程已拿到自己的控制台

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| format!("clone pty reader failed: {error}"))?;
        let writer = Arc::new(Mutex::new(
            pair.master
                .take_writer()
                .map_err(|error| format!("take pty writer failed: {error}"))?,
        ));

        let id = format!("term-{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        let history = Arc::new(Mutex::new(Vec::new()));

        {
            let emit = Arc::clone(&self.emit);
            let pump_id = id.clone();
            let pump_writer = Arc::clone(&writer);
            let pump_history = Arc::clone(&history);
            thread::spawn(move || {
                pump_output(pump_id, reader, emit, pump_writer, pump_history);
            });
        }
        self.insert(
            &id,
            Session {
                master: pair.master,
                writer,
                child: Mutex::new(child),
                history,
            },
        );
        if let Some(key) = key {
            lock(&self.keys).insert(key, &id);
        }
        {
            let manager = Arc::clone(self);
            let exit_id = id.clone();
            thread::spawn(move || watch_exit(manager, exit_id));
        }
        Ok(TerminalCreateResult {
            id,
            replay: String::new(),
            reattached: false,
        })
    }

    fn insert(&self, id: &str, session: Session) {
        lock(&self.sessions).insert(id.to_string(), session);
    }

    fn remove(&self, id: &str) -> Option<Session> {
        lock(&self.keys).remove_id(id);
        lock(&self.sessions).remove(id)
    }

    pub fn write(&self, id: &str, data: &str) -> Result<(), String> {
        let sessions = lock(&self.sessions);
        let Some(session) = sessions.get(id) else {
            return Err(format!("terminal session {id} not found"));
        };
        let mut writer = session
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        writer
            .write_all(data.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|error| format!("terminal write failed: {error}"))
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let sessions = lock(&self.sessions);
        let Some(session) = sessions.get(id) else {
            return Err(format!("terminal session {id} not found"));
        };
        session
            .master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| format!("terminal resize failed: {error}"))
    }

    /// 按稳定 key 结束会话（渲染层关闭带 key 的 tab 时用）。
    /// key 不存在或会话已退出按幂等成功处理，避免关闭旧 tab 时报错；
    /// 返回被释放的会话 id，供渲染层清理按 id 登记的事件 emitter。
    pub fn dispose_by_key(&self, key: &str) -> Result<Option<String>, String> {
        let id = lock(&self.keys).get(key).map(str::to_string);
        match id {
            Some(id) => {
                self.dispose(&id)?;
                Ok(Some(id))
            }
            None => Ok(None),
        }
    }

    /// 结束会话：杀进程；Exit 事件与会话移除交给退出监视线程统一处理（幂等——
    /// 对已死进程 kill 的错误按已退出处理）。
    pub fn dispose(&self, id: &str) -> Result<(), String> {
        // 先摘 key：dispose 与退出监视线程之间有 200ms 窗口，期间同 key 的重挂应新建而不是
        // 命中一个正在被杀死的会话。
        lock(&self.keys).remove_id(id);
        let sessions = lock(&self.sessions);
        let Some(session) = sessions.get(id) else {
            return Ok(());
        };
        let mut child = session
            .child
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        child
            .kill()
            .map_err(|error| format!("terminal kill failed: {error}"))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 退出监视线程：轮询 `try_wait`（不阻塞、无需持锁跨等待），进程退出后发 Exit
/// 并从表里移除会话。会话不在表里（已被移除）就直接返回。
fn watch_exit(manager: Arc<TerminalSessionManager>, id: String) {
    loop {
        thread::sleep(Duration::from_millis(200));
        let code = {
            let sessions = lock(&manager.sessions);
            let Some(session) = sessions.get(&id) else {
                return;
            };
            let mut child = session
                .child
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match child.try_wait() {
                Ok(Some(status)) => status.exit_code() as i64,
                Ok(None) => continue,
                Err(_) => -1,
            }
        };
        (manager.emit)(TerminalEvent::Exit { id: id.clone(), code });
        manager.remove(&id);
        return;
    }
}

/// 输出泵：读到什么就（增量 UTF-8 解码后）发什么；EOF/错误即结束。
/// UTF-8 按「最长有效前缀」切分——多字节序列跨块时把尾巴留给下一轮，
/// 非法字节（error_len = Some(n)）丢弃 n 字节防卡死。
fn pump_output(
    id: String,
    mut reader: impl Read,
    emit: TerminalEmit,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    history: Arc<Mutex<Vec<u8>>>,
) {
    let mut chunk = [0u8; 8192];
    let mut pending: Vec<u8> = Vec::new();
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => {
                pending.extend_from_slice(&chunk[..n]);
                if pending.len() > READ_BUFFER_LIMIT {
                    pending.drain(..pending.len() - READ_BUFFER_LIMIT);
                }
                match std::str::from_utf8(&pending) {
                    Ok(text) => {
                        // 先拷成 owned 解除对 pending 的借用：clear 与 DSR 检查都在其后。
                        let text = text.to_string();
                        if !text.is_empty() {
                            append_history(&history, text.as_bytes());
                            emit(TerminalEvent::Data {
                                id: id.clone(),
                                data: text.clone(),
                            });
                        }
                        pending.clear();
                        // ConPTY 启动时会发 DSR 位置查询并**阻塞等待应答**——产品里由
                        // xterm.js 自动应答（经 write 回流）；泵在此兜底应答，保证无头
                        // 消费者（测试/后台形态）也能让输出流动。
                        if text.contains("\u{1b}[6n") {
                            let mut writer = writer
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            let _ = writer.write_all("\u{1b}[1;1R".as_bytes());
                            let _ = writer.flush();
                        }
                    }
                    Err(error) => {
                        let valid = error.valid_up_to();
                        if valid > 0 {
                            let text = std::str::from_utf8(&pending[..valid])
                                .expect("valid_up_to 前缀必为合法 UTF-8");
                            append_history(&history, text.as_bytes());
                            emit(TerminalEvent::Data {
                                id: id.clone(),
                                data: text.to_string(),
                            });
                        }
                        match error.error_len() {
                            Some(bad) => {
                                pending.drain(..bad);
                            }
                            None => {
                                pending.drain(..valid);
                            }
                        }
                    }
                }
            }
            Err(_) => return,
        }
    }
}

/// 追加输出到有界回放缓冲；超出上限时从头部丢弃，并退到 UTF-8 字符边界，
/// 避免 replay 从一个多字节字符中间开始（快照本身仍用 lossy 兜底）。
fn append_history(history: &Mutex<Vec<u8>>, bytes: &[u8]) {
    let mut buffer = lock(history);
    buffer.extend_from_slice(bytes);
    if buffer.len() <= HISTORY_LIMIT {
        return;
    }
    let mut drain_to = buffer.len() - HISTORY_LIMIT;
    while drain_to < buffer.len() && (buffer[drain_to] & 0xC0) == 0x80 {
        drain_to += 1;
    }
    buffer.drain(..drain_to);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    struct Collector {
        data: Mutex<Vec<String>>,
        exit: Mutex<Vec<i64>>,
        seen_data: AtomicBool,
        t0: std::time::Instant,
    }

    impl Collector {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                data: Mutex::new(Vec::new()),
                exit: Mutex::new(Vec::new()),
                seen_data: AtomicBool::new(false),
                t0: std::time::Instant::now(),
            })
        }

        fn data_text(&self) -> String {
            lock(&self.data).join("")
        }

        fn exits(&self) -> Vec<i64> {
            lock(&self.exit).clone()
        }
    }

    fn collector_emit(collector: &Arc<Collector>) -> TerminalEmit {
        let collector = Arc::clone(collector);
        Arc::new(move |event| match event {
            TerminalEvent::Data { data, .. } => {
                eprintln!("[chunk @{:?}] {:?}", collector.t0.elapsed(), data);
                collector.seen_data.store(true, Ordering::SeqCst);
                lock(&collector.data).push(data);
            }
            TerminalEvent::Exit { code, .. } => {
                lock(&collector.exit).push(code);
            }
        })
    }

    fn wait_for(timeout_ms: u64, mut predicate: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        while Instant::now() < deadline {
            if predicate() {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        predicate()
    }

    #[cfg(windows)]
    const TEST_SHELL: &str = "cmd.exe";
    #[cfg(not(windows))]
    const TEST_SHELL: &str = "/bin/sh";

    #[test]
    fn utf8_split_across_reads_is_decoded_once() {
        let collected: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let emit: TerminalEmit = {
            let collected = Arc::clone(&collected);
            Arc::new(move |event| match event {
                TerminalEvent::Data { data, .. } => lock(&collected).push(data),
                TerminalEvent::Exit { .. } => {}
            })
        };
        let good = "好".as_bytes();
        // 「好」= 3 字节：前 2 字节一批、第 3 字节一批，跨块必须只解码出一次。
        let writer = Arc::new(Mutex::new(Box::new(Vec::new()) as Box<dyn Write + Send>));
        let history = Arc::new(Mutex::new(Vec::new()));
        pump_output(
            "t".to_string(),
            &good[..2] as &[u8],
            Arc::clone(&emit),
            Arc::clone(&writer),
            Arc::clone(&history),
        );
        pump_output("t".to_string(), good, emit, writer, Arc::clone(&history));
        let text = lock(&collected).join("");
        assert_eq!(text, "好");
        assert_eq!(std::str::from_utf8(&lock(&history)).unwrap(), "好");
    }

    #[test]
    fn key_index_removes_by_id_and_ignores_stale_keys() {
        let mut index = KeyIndex::default();
        index.insert("side:1", "term-1");
        assert_eq!(index.get("side:1"), Some("term-1"));
        index.remove_id("term-1");
        assert_eq!(index.get("side:1"), None);
        index.remove_id("term-1");
        assert_eq!(index.get("side:1"), None);
    }

    #[test]
    fn history_is_bounded_and_keeps_utf8_boundary() {
        let history = Mutex::new(Vec::new());
        append_history(&history, "好".repeat(HISTORY_LIMIT / 3 + 10).as_bytes());
        let buffer = lock(&history);
        assert!(buffer.len() <= HISTORY_LIMIT + 4);
        assert!(std::str::from_utf8(&buffer).is_ok());
    }

    #[test]
    fn spawn_write_read_exit_round_trip() {
        let collector = Collector::new();
        let manager = Arc::new(TerminalSessionManager::new(collector_emit(&collector)));
        let id = manager
            .create(80, 24, None, TEST_SHELL)
            .expect("pty create");

        // 等 shell 提示符就绪（首帧输出）再写命令。
        assert!(
            wait_for(15_000, || collector.seen_data.load(Ordering::SeqCst)),
            "15s 内应收到首帧输出"
        );
        #[cfg(windows)]
        let command = "echo __PTI__\r\nexit\r\n";
        #[cfg(not(windows))]
        let command = "echo __PTI__\nexit\n";
        manager.write(&id, command).expect("write");

        assert!(
            wait_for(15_000, || collector.data_text().contains("__PTI__")),
            "输出应包含命令回显；实际：{}",
            collector.data_text()
        );
        assert!(
            wait_for(15_000, || !collector.exits().is_empty()),
            "exit 后应收到退出事件"
        );
        // 会话已随退出移除：再次 write 报 not found。
        assert!(manager.write(&id, "x").is_err());
    }

    #[test]
    fn create_with_key_reattaches_same_session() {
        let collector = Collector::new();
        let manager = Arc::new(TerminalSessionManager::new(collector_emit(&collector)));

        let first = manager
            .create_with_key(80, 24, None, TEST_SHELL, Some("bottom:test:1"))
            .expect("first create");
        assert!(!first.reattached);
        assert!(first.replay.is_empty());

        let second = manager
            .create_with_key(80, 24, None, TEST_SHELL, Some("bottom:test:1"))
            .expect("reattach");
        assert!(second.reattached);
        assert_eq!(second.id, first.id);

        let other = manager
            .create_with_key(80, 24, None, TEST_SHELL, Some("bottom:test:2"))
            .expect("different key");
        assert!(!other.reattached);
        assert_ne!(other.id, first.id);

        manager.dispose(&first.id).expect("dispose first");
        manager.dispose(&other.id).expect("dispose other");
    }

    #[test]
    fn dispose_by_key_kills_session_and_allows_new_create() {
        let collector = Collector::new();
        let manager = Arc::new(TerminalSessionManager::new(collector_emit(&collector)));

        let first = manager
            .create_with_key(80, 24, None, TEST_SHELL, Some("bottom:test:1"))
            .expect("first create");
        assert_eq!(
            manager.dispose_by_key("bottom:test:1").expect("dispose by key"),
            Some(first.id.clone())
        );
        // 幂等：重复释放不报错，且不再返回 id。
        assert_eq!(
            manager.dispose_by_key("bottom:test:1").expect("second dispose"),
            None
        );

        let second = manager
            .create_with_key(80, 24, None, TEST_SHELL, Some("bottom:test:1"))
            .expect("create after dispose");
        assert!(!second.reattached);
        assert_ne!(second.id, first.id);

        manager.dispose(&second.id).expect("dispose second");
    }

    #[test]
    fn resize_and_dispose_are_safe() {
        let collector = Collector::new();
        let manager = Arc::new(TerminalSessionManager::new(collector_emit(&collector)));
        let id = manager.create(80, 24, None, TEST_SHELL).expect("pty create");
        manager.resize(&id, 100, 30).expect("resize");
        manager.dispose(&id).expect("dispose");
        // 幂等：再 dispose 不报错。
        manager.dispose(&id).expect("dispose twice");
        assert!(wait_for(15_000, || !collector.exits().is_empty()));
    }
}
