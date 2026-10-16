//! Host sidecar 的 spawn、监督与字节传输（纯 std，不依赖 Tauri，便于直接跑用例）。
//!
//! 生命周期归壳（DR-T1-3），这里有三个参与者，各管一件事：
//!
//! | 线程 | 观察 | 动作 |
//! | --- | --- | --- |
//! | reader | Host 的 stdout 帧 | 入队；**第一帧 = 就绪**（`ChannelServer` 的 `Initialize`） |
//! | reaper | 子进程退出 | 收尸；非主动退出即发布 `failed` + `errorCode` |
//! | watchdog | 就绪期限 | 超期未拿到第一帧即发布 `failed(startup_status_timeout)` 并杀掉子进程 |
//!
//! 三者用一个 `lifecycle` 原子量决定**谁**有资格发布终态（CAS 单赢家），因此不会出现
//! 「先 failed 后 ready」这种自相矛盾的流。退出清理用 `taskkill /T`（杀整棵进程树，Task 2 的
//! Host 会带 `node-pty` 子进程），失败再退回直接 kill；`shutdown` 幂等，可重复调用。

use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use crate::frame::{read_frame, write_frame};
use crate::startup::{
    StartupErrorCode, StartupMonitor, StartupPhase, StartupSnapshot,
};

/// 入方向（Host → 页面）队列的默认容量（帧数）。满了读线程阻塞 = 真背压，
/// 不是丢帧：`ChannelServer` 的应答丢了就会变成 RPC 超时。
const DEFAULT_INBOUND_CAPACITY: usize = 256;

/// 就绪期限的默认值：渲染层的失败屏在 40s，这里必须显著更早，好让渲染层拿到**原因**而不是超时。
pub const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(20);

/// 起 Host 需要的全部输入。`entry = None` 表示「只跑 `node_args`」——留给监督用例
/// 起一个可控的沉默子进程。
#[derive(Clone, Debug)]
pub struct HostConfig {
    /// Node 可执行文件（打包形态会换成随包的那个）。
    pub node: PathBuf,
    /// 传给 node 的额外参数（`.ts` 入口会自动补 `--import tsx`）。
    pub node_args: Vec<String>,
    /// 入口脚本路径。
    pub entry: Option<PathBuf>,
    /// 子进程工作目录（`--import tsx` 之类的裸模块解析从这里开始）。
    pub cwd: PathBuf,
    pub startup_id: String,
    pub attempt_id: String,
    /// 数据根：通过 `MODE_DATA_BASE_DIR` 传给 Host（隔离口径见 `docs/specs/tauri-renderer-host.md`）。
    pub data_root: PathBuf,
    pub ready_timeout: Duration,
    pub inbound_capacity: usize,
}

impl HostConfig {
    /// 按入口扩展名补 node 参数：`.ts` / `.mts` 需要 `tsx` 才能跑。
    pub fn for_sidecar_entry(entry: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        let entry = entry.into();
        let mut node_args = Vec::new();
        if matches!(
            entry.extension().and_then(|ext| ext.to_str()),
            Some("ts") | Some("mts")
        ) {
            node_args.push("--import".to_string());
            node_args.push("tsx".to_string());
        }
        Self {
            node: PathBuf::from("node"),
            node_args,
            entry: Some(entry),
            cwd: cwd.into(),
            startup_id: String::new(),
            attempt_id: String::new(),
            data_root: PathBuf::new(),
            ready_timeout: DEFAULT_READY_TIMEOUT,
            inbound_capacity: DEFAULT_INBOUND_CAPACITY,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.node);
        command.args(&self.node_args);
        if let Some(entry) = &self.entry {
            command.arg(entry);
        }
        command
    }
}

#[derive(Debug)]
pub enum HostError {
    /// 拉不起进程（node 或入口缺失 / 不可执行）。
    Spawn(std::io::Error),
    /// 写不进去（Host 已死或管道坏了）。
    Write(std::io::Error),
    /// 传输已关闭且队列已排空。
    TransportClosed,
    /// 配置缺输入（例如没有入口也没有 node 参数）。
    InvalidConfig(String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(error) => write!(formatter, "拉起 Host 失败：{error}"),
            Self::Write(error) => write!(formatter, "写 Host 帧失败：{error}"),
            Self::TransportClosed => write!(formatter, "Host 传输已关闭"),
            Self::InvalidConfig(reason) => write!(formatter, "Host 配置非法：{reason}"),
        }
    }
}

impl std::error::Error for HostError {}

