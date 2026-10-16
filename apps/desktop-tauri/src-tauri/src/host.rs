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
use std::time::Duration;

use sidecar_host::bridge::{CMD_PLATFORM_REPORT, CMD_RECV, CMD_SEND, CMD_STATE};
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

    sidecar_host::pick_entry(std::env::var_os("MODE_HOST_SIDECAR_ENTRY").as_deref(), &candidates)
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

    let node = std::env::var_os("MODE_HOST_NODE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("node"));
    let mut config = match resolve_entry(app) {
        Some(entry) => HostConfig::for_sidecar_entry(entry, cwd),
        // 没有入口也别在这里失败：交给 `spawn_host` 转成推给渲染层的 failed 状态。
        None => HostConfig::unavailable(node.clone(), cwd),
    };
    config.node = node;
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

/// 起 Host。**不返回 `Result`**：起不来时返回一个「已失败」的句柄，渲染层照样能拿到
/// `phase: "failed"` + `errorCode`（DR-T1-3：失败必须被推给渲染层，而不是让壳先死）。
pub fn spawn_host<R: Runtime>(app: &AppHandle<R>) -> HostHandle {
    let config = resolve_config(app);
    match HostHandle::spawn(config.clone()) {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("[mode-tauri] 起 Host 失败：{error}（渲染层会收到 failed 状态）");
            HostHandle::failed(&config, &error)
        }
    }
}

/// 页面 → Host：整帧 raw body 写进 sidecar 的 stdin。
#[tauri::command(rename = "host_send")]
pub fn host_send(handle: State<'_, HostHandle>, request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes.clone(),
        other => return Err(format!("host_send 期望 raw body，实收 {other:?}")),
    };
    handle.send_frame(&bytes).map_err(|error| error.to_string())
}

/// Host → 页面：长轮询一帧，按 raw bytes 返回（页面拿到 `ArrayBuffer`，必须自己包 `Uint8Array`）。
#[tauri::command(rename = "host_recv")]
pub async fn host_recv(handle: State<'_, HostHandle>) -> Result<tauri::ipc::Response, String> {
    let handle = handle.inner().clone();
    let frame = tauri::async_runtime::spawn_blocking(move || handle.next_frame(RECV_TIMEOUT))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    Ok(tauri::ipc::Response::new(frame.unwrap_or_default()))
}

/// 启动状态长轮询：返回 `databaseStartupStateSchema` 形状的最新状态（不含任何业务判断）。
#[tauri::command(rename = "host_startup_state")]
pub async fn host_startup_state(
    handle: State<'_, HostHandle>,
    since: Option<u32>,
) -> Result<serde_json::Value, String> {
    let handle = handle.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        handle.wait_for_startup_change(since, STATE_TIMEOUT).to_state_json()
    })
    .await
    .map_err(|error| error.to_string())
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
        host_platform_report
    ]
}

/// 命令名清单（与 `sidecar_host::bridge::COMMAND_NAMES` 同源；由 Node 侧守卫用例比对）。
pub const COMMAND_NAMES: &[&str] = &[CMD_SEND, CMD_RECV, CMD_STATE, CMD_PLATFORM_REPORT];
