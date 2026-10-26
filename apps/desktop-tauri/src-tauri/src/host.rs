//! 壳侧的 Host 接线：把 Tauri 的事件面接到 `crates/sidecar-host` 的框架无关 API 上。
//!
//! 这里只有三类东西，没有可测的业务逻辑（可测的部分在 `crates/sidecar-host`，那边能真跑用例）：
//!
//! 1. [`resolve_config`] / [`spawn_host`]：把 `AppHandle` 上的路径与进程环境收成一份 [`HostConfig`]；
//! 2. 三个 `#[tauri::command]`：端口字节泵（raw 帧）+ 启动状态长轮询；
//! 3. [`handler`]：把这批命令交给壳，与 M1 的平台命令面并进**同一个** `invoke_handler`。
//!
//! ## 为什么命令名写死成字面量
//!
//! `#[tauri::command(rename = …)]` 的参数必须是字面量，而注入脚本里的命令名来自
//! `sidecar_host::bridge::COMMAND_NAMES`。两处一致性由
//! `packages/desktop/tests/host-standalone.test.mts` 扫这两个源文件比对（改名即红）——
//! 与 M1 的 `platform_commands` 守卫同一种做法。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use sidecar_host::bridge::{
    CMD_PLATFORM_REPORT, CMD_RECV, CMD_SEND, CMD_STARTUP_CONTROL, CMD_STARTUP_ID, CMD_STATE,
};
use sidecar_host::respawn::{RespawnHistory, MAX_QUICK_CRASHES};
use sidecar_host::{data_base_dir_from, HostConfig, HostHandle};
use tauri::{AppHandle, Manager, Runtime, State};

/// 入方向长轮询的单次上限：到点回空帧，页面立刻再轮询（不把一次 invoke 挂太久）。
const RECV_TIMEOUT: Duration = Duration::from_secs(10);
/// 状态长轮询的单次上限：有变更立刻返回，没有就等到这里再返回当前快照。
const STATE_TIMEOUT: Duration = Duration::from_secs(5);

/// 本次 Host 实例的启动代 id（`[A-Za-z0-9_-]`，可安全注入脚本）。
fn new_startup_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    format!("host-{}-{}", std::process::id(), millis)
}

/// 仓库根（仅 debug 构建：`apps/desktop-tauri/src-tauri` 上溯三级）。
#[cfg(debug_assertions)]
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// 入口脚本解析顺序：`MODE_HOST_SIDECAR_ENTRY` → 应用资源目录 → （debug 构建）仓库里的源码入口。
///
/// 不硬编码绝对路径：资源目录那档是打包形态（Task 3/5 把 sidecar 落进去），
/// 仓库相对那档只在 debug 构建存在，纯粹是开发便利（release 下编译器不会把它编进去）。
fn resolve_entry<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    // 候选枚举需要 AppHandle（资源目录）；**挑选规则**本身在 `sidecar_host::pick_entry` 里，
    // 那部分是纯函数、有单测（本文件整体没有自动化覆盖，见 spec 的覆盖分工）。
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(resource_dir) = app.path().resource_dir() {
        for relative in ["host-sidecar/index.mjs", "host-sidecar/index.ts"] {
            candidates.push(resource_dir.join(relative));
        }
    }
    #[cfg(debug_assertions)]
    candidates.push(repo_root().join("packages/desktop/src/host-standalone/index.ts"));

    sidecar_host::pick_entry(
        std::env::var_os("MODE_HOST_SIDECAR_ENTRY").as_deref(),
        &candidates,
    )
}

/// 开发形态下的默认工作目录：仓库根（`--import tsx` 的裸模块解析从这里开始）。
#[cfg(debug_assertions)]
fn default_sidecar_cwd() -> Option<PathBuf> {
    Some(repo_root())
}

#[cfg(not(debug_assertions))]
fn default_sidecar_cwd() -> Option<PathBuf> {
    None
}