impl HostError {
    /// 与 `databaseStartupErrorCodeSchema` 对齐的失败码（失败推给渲染层时用它）。
    pub fn startup_error_code(&self) -> StartupErrorCode {
        match self {
            Self::Spawn(_) => StartupErrorCode::OpenFailed,
            Self::InvalidConfig(_) => StartupErrorCode::UnsupportedRuntime,
            Self::Write(_) | Self::TransportClosed => StartupErrorCode::TransportClosed,
        }
    }
}

/// 生命周期单赢家：只有它决定终态是谁先发布的。
const LIFECYCLE_STARTING: u8 = 0;
const LIFECYCLE_READY: u8 = 1;
const LIFECYCLE_FAILED: u8 = 2;

struct Inbound {
    frames: std::collections::VecDeque<Vec<u8>>,
    closed: bool,
}

struct HostInner {
    startup_id: String,
    lifecycle: AtomicU8,
    shutting_down: AtomicBool,
    pid: u32,
    stdin: Mutex<Option<ChildStdin>>,
    inbound: Mutex<Inbound>,
    inbound_ready: Condvar,
    inbound_space: Condvar,
    startup: StartupMonitor,
    capacity: usize,
}

/// 壳持有的句柄：可克隆（Tauri 的 `State` 需要共享），生命周期由 Rust 壳负责。
///
/// `Debug` 手写而不派生：`HostInner` 里有 `ChildStdin` 与 `Condvar`，派生会把一堆实现细节
/// 塞进诊断输出；这里只暴露定位需要的三件事。
#[derive(Clone)]
pub struct HostHandle {
    inner: Arc<HostInner>,
}

