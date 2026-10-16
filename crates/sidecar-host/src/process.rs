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
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use crate::frame::{read_frame, write_frame};
use crate::host_stderr::{StartupFailureFacts, StderrMonitor};
use crate::startup::{
    StartupErrorCode, StartupMonitor, StartupPhase, StartupSnapshot, StartupStage,
};

/// 入方向（Host → 页面）队列的默认容量（帧数）。满了读线程阻塞 = 真背压，
/// 不是丢帧：`ChannelServer` 的应答丢了就会变成 RPC 超时。
const DEFAULT_INBOUND_CAPACITY: usize = 256;

/// 就绪期限的默认值。
///
/// 渲染层「一条状态都没收到」的失败屏在 30s（`renderer/src/main.tsx`），所以这里必须显著更早，
/// 好在渲染层等待期间就把**原因**（`failed` + `errorCode`）推给它。
/// 那个 30s 计时器一收到状态就被拆除，因此投出 `starting` 之后，本期限与 `failed` 推送
/// 是渲染层唯一的失败来源（见 `startup.rs` 的模块注释）。
pub const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(20);

/// 等 Host stderr 读干（EOF）的上限。
///
/// Host 一退出，它与我们之间那根 stderr 管道就该 EOF；但这个等待是**有界**的：
/// Host 可能把 stderr 交给了后代进程（`node-pty` 起的 shell），那些后代仍握着写端，
/// 于是 EOF 会晚到甚至不来。等不到就退回粗粒度原因（如实的降级，不是假装知道）。
pub const DEFAULT_STDERR_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

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
    /// 数据**基**目录：原样通过 `MODE_DATA_BASE_DIR` 传给 Host（Host 自己派生 `{base}/.mode`）。
    /// 语义是「替换 homedir() 前缀」，不是「数据根」——见 `crate::data_base_dir_from` 的注释。
    pub data_base_dir: PathBuf,
    /// 打包形态的**资源目录**（经 `MODE_HOST_RESOURCES_DIR` 传给 Host）。
    /// `None` = 不设这条环境变量，Host 侧退回「仓库相对」那一档（开发形态）。
    ///
    /// 为什么由壳设：Host 按它解析 `config/provider/mode-builtin.json`
    /// （`packages/desktop/src/host-standalone/serviceAccessor.ts` 的 `resolveBuiltinProviderConfig`，
    /// 第二档就是这条，那里的注释写着「壳负责设置」）。不设的后果不是崩溃，而是
    /// **静默走到第三档**（仓库相对路径，安装后不存在）→ `exists=false`、内置 Provider 配置缺失。
    /// 这种「不报错的错配置」正是要靠显式传参挡住的。
    pub resources_dir: Option<PathBuf>,
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
            data_base_dir: PathBuf::new(),
            resources_dir: None,
            ready_timeout: DEFAULT_READY_TIMEOUT,
            inbound_capacity: DEFAULT_INBOUND_CAPACITY,
        }
    }

    /// 起不了 Host 时的占位配置：没有入口、也没有 node 参数，`spawn` 会以 `InvalidConfig` 失败。
    ///
    /// 存在的理由：壳在「入口解析不到」时仍需要一个**可查询**的句柄来承载失败状态
    /// （DR-T1-3：渲染层必须拿到 `failed` + `errorCode`，而不是等超时）。
    pub fn unavailable(node: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            node: node.into(),
            node_args: Vec::new(),
            entry: None,
            cwd: cwd.into(),
            startup_id: String::new(),
            attempt_id: String::new(),
            data_base_dir: PathBuf::new(),
            resources_dir: None,
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
    /// Host stderr 的观察者：留痕 + 结构化失败事实（DR-T3-3 ①）。
    stderr: Arc<StderrMonitor>,
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
        // 隔离纪律（M8）：空基目录会被当成「未设置」，让 Host 悄悄指向开发者真实的数据根。
        // 隔离必须由配置类型/校验保证，不能靠每个调用点记得设。
        if config.data_base_dir.as_os_str().is_empty() {
            return Err(HostError::InvalidConfig(
                "data_base_dir 为空：拒绝在没有显式数据基目录的情况下启动 Host（隔离纪律）".to_string(),
            ));
        }
        // **交给 Node 之前统一去掉 verbatim 前缀**（`\\?\`），唯一的一处收口。
        //
        // 为什么收在这里而不是各调用点：能产出 verbatim 路径的是 Tauri 的 `resource_dir()`
        // （内部 `canonicalize()`，见 `crate::strip_verbatim_prefix` 的文档），而入口、随包 Node、
        // 资源目录、cwd 都可能从它派生。只在壳里改一处，下一个调用点就会重新踩上；
        // 收在「Rust 路径 → 子进程 argv/env」这个边界上，就没有第二条路能把 verbatim 形态漏过去。
        //
        // 漏过去的后果是**静默的粗粒度失败**：node 在装载入口前就退出（`EISDIR: lstat 'C:'`），
        // 连 Host 自己那行 `startup_failed` 都写不出来，渲染层只看到
        // `transport_closed`/`starting`——正是 2026-10-17 用户报障的那一份 payload。
        let mut config = config;
        config.node = crate::strip_verbatim_prefix(&config.node);
        config.entry = config.entry.as_deref().map(crate::strip_verbatim_prefix);
        config.cwd = crate::strip_verbatim_prefix(&config.cwd);
        config.data_base_dir = crate::strip_verbatim_prefix(&config.data_base_dir);
        config.resources_dir = config.resources_dir.as_deref().map(crate::strip_verbatim_prefix);
        let mut command = config.command();
        command
            .current_dir(&config.cwd)
            .env("MODE_HOST_STARTUP_ID", &config.startup_id)
            .env("MODE_DATA_BASE_DIR", &config.data_base_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Host 的日志仍然要能被看见（帧只在 stdout 上走），但我们要**读**它才能拿到
            // 结构化的失败事实，所以改成管道：读线程把每一行原样转到壳的 stderr
            // （留痕不丢，本地跑壳照样看得见）并顺手解析 startup_failed。
            .stderr(Stdio::piped());
        // 打包形态的资源目录：只在有值时设（`None` = 保持开发形态那一档，
        // 不让一条空字符串把 Host 的「显式覆写优先」误触发成 `exists=false`）。
        if let Some(resources_dir) = &config.resources_dir {
            command.env("MODE_HOST_RESOURCES_DIR", resources_dir);
        }

        let mut child = command.spawn().map_err(HostError::Spawn)?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout 是 piped");
        let stderr = child.stderr.take().expect("stderr 是 piped");
        let stderr_monitor = StderrMonitor::new();
        stderr_monitor.spawn(stderr);

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
            stderr: Arc::clone(&stderr_monitor),
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
                Some(StartupStage::Starting),
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
            stderr: StderrMonitor::new(),
        });
        inner.startup.publish(
            StartupPhase::Failed,
            Some(error.startup_error_code()),
            Some(StartupStage::Starting),
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
                fail_with_host_facts(&inner, StartupErrorCode::TransportClosed, "host_stdout_eof");
                mark_closed(&inner);
                return;
            }
            Err(_) => {
                fail_with_host_facts(&inner, StartupErrorCode::TransportClosed, "host_stdout_error");
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
    fail_with_host_facts(&inner, StartupErrorCode::TransportClosed, &reason);
    mark_closed(&inner);
}