/// 起 Host 需要的全部配置（路径、隔离与 startupId 都在这里收敛）。
pub fn resolve_config<R: Runtime>(app: &AppHandle<R>) -> HostConfig {
    let startup_id = new_startup_id();
    let cwd = std::env::var_os("MODE_HOST_SIDECAR_CWD")
        .map(PathBuf::from)
        .or_else(default_sidecar_cwd)
        .unwrap_or_else(|| Path::new(".").to_path_buf());

    // 资源目录：打包形态的落点（Task 5 把 `host-sidecar/`、`node/`、`config/` 放进安装包的资源目录）。
    // 开发形态下 `resource_dir()` 仍然返回一个真实目录（target/debug），所以它**不是**
    // 「有没有打包」的判据——判据是候选文件在不在（见 `pick_entry` / `pick_node`）。
    let resource_dir = app.path().resource_dir().ok();
    // Node 可执行文件的挑选（优先级与理由见 `sidecar_host::pick_node`）：
    // 显式覆写 → 随包 Node（`<resource>/node/node.exe`）→ PATH 上的 `node`。
    let node_override = std::env::var_os("MODE_HOST_NODE");
    let bundled_node: Vec<PathBuf> = resource_dir
        .as_ref()
        .map(|dir| {
            ["node/node.exe", "node/node"]
                .into_iter()
                .map(|relative| dir.join(relative))
                .collect()
        })
        .unwrap_or_default();
    let node = sidecar_host::pick_node(node_override.as_deref(), &bundled_node);

    let mut config = match resolve_entry(app) {
        Some(entry) => HostConfig::for_sidecar_entry(entry, cwd),
        // 没有入口也别在这里失败：交给 `spawn_host` 转成推给渲染层的 failed 状态。
        None => HostConfig::unavailable(node.clone(), cwd),
    };
    config.node = node;
    // Host 按它解析 `config/provider/mode-builtin.json`（host-standalone 的
    // `resolveBuiltinProviderConfig` 第二档）。传空目录没有意义，所以只在真有资源目录时传。
    config.resources_dir = resource_dir;
    // 统一前端构建：内嵌资源给 Tauri 窗口用，同时解压一份给 Host 的 LAN Web 服务。
    // 解压失败只影响浏览器入口，桌面窗口仍走内嵌资源。
    config.web_static_root = crate::web_assets::extract(app);
    config.startup_id = startup_id.clone();
    config.attempt_id = format!("{startup_id}-a1");
    // `MODE_DATA_BASE_DIR` 是**基**目录：原样透传给子进程，由 Host 派生 `{base}/.mode`
    // （口径见 sidecar_host::data_base_dir_from 与 docs/specs/tauri-renderer-host.md §4）。
    config.data_base_dir = {
        let home = app.path().home_dir().unwrap_or_else(|_| PathBuf::from("."));
        data_base_dir_from(std::env::var_os("MODE_DATA_BASE_DIR").as_deref(), &home)
    };
    config
}