impl std::fmt::Debug for HostHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostHandle")
            .field("startup_id", &self.inner.startup_id)
            .field("pid", &self.inner.pid)
            .field("phase", &self.inner.startup.snapshot().phase)
            .finish()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // 锁中毒只可能是某个线程 panic；此时宁可继续用可能有偏差的数据，也不能让壳二次 panic。
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl HostHandle {
    /// 起 Host。失败**不**在这里吞掉——调用方（壳）负责把它转成推给渲染层的 `failed` 状态。
    pub fn spawn(config: HostConfig) -> Result<Self, HostError> {
        if config.entry.is_none() && config.node_args.is_empty() {
            return Err(HostError::InvalidConfig(
                "既没有入口脚本也没有 node 参数".to_string(),
            ));
        }
        let mut command = config.command();
        command
            .current_dir(&config.cwd)
            .env("MODE_HOST_STARTUP_ID", &config.startup_id)
            .env("MODE_DATA_BASE_DIR", &config.data_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Host 的日志走壳的 stderr：本地跑壳时直接看得见（帧只在 stdout 上走）。
            .stderr(Stdio::inherit());

        let mut child = command.spawn().map_err(HostError::Spawn)?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout 是 piped");

        let inner = Arc::new(HostInner {
            startup_id: config.startup_id.clone(),
            lifecycle: AtomicU8::new(LIFECYCLE_STARTING),
            shutting_down: AtomicBool::new(false),
            pid: child.id(),
            stdin: Mutex::new(stdin),
            inbound: Mutex::new(Inbound {
                frames: std::collections::VecDeque::new(),
                closed: false,
            }),
            inbound_ready: Condvar::new(),
            inbound_space: Condvar::new(),
            startup: StartupMonitor::new(config.startup_id.clone(), config.attempt_id.clone()),
            capacity: config.inbound_capacity.max(1),
        });

        let reader_inner = inner.clone();
        thread::spawn(move || read_loop(reader_inner, BufReader::new(stdout)));

        let reaper_inner = inner.clone();
        thread::spawn(move || reap_loop(reaper_inner, child));

        let watchdog_inner = inner.clone();
        let ready_timeout = config.ready_timeout;
        thread::spawn(move || {
            thread::sleep(ready_timeout);
            if watchdog_inner
                .lifecycle
                .compare_exchange(
                    LIFECYCLE_STARTING,
                    LIFECYCLE_FAILED,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_err()
            {
                return; // 已经就绪或已失败：看门狗让位
            }
            watchdog_inner.startup.publish(
                StartupPhase::Failed,
                Some(StartupErrorCode::StartupStatusTimeout),
                Some(StartupPhase::Starting),
            );
            mark_closed(&watchdog_inner);
            // 沉默的 Host 没有任何价值，留着只会变成孤儿；这里直接收摊。
            kill_tree_by_pid(watchdog_inner.pid);
        });

        Ok(Self { inner })
    }

    /// 起不来时的句柄：**仍然要能被查询**，否则渲染层拿不到 `failed` 只能等超时（DR-T1-3）。
    pub fn failed(config: &HostConfig, error: &HostError) -> Self {
        let inner = Arc::new(HostInner {
            startup_id: config.startup_id.clone(),
            lifecycle: AtomicU8::new(LIFECYCLE_FAILED),
            shutting_down: AtomicBool::new(true),
            pid: 0,
            stdin: Mutex::new(None),
            inbound: Mutex::new(Inbound {
                frames: std::collections::VecDeque::new(),
                closed: true,
            }),
            inbound_ready: Condvar::new(),
            inbound_space: Condvar::new(),
            startup: StartupMonitor::new(config.startup_id.clone(), config.attempt_id.clone()),
            capacity: 1,
        });
        inner.startup.publish(
            StartupPhase::Failed,
            Some(error.startup_error_code()),
            Some(StartupPhase::Starting),
        );
        Self { inner }
    }

    pub fn startup_id(&self) -> &str {
        &self.inner.startup_id
    }

    pub fn pid(&self) -> u32 {
        self.inner.pid
    }

    pub fn startup_snapshot(&self) -> StartupSnapshot {
        self.inner.startup.snapshot()
    }

    /// 长轮询状态变更（渲染层的状态流就靠它推；`since` 传上次拿到的 `sequence`）。
    pub fn wait_for_startup_change(&self, since: Option<u32>, timeout: Duration) -> StartupSnapshot {
        self.inner.startup.wait_for_change(since, timeout)
    }

    /// 页面 → Host：整帧写进 stdin。
    pub fn send_frame(&self, payload: &[u8]) -> Result<(), HostError> {
        let mut guard = lock(&self.inner.stdin);
        let stdin = guard.as_mut().ok_or(HostError::TransportClosed)?;
        write_frame(stdin, payload).map_err(HostError::Write)
    }

    /// Host → 页面：等一帧。`Ok(None)` = 期限内没有新帧（页面继续轮询）；
    /// `Err(TransportClosed)` = Host 没了且队列已排空。
    pub fn next_frame(&self, timeout: Duration) -> Result<Option<Vec<u8>>, HostError> {
        let mut queue = lock(&self.inner.inbound);
        loop {
            if let Some(frame) = queue.frames.pop_front() {
                drop(queue);
                self.inner.inbound_space.notify_all();
                return Ok(Some(frame));
            }
            if queue.closed {
                return Err(HostError::TransportClosed);
            }
            let (waited, wait_result) = self
                .inner
                .inbound_ready
                .wait_timeout(queue, timeout)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            queue = waited;
            if queue.frames.is_empty() && !queue.closed && wait_result.timed_out() {
                return Ok(None);
            }
        }
    }

    /// 退出清理：关 stdin → 杀整棵进程树 → 收尸。**幂等**，可重复调用。
    pub fn shutdown(&self) -> Result<(), HostError> {
        self.inner.shutting_down.store(true, Ordering::SeqCst);
        // 先杀树、再关 stdin：反过来的话 Host 会因为 stdin EOF 先自己退出，
        // 于是 taskkill 作用在一个已死的 pid 上、报「未成功」，把真正的失败信号淹掉
        // （2026-10-16 的 opt-in 实测就出现过这条误导性日志）。
        if self.inner.pid != 0 {
            kill_tree_by_pid(self.inner.pid);
        }
        // 关 stdin 是第二道保险：Host 自带 `stdin end` 退出保护，即使杀树失败也不会变孤儿。
        lock(&self.inner.stdin).take();
        mark_closed(&self.inner);
        Ok(())
    }

    /// 进程是否还活着（用例断言用；`try_wait` 需要 `&mut Child`，这里按 pid 查）。
    pub fn is_running(&self) -> bool {
        self.inner.pid != 0 && process_exists(self.inner.pid)
    }
}

fn read_loop(inner: Arc<HostInner>, mut stdout: BufReader<impl Read>) {
    loop {
        match read_frame(&mut stdout) {
            Ok(Some(frame)) => {
                if inner
                    .lifecycle
                    .compare_exchange(
                        LIFECYCLE_STARTING,
                        LIFECYCLE_READY,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    )
                    .is_ok()
                {
                    // 第一帧就是 `ChannelServer` 的 Initialize：Host 的服务端已经起来了。
                    inner.startup.publish(StartupPhase::Ready, None, None);
                }
                push_inbound(&inner, frame);
            }
            Ok(None) => {
                fail_if_not_shutting_down(&inner, StartupErrorCode::TransportClosed, "host_stdout_eof");
                mark_closed(&inner);
                return;
            }
            Err(_) => {
                fail_if_not_shutting_down(&inner, StartupErrorCode::TransportClosed, "host_stdout_error");
                mark_closed(&inner);
                return;
            }
        }
    }
}

fn reap_loop(inner: Arc<HostInner>, mut child: Child) {
    let status = child.wait();
    let exiting = inner.shutting_down.load(Ordering::SeqCst);
    if exiting {
        // 主动收摊：不推 failed（否则渲染层会把一次正常退出读成故障）。
        mark_closed(&inner);
        return;
    }
    let reason = match status {
        Ok(status) => format!("host_exit_code={}", status.code().unwrap_or(-1)),
        Err(error) => format!("host_wait_failed={error}"),
    };
    fail_if_not_shutting_down(&inner, StartupErrorCode::TransportClosed, &reason);
    mark_closed(&inner);
}

/// 只有「还没就绪」或「已经就绪」的 Host 会被判失败；已失败的不重复发布（单赢家）。
fn fail_if_not_shutting_down(inner: &HostInner, code: StartupErrorCode, reason: &str) {
    if inner.shutting_down.load(Ordering::SeqCst) {
        return;
    }
    let current = inner.lifecycle.load(Ordering::SeqCst);
    if current == LIFECYCLE_FAILED {
        return;
    }
    if inner
        .lifecycle
        .compare_exchange(current, LIFECYCLE_FAILED, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    eprintln!("[sidecar-host] host 停止：{reason}");
    inner.startup.publish(
        StartupPhase::Failed,
        Some(code),
        Some(StartupPhase::Starting),
    );
}

fn push_inbound(inner: &HostInner, frame: Vec<u8>) {
    let mut queue = lock(&inner.inbound);
    while queue.frames.len() >= inner.capacity && !queue.closed {
        let (waited, _) = inner
            .inbound_space
            .wait_timeout(queue, Duration::from_millis(200))
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        queue = waited;
    }
    if queue.closed {
        return;
    }
    queue.frames.push_back(frame);
    drop(queue);
    inner.inbound_ready.notify_all();
}

fn mark_closed(inner: &HostInner) {
    lock(&inner.inbound).closed = true;
    inner.inbound_ready.notify_all();
    inner.inbound_space.notify_all();
}

/// 杀整棵进程树：Task 2 的 Host 会带 `node-pty` 之类的子进程，只杀直接子进程会留孤儿。
/// `taskkill` 不可用（非 Windows 的开发环境）时退回「关 stdin 后等它自己退」+ 直接 kill。
fn kill_tree_by_pid(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    // 判据不看 taskkill 的退出码（进程早就不在时它也会报失败），而是看进程表：
    // 先给它一点收摊时间，仍在才报警——这条日志必须只在**真失败**时出现。
    for _ in 0..5 {
        if !process_exists(pid) {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    eprintln!("[sidecar-host] taskkill 未成功（pid {pid}），依赖 Host 的 stdin EOF 退出保护");
}

/// 进程是否还在（Windows 上用 `tasklist` 查；用于「不留孤儿」的断言与诊断）。
pub fn process_exists(pid: u32) -> bool {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output();
    match output {
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stdout);
            text.contains(&format!("\"{pid}\"")) || text.contains(&format!(",{pid},"))
        }
        // 查不到工具链时保守地说「还在」：宁可让用例失败，也不谎报「没有孤儿」。
        Err(_) => true,
    }
}

/// 测试与诊断用的辅助：等 Host 到某个阶段（默认就绪）。
pub fn wait_for_phase(handle: &HostHandle, phase: StartupPhase, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    let mut since = None;
    loop {
        let snapshot = handle.wait_for_startup_change(since, Duration::from_millis(50));
        if snapshot.phase == phase {
            return true;
        }
        if snapshot.phase == StartupPhase::Failed {
            return false;
        }
        since = Some(snapshot.sequence);
        if std::time::Instant::now() >= deadline {
            return false;
        }
    }
}

/// 便捷构造：把 `entry` 换成一个「沉默但活着」的 node 进程（监督用例用）。
pub fn silent_config(script: &str) -> HostConfig {
    HostConfig {
        node: PathBuf::from("node"),
        node_args: vec!["--eval".to_string(), script.to_string()],
        entry: None,
        cwd: Path::new(".").to_path_buf(),
        startup_id: "silent-startup".to_string(),
        attempt_id: "silent-attempt".to_string(),
        data_root: PathBuf::from("."),
        ready_timeout: DEFAULT_READY_TIMEOUT,
        inbound_capacity: DEFAULT_INBOUND_CAPACITY,
    }
}