/// 发布失败**之前**先把 Host 报来的结构化失败事实收齐（DR-T3-3 ①）。
///
/// 顺序是语义，不是优化：Host 先写 stderr、再退出，而**stdout EOF 通常比 stderr 那行先到**
/// （实测：不等的话 `host_stdout_eof` 会抢在事实之前把状态发布成 `transport_closed`/`starting`）。
/// 等待是**有界**的（[`DEFAULT_STDERR_DRAIN_TIMEOUT`]）：Host 可能把 stderr 交给了后代进程，
/// EOF 会晚到甚至不来，那时如实退回粗粒度原因，不假装知道。
fn fail_with_host_facts(inner: &HostInner, code: StartupErrorCode, reason: &str) {
    if !inner.shutting_down.load(Ordering::SeqCst)
        && !inner.stderr.wait_drained(DEFAULT_STDERR_DRAIN_TIMEOUT)
    {
        eprintln!(
            "[sidecar-host] stderr 未在 {:?} 内读干，失败原因退回粗粒度（Host 可能把 stderr 交给了后代进程）",
            DEFAULT_STDERR_DRAIN_TIMEOUT
        );
    }
    fail_if_not_shutting_down(inner, code, reason);
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
    // Host 报过结构化失败事实时**用它**：粗粒度码只是壳能自己判断的兜底
    // （DR-T3-3 ①：渲染层看到的原因要与 Host 知道的同精度）。两个字段各自独立降级：
    // 只认出错码就只换错码，阶段仍旧报壳能判断的那一档。
    let facts = inner.stderr.facts().unwrap_or(StartupFailureFacts {
        failed_phase: None,
        error_code: None,
    });
    inner.startup.publish(
        StartupPhase::Failed,
        Some(facts.error_code.unwrap_or(code)),
        Some(facts.failed_phase.unwrap_or(StartupStage::Starting)),
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