/// 起一代 Host。**不返回 `Result`**：起不来时返回一个「已失败」的句柄，渲染层照样能拿到
/// `phase: "failed"` + `errorCode`（DR-T1-3：失败必须被推给渲染层，而不是让壳先死）。
fn spawn_or_failed(config: &HostConfig) -> HostHandle {
    match HostHandle::spawn(config.clone()) {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("[mode-tauri] 起 Host 失败：{error}（渲染层会收到 failed 状态）");
            HostHandle::failed(config, &error)
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // 锁中毒只可能是某个线程 panic；继续用可能有偏差的数据，也别让壳二次 panic。
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 起第一代 Host 并返回带监督线程的管理器（壳装配用；`main.rs` 与往返 harness 共用）。
pub fn start<R: Runtime>(app: &AppHandle<R>) -> Arc<HostSupervisor> {
    HostSupervisor::start(app)
}

/// 一个窗口的 Host 代际管理者：命令面只读 [`HostSupervisor::current`]，监督线程在**进程死亡**
/// （不是渲染层收到 failed）后按退避策略换代（spec `tauri-renderer-host.md` §22.4）。
///
/// 为什么需要它而不是直接托管 `HostHandle`：Tauri 的托管状态不可替换，而自动重生必须换句柄；
/// 另外「同一个代只计一次死亡」「连续快速退出后放弃」「人工重试」都需要一块壳侧状态。
pub struct HostSupervisor {
    current: Mutex<HostHandle>,
    state: Mutex<SupervisorState>,
    wake: Condvar,
    shutting_down: AtomicBool,
}

struct SupervisorState {
    history: RespawnHistory,
    /// 已经处理过死亡的代：同一个代只计一次，重生换代后清空。
    handled_death_for: Option<String>,
    /// 人工重试请求（终态屏 retry）：唤醒可能停在「放弃后等待」的监督线程。
    retry_requested: bool,
}

impl HostSupervisor {
    fn new(handle: HostHandle) -> Self {
        let mut state = SupervisorState {
            history: RespawnHistory::default(),
            handled_death_for: None,
            retry_requested: false,
        };
        state.history.record_spawn(Instant::now());
        Self {
            current: Mutex::new(handle),
            state: Mutex::new(state),
            wake: Condvar::new(),
            shutting_down: AtomicBool::new(false),
        }
    }

    /// 起第一代并启动监督线程（返回 `Arc`：命令面与线程共享同一份）。
    pub fn start<R: Runtime>(app: &AppHandle<R>) -> Arc<Self> {
        let config = resolve_config(app);
        let supervisor = Arc::new(Self::new(spawn_or_failed(&config)));
        let watcher = Arc::clone(&supervisor);
        let watcher_app = app.clone();
        thread::spawn(move || watch_loop(watcher_app, watcher));
        supervisor
    }

    /// 当前代的句柄快照（命令面每次调用都取最新一代）。
    pub fn current(&self) -> HostHandle {
        lock(&self.current).clone()
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    /// 壳退出：标记停止并收掉当前代（幂等）。监督线程醒来后自行退出，不再重生。
    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        self.wake.notify_all();
        if let Err(error) = self.current().shutdown() {
            eprintln!("[mode-tauri] 收 Host 失败：{error}");
        }
    }

    /// 杀掉当前代但**不**标记壳退出（心跳判定传输停摆后的 kill 动作）：
    /// 监督线程看到进程退出，按一次崩溃计账并走重生换代。
    pub fn kill_current(&self) {
        if let Err(error) = self.current().shutdown() {
            eprintln!("[mode-tauri] kill Host 失败：{error}");
        }
    }

    /// 终态屏「重试」：给一整轮新的重试预算。
    ///
    /// 进程还活着时只刷新页面（用户的「重试」针对的是渲染层状态，杀 Host 会打断任务）；
    /// 已经死亡时置 `retry_requested` 唤醒监督线程，由它重生并 reload。
    pub fn force_restart<R: Runtime>(&self, app: &AppHandle<R>) {
        if self.is_shutting_down() {
            return;
        }
        let running = self.current().is_running();
        {
            let mut state = lock(&self.state);
            state.history.reset();
            if !running {
                state.retry_requested = true;
            }
        }
        if running {
            reload_main_window(app);
        } else {
            self.wake.notify_all();
        }
    }

    /// 换代：替换句柄并清空「已处理死亡」标记（新一代的死亡要重新计）。
    fn replace(&self, handle: HostHandle) {
        *lock(&self.current) = handle;
        let mut state = lock(&self.state);
        state.handled_death_for = None;
        state.retry_requested = false;
        state.history.record_spawn(Instant::now());
    }
}

/// 监督线程：等当前代**进程退出** → 退避策略 → 重生 → 整页 reload。
///
/// 为什么要 reload：注入脚本在 document-start 拼好，桥的 startupId 是**启动时采用**的；
/// 重生换代后旧文档的桥会按换代保护拒收新代状态。reload 让新文档重新走
/// 「采用当前代 → 投端口 → 状态流」，渲染层的启动闸门与既有启动链一行都不用改。
fn watch_loop<R: Runtime>(app: AppHandle<R>, supervisor: Arc<HostSupervisor>) {
    loop {
        let handle = supervisor.current();
        handle.wait_for_exit();
        if supervisor.is_shutting_down() {
            return;
        }
        let generation = handle.startup_id().to_string();
        let mut state = lock(&supervisor.state);
        if state.handled_death_for.as_deref() == Some(generation.as_str()) {
            // 这一代已经判过（放弃自动重生，或重生被中断）：等人工重试。
            while !state.retry_requested && !supervisor.is_shutting_down() {
                state = supervisor
                    .wake
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            if supervisor.is_shutting_down() {
                return;
            }
            state.retry_requested = false;
            // 人工重试允许对同一代重新判断（预算已由 force_restart 重置）。
            state.handled_death_for = None;
            drop(state);
            continue;
        }
        state.handled_death_for = Some(generation);
        let delay = state.history.record_death(Instant::now());
        drop(state);

        let Some(delay) = delay else {
            eprintln!(
                "[mode-tauri] Host 连续 {MAX_QUICK_CRASHES} 次快速退出，暂停自动重生（启动失败屏可手动重试）"
            );
            continue; // 下一圈命中「已处理死亡」→ 等人工重试
        };
        if !delay.is_zero() {
            thread::sleep(delay);
            if supervisor.is_shutting_down() {
                return;
            }
        }
        let config = resolve_config(&app);
        let next = spawn_or_failed(&config);
        // spawn 与 shutdown 可能并发（退避睡眠的窗口）：若壳在 spawn 期间决定退出，
        // 这里必须立刻收掉刚拉起的进程，否则 RunEvent::Exit 已经过去、它会成为孤儿。
        if supervisor.is_shutting_down() {
            let _ = next.shutdown();
            return;
        }
        eprintln!(
            "[mode-tauri] Host 重生：startupId={} pid={}",
            next.startup_id(),
            next.pid()
        );
        supervisor.replace(next);
        reload_main_window(&app);
    }
}

/// 让主窗口整页重载（新一代的注入脚本会采用新 startupId）。
fn reload_main_window<R: Runtime>(app: &AppHandle<R>) {
    match app.get_webview_window(crate::MAIN_WINDOW_LABEL) {
        Some(window) => {
            if let Err(error) = window.reload() {
                eprintln!("[mode-tauri] 重载主窗口失败：{error}");
            }
        }
        None => eprintln!("[mode-tauri] 主窗口不存在，跳过重载"),
    }
}

/// 页面 → Host：整帧 raw body 写进 sidecar 的 stdin。
#[tauri::command(rename = "host_send")]
pub fn host_send(
    supervisor: State<'_, Arc<HostSupervisor>>,
    request: tauri::ipc::Request<'_>,
) -> Result<(), String> {
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes.clone(),
        other => return Err(format!("host_send 期望 raw body，实收 {other:?}")),
    };
    supervisor
        .current()
        .send_frame(&bytes)
        .map_err(|error| error.to_string())
}

/// Host → 页面：长轮询一帧，按 raw bytes 返回（页面拿到 `ArrayBuffer`，必须自己包 `Uint8Array`）。
#[tauri::command(rename = "host_recv")]
pub async fn host_recv(
    supervisor: State<'_, Arc<HostSupervisor>>,
) -> Result<tauri::ipc::Response, String> {
    let handle = supervisor.current();
    let frame = tauri::async_runtime::spawn_blocking(move || handle.next_frame(RECV_TIMEOUT))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    Ok(tauri::ipc::Response::new(frame.unwrap_or_default()))
}

/// 启动状态长轮询：返回 `databaseStartupStateSchema` 形状的最新状态（不含任何业务判断）。
#[tauri::command(rename = "host_startup_state")]
pub async fn host_startup_state(
    supervisor: State<'_, Arc<HostSupervisor>>,
    since: Option<u32>,
) -> Result<serde_json::Value, String> {
    let handle = supervisor.current();
    tauri::async_runtime::spawn_blocking(move || {
        handle
            .wait_for_startup_change(since, STATE_TIMEOUT)
            .to_state_json()
    })
    .await
    .map_err(|error| error.to_string())
}

/// 当前 Host 代 id：桥在文档启动时采用它（reload 后自动绑定新代，见 `bridge.ts` 的注释）。
#[tauri::command(rename = "host_startup_id")]
pub fn host_startup_id(supervisor: State<'_, Arc<HostSupervisor>>) -> String {
    supervisor.current().startup_id().to_string()
}

/// 终态屏控制（`mode:database-startup-control`）：旧 Electron main 的消费者已随旧线 EOL，
/// 由桥转发到这里。`retry` = 重置退避并重生（存活时只 reload）；`exit` = 退出应用；
/// `kill` = 心跳判定传输停摆：杀掉当前 Host（即使进程活着）并走重生换代；
/// `snapshot` = 无需动作（状态流本就连续，桥会直接回放最后一帧）。
#[tauri::command(rename = "host_startup_control")]
pub fn host_startup_control<R: Runtime>(
    app: AppHandle<R>,
    supervisor: State<'_, Arc<HostSupervisor>>,
    action: String,
) -> Result<(), String> {
    match action.as_str() {
        "retry" => {
            supervisor.force_restart(&app);
            Ok(())
        }
        "kill" => {
            supervisor.kill_current();
            Ok(())
        }
        "exit" => {
            app.exit(0);
            Ok(())
        }
        "snapshot" => Ok(()),
        other => Err(format!("未知的启动控制动作：{other}")),
    }
}

/// 页面上报的平台面清单（`window.mode` 的切片与降级）。字段名与
/// `crates/sidecar-host/src/window_mode.ts` 的诊断面一一对应。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformSurfaceReport {
    implemented: Vec<String>,
    not_implemented: Vec<String>,
    no_op: Vec<String>,
    deliberately_absent: Vec<String>,
    degraded: Vec<String>,
}

/// 单条清单的长度上限：这是**产品诊断面**，不是数据通道。注入层报的是自己的成员表，
/// 不设上限就等于让一条 IPC 把 stderr 刷爆（页面是本应用自己的本地内容，但边界仍要写死）。
const MAX_REPORT_ENTRIES: usize = 64;
/// 每条名字的长度上限（成员名都是标识符，超长即异常输入）。
const MAX_REPORT_NAME_LEN: usize = 128;

/// 页面上报平台面清单 → 壳把它落到**自己的 stderr**（DR-T3-3 的 I3 处置）。
///
/// **为什么需要这条命令**：注入层对「没有要读的值」的成员只能降级为空操作——**调用方收到的是
/// 成功**，而 release 里既没有 devtools 也没有 console 捕获（`tauri = "2"` 默认不启 devtools），
/// 于是那条降级在真机上不可见。这条命令把清单送进**已有的 stderr 诊断通道**（Host 的结构化失败行
/// 走的就是它），让「能力缺失」在产品侧留下一条记录。
///
/// 壳**不解释**这份清单（不据它做任何决定）：它只是把渲染层自己的话记下来。
#[tauri::command(rename = "host_platform_report")]
pub fn host_platform_report(report: PlatformSurfaceReport) -> Result<(), String> {
    let bounded = |names: &[String]| -> String {
        let mut rendered: Vec<String> = names
            .iter()
            .take(MAX_REPORT_ENTRIES)
            .map(|name| {
                let mut clipped: String = name.chars().take(MAX_REPORT_NAME_LEN).collect();
                if clipped.chars().any(|c| c.is_whitespace()) {
                    clipped = "<invalid-name>".to_string();
                }
                clipped
            })
            .collect();
        if names.len() > MAX_REPORT_ENTRIES {
            rendered.push(format!("+{} more", names.len() - MAX_REPORT_ENTRIES));
        }
        rendered.join(",")
    };
    // 一行、键值形态：与 Host 的结构化失败行（`startup_failed phase=… errorCode=…`）同一种读法。
    eprintln!(
        "[mode-tauri] platform_surface implemented={} rejecting={} noop={} absent={} degraded={} reasons={}",
        report.implemented.len(),
        report.not_implemented.len(),
        report.no_op.len(),
        report.deliberately_absent.len(),
        report.degraded.len(),
        bounded(&report.degraded),
    );
    Ok(())
}

/// 本模块注册的命令，交给 `platform::commands::init_with_extra` 并进同一个 `invoke_handler`。
pub fn handler<R: Runtime>() -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        host_send,
        host_recv,
        host_startup_state,
        host_startup_id,
        host_startup_control,
        host_platform_report
    ]
}

/// 命令名清单（与 `sidecar_host::bridge::COMMAND_NAMES` 同源；由 Node 侧守卫用例比对）。
pub const COMMAND_NAMES: &[&str] = &[
    CMD_SEND,
    CMD_RECV,
    CMD_STATE,
    CMD_STARTUP_ID,
    CMD_STARTUP_CONTROL,
    CMD_PLATFORM_REPORT,
];
