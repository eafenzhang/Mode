//! Host sidecar 的生命周期用例（M3 计划 DR-T1-3 的判据 + 就绪/失败/超时/退出后的各条路径）。
//!
//! 这些用例**真的拉起 Node 子进程**（本机与 CI 的 Windows 腿都有 node），因为要验的正是进程事实：
//! 就绪信号来自 Host 写出的第一帧、失败信号来自壳能观测到的进程事实（起不来 / 退出 / 超期 / 就绪后崩）。
//! 端口那一侧（真 `MessagePort` + `Initialize` + RPC + 事件订阅）由
//! `apps/desktop-tauri/test/portRoundTrip.test.mts` 覆盖，这里不重复。
//!
//! 用例辅助（`silent_config` / `wait_for_phase` / `wait_for_failed`）刻意**留在这里**，
//! 不进 `sidecar_host` 的生产面：复评指出把测试辅助挂在公开 API 上会把两条面混在一起。
//!
//! 运行：`node scripts/rust-cargo.ts test -p sidecar-host`（CI 的 Windows 腿也跑，见 release.yml）。

use std::path::PathBuf;
use std::time::Duration;

use sidecar_host::process::process_exists;
use sidecar_host::{
    HostConfig, HostError, HostHandle, StartupErrorCode, StartupPhase, StartupSnapshot, StartupStage,
};

/// 仓库根（`crates/sidecar-host` 上溯两级）。
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// 真的 Host 入口（Task 1 的交付物）。
fn sidecar_entry() -> PathBuf {
    repo_root().join("packages/desktop/src/host-standalone/index.ts")
}

/// 隔离用的数据**基**目录：一律落在 `target/` 下，绝不碰开发者真实家目录。
/// （语义是基目录：Host 的根 = `{base}/.mode`，见 `sidecar_host::data_base_dir_from`。）
fn isolated_base_dir(name: &str) -> PathBuf {
    repo_root().join("target/host-sidecar-test-data").join(name)
}

fn sidecar_config(name: &str) -> HostConfig {
    let mut config = HostConfig::for_sidecar_entry(sidecar_entry(), repo_root());
    config.startup_id = format!("test-{name}");
    config.attempt_id = format!("test-{name}-a1");
    config.data_base_dir = isolated_base_dir(name);
    config.ready_timeout = Duration::from_secs(30);
    config
}

/// 只跑 node 参数、不写 stdout 的「沉默 Host」：监督路径的用例需要一个可控的子进程。
fn silent_config(name: &str, script: &str) -> HostConfig {
    let mut config = HostConfig::for_sidecar_entry(sidecar_entry(), repo_root());
    config.entry = None;
    config.node_args = vec!["--eval".to_string(), script.to_string()];
    config.startup_id = format!("test-{name}");
    config.attempt_id = format!("test-{name}-a1");
    config.data_base_dir = isolated_base_dir(name);
    config
}

/// 等 Host 到某个阶段；超时、或期间已经失败，返回 `None`。
fn wait_for_phase(
    handle: &HostHandle,
    phase: StartupPhase,
    timeout: Duration,
) -> Option<StartupSnapshot> {
    let deadline = std::time::Instant::now() + timeout;
    let mut since = None;
    loop {
        let snapshot = handle.wait_for_startup_change(since, Duration::from_millis(50));
        if snapshot.phase == phase {
            return Some(snapshot);
        }
        if snapshot.phase == StartupPhase::Failed {
            return None;
        }
        since = Some(snapshot.sequence);
        if std::time::Instant::now() >= deadline {
            return None;
        }
    }
}

/// 等 Host 失败并返回终态快照（`wait_for_phase` 会把失败当成「没到目标阶段」）。
fn wait_for_failed(handle: &HostHandle, timeout: Duration) -> StartupSnapshot {
    let deadline = std::time::Instant::now() + timeout;
    let mut since = None;
    loop {
        let snapshot = handle.wait_for_startup_change(since, Duration::from_millis(50));
        if snapshot.phase == StartupPhase::Failed {
            return snapshot;
        }
        since = Some(snapshot.sequence);
        if std::time::Instant::now() >= deadline {
            // 拿不到失败就返回当前快照，由调用方按 phase 断言判红（不在这里 panic，
            // 错误信息里能带上真实快照）。
            return snapshot;
        }
    }
}

/// 轮询等进程消失（taskkill 是同步的，但进程表刷新有一点延迟）。
fn wait_until_gone(pid: u32) -> bool {
    for _ in 0..20 {
        if !process_exists(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[test]
fn real_sidecar_becomes_ready_on_its_first_frame_and_shuts_down_cleanly() {
    let handle = HostHandle::spawn(sidecar_config("ready")).expect("拉起真 Host");
    let pid = handle.pid();

    assert!(
        wait_for_phase(&handle, StartupPhase::Ready, Duration::from_secs(30)).is_some(),
        "30s 内应就绪；当前快照：{:?}",
        handle.startup_snapshot()
    );

    // 第一帧就是 `ChannelServer` 的 `Initialize`。线上字节实测形如 `[4, 1, 6, 200, …]`：
    // 首字节 4 是序列化层的 Array 标签，第 4 个字节 200 是 `ResponseType.Initialize`
    // （帧的语义判定在 Node 侧：`test/portRoundTrip.test.mts` 用真 `ChannelClient` 断言
    // `Initialize` 到达；这里只验「壳确实收到了 Host 的第一帧」）。
    let frame = handle
        .next_frame(Duration::from_secs(10))
        .expect("读帧成功")
        .expect("应拿到就绪后的第一帧");
    assert!(
        frame.len() >= 4 && frame[0] == 4 && frame[3] == 200,
        "Host → 页面的第一帧必须是 Initialize envelope，实际：{frame:?}"
    );

    handle.shutdown().expect("shutdown 成功");
    assert!(!handle.is_running(), "shutdown 后 Host 必须已经退出");
    assert!(wait_until_gone(pid), "进程表里不得残留 sidecar（pid {pid}）");
    handle.shutdown().expect("重复 shutdown 必须无害");
}

/// 实测抓下来的一次 `host.describe` 请求帧（真 `ChannelClient` → 真端口 → 产品 Host）。
///
/// 这是**跨语言帧契约的定点**：Rust 侧按壳的方式把它整帧写进 Host 的 stdin
/// （`host_send` 做的就是这件事），产品 Node 解码器必须解出它、`ChannelServer` 必须回一帧，
/// 而 Rust 的读帧必须能把那一帧解出来。两侧任一方改坏编码或分帧，这条用例就红——
/// 原来 `frame.rs` 里「Node 侧同形状实现」那句话只有注释，没有测试守着（复评 I4(c)）。
///
/// 抓取方式（可复核）：让真 `ChannelClient` 经真 `MessagePort` 对产品 Host 调一次
/// `host.describe`，把 relay 观测到的第一帧原样打出来。
const DESCRIBE_REQUEST: &[u8] = &[
    4, 4, 6, 100, 6, 0, 1, 4, 104, 111, 115, 116, 1, 8, 100, 101, 115, 99, 114, 105, 98, 101, 0,
];

#[test]
fn the_shell_frame_path_reaches_the_host_and_gets_a_reply() {
    let handle = HostHandle::spawn(sidecar_config("send-path")).expect("拉起真 Host");
    assert!(
        wait_for_phase(&handle, StartupPhase::Ready, Duration::from_secs(30)).is_some(),
        "先要就绪"
    );

    // Initialize 先到（就绪判据就是它）：消费掉，免得把它当成应答。
    let initialize = handle
        .next_frame(Duration::from_secs(10))
        .expect("读帧成功")
        .expect("应拿到 Initialize");
    assert_eq!(initialize[3], 200, "第一帧是 Initialize：{initialize:?}");

    // 壳 → Host：整帧写进去（与 `host_send` 同一条路径）。
    handle.send_frame(DESCRIBE_REQUEST).expect("写帧成功");

    // Host → 壳：应答帧必须回来，且内容确实来自我们的 host 频道。
    let reply = handle
        .next_frame(Duration::from_secs(10))
        .expect("读帧成功")
        .expect("应拿到 describe 的应答");
    assert_ne!(reply, initialize, "应答不能还是 Initialize");
    let text = String::from_utf8_lossy(&reply);
    assert!(
        text.contains("mode-host-sidecar"),
        "应答里应带 host.describe 的载荷（name 字段），实际：{text}"
    );
    assert!(
        text.contains("dataRoot"),
        "应答里应带数据根字段，实际：{text}"
    );

    let _ = handle.shutdown();
}

#[test]
fn next_frame_reports_timeout_then_closed_for_a_silent_host() {
    // 覆盖 `host_recv` 的两个非数据分支：期限内没有帧 → `Ok(None)`（页面继续轮询）；
    // 传输关闭且队列排空 → `Err(TransportClosed)`（页面停泵，失败原因由状态流给出）。
    let mut config = silent_config("recv-timeout", "setInterval(function () {}, 1000);");
    config.ready_timeout = Duration::from_secs(60); // 别让看门狗提前收摊

    let handle = HostHandle::spawn(config).expect("拉起沉默 Host");
    assert_eq!(
        handle.next_frame(Duration::from_millis(200)).expect("超时不是错误"),
        None,
        "期限内没有帧应返回 Ok(None)"
    );
    handle.shutdown().expect("shutdown 成功");
    assert!(
        matches!(
            handle.next_frame(Duration::from_millis(200)),
            Err(HostError::TransportClosed)
        ),
        "关闭且排空后必须显式报传输关闭，不能继续空转"
    );
}

#[test]
fn shutdown_kills_a_silent_host_and_leaves_no_process() {
    let mut config = silent_config("silent", "setInterval(function () {}, 1000);");
    config.ready_timeout = Duration::from_secs(60); // 别让看门狗抢在用例的 shutdown 之前

    let handle = HostHandle::spawn(config).expect("拉起沉默 Host");
    let pid = handle.pid();
    assert!(handle.is_running(), "沉默 Host 应该活着");
    assert_eq!(handle.startup_snapshot().phase, StartupPhase::Starting);

    handle.shutdown().expect("shutdown 成功");
    assert!(wait_until_gone(pid), "shutdown 后不得残留进程（pid {pid}）");
}

#[test]
fn a_host_that_exits_before_ready_reports_failed_with_an_error_code() {
    // 入口不存在 → node 立刻以非 0 退出：这正是「sidecar 崩了」在壳侧的样子。
    let mut config =
        HostConfig::for_sidecar_entry(repo_root().join("no-such-host-entry.ts"), repo_root());
    config.startup_id = "test-crash".to_string();
    config.attempt_id = "test-crash-a1".to_string();
    config.data_base_dir = isolated_base_dir("crash-before-ready");
    config.ready_timeout = Duration::from_secs(30);

    let handle = HostHandle::spawn(config).expect("node 本身能起来（失败发生在脚本解析）");
    let snapshot = wait_for_failed(&handle, Duration::from_secs(20));
    assert_eq!(snapshot.phase, StartupPhase::Failed);
    assert_eq!(
        snapshot.error_code,
        Some(StartupErrorCode::TransportClosed),
        "就绪前退出必须报 transport_closed（渲染层据此画失败屏，而不是等超时）"
    );
    assert_eq!(snapshot.failed_phase, Some(StartupStage::Starting));
    let _ = handle.shutdown();
}

#[test]
fn a_host_that_reports_a_stage_precise_failure_has_it_forwarded_verbatim() {
    // DR-T3-3 ①：Host 知道的是 `sql_failed` / `preparing_host_storage`，壳必须把这两个字段
    // **原样**送到渲染层，而不是塌成 `transport_closed` / `starting`。这不只是措辞：
    // 渲染层的 `canRetryDatabaseStartup` 按 `failedPhase` + `errorCode` 决定给不给「重试」，
    // 而粗粒度原因（transport_closed）会把它算成不可重试。
    let script = concat!(
        "process.stderr.write('[mode-host-sidecar] startup_failed phase=failed ",
        "failedPhase=preparing_host_storage errorCode=sql_failed error=ENOTDIR: not a directory');",
        "process.exit(1);"
    );
    let mut config = silent_config("stage-precise", script);
    config.ready_timeout = Duration::from_secs(30);

    let handle = HostHandle::spawn(config).expect("拉起子进程");
    let snapshot = wait_for_failed(&handle, Duration::from_secs(20));
    assert_eq!(snapshot.phase, StartupPhase::Failed);
    assert_eq!(
        snapshot.error_code,
        Some(StartupErrorCode::SqlFailed),
        "Host 报的错码必须原样送到渲染层（不是壳自己判断的 transport_closed）"
    );
    assert_eq!(
        snapshot.failed_phase,
        Some(StartupStage::PreparingHostStorage),
        "失败阶段必须精确到 Host 报的那一段"
    );
    // 渲染层读的是序列化之后的字段名/取值，所以在这里也钉一遍（.strict() schema，写错就整条丢）。
    let serialized = snapshot.to_state_json().to_string();
    assert!(
        serialized.contains("\"errorCode\":\"sql_failed\""),
        "序列化后的 errorCode 必须是 sql_failed：{serialized}"
    );
    assert!(
        serialized.contains("\"failedPhase\":\"preparing_host_storage\""),
        "序列化后的 failedPhase 必须是 preparing_host_storage：{serialized}"
    );
    let _ = handle.shutdown();
}

#[test]
fn a_host_that_dies_after_ready_publishes_failed_transport_closed() {
    // 「就绪后崩」也是一条必须被推送的状态：渲染层挂载完成后 Host 死掉，
    // 不能只表现为后续 RPC 全部超时（2026-10-16 复评把这条列为未覆盖项）。
    let handle = HostHandle::spawn(sidecar_config("crash-after-ready")).expect("拉起真 Host");
    let pid = handle.pid();
    assert!(
        wait_for_phase(&handle, StartupPhase::Ready, Duration::from_secs(30)).is_some(),
        "先要就绪"
    );

    // 从**外部**杀（不是 handle.shutdown()：那条路径会标记主动退出、按设计不推 failed），
    // 模拟 Host 自己崩掉。
    let killed = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status()
        .expect("taskkill 可用");
    assert!(
        killed.success() || wait_until_gone(pid),
        "用例先要把 Host 真的杀掉（pid {pid}）"
    );

    let snapshot = wait_for_failed(&handle, Duration::from_secs(20));
    assert_eq!(
        snapshot.phase,
        StartupPhase::Failed,
        "就绪后崩必须推 failed；当前快照：{snapshot:?}"
    );
    assert_eq!(
        snapshot.error_code,
        Some(StartupErrorCode::TransportClosed),
        "就绪后崩的错误码是 transport_closed"
    );
    let _ = handle.shutdown();
}

#[test]
fn a_host_that_never_writes_hits_the_ready_deadline_and_gets_killed() {
    let mut config = silent_config("deadline", "setInterval(function () {}, 1000);");
    config.ready_timeout = Duration::from_millis(300);

    let handle = HostHandle::spawn(config).expect("拉起沉默 Host");
    let pid = handle.pid();
    let snapshot = wait_for_failed(&handle, Duration::from_secs(10));
    assert_eq!(snapshot.phase, StartupPhase::Failed);
    assert_eq!(
        snapshot.error_code,
        Some(StartupErrorCode::StartupStatusTimeout)
    );
    assert!(wait_until_gone(pid), "超期的 Host 必须被壳收掉（pid {pid}）");
    let _ = handle.shutdown();
}

#[test]
fn a_host_that_cannot_be_spawned_still_yields_a_queryable_failed_handle() {
    let mut config = HostConfig::for_sidecar_entry(sidecar_entry(), repo_root());
    config.node = PathBuf::from("definitely-not-a-node-binary-m3");
    config.startup_id = "test-missing-node".to_string();
    config.attempt_id = "test-missing-node-a1".to_string();
    config.data_base_dir = isolated_base_dir("missing-node");

    let error = HostHandle::spawn(config.clone()).expect_err("node 不存在时应失败");
    assert!(matches!(error, HostError::Spawn(_)));
    assert_eq!(error.startup_error_code(), StartupErrorCode::OpenFailed);

    // 关键：失败也要有句柄，否则渲染层拿不到失败状态（DR-T1-3）。
    let handle = HostHandle::failed(&config, &error);
    let snapshot = handle.startup_snapshot();
    assert_eq!(snapshot.phase, StartupPhase::Failed);
    assert_eq!(snapshot.error_code, Some(StartupErrorCode::OpenFailed));
    assert!(!handle.is_running());
    // 状态流必须能立刻取到失败状态（渲染层的长轮询就是这条路径）。
    let polled = handle.wait_for_startup_change(None, Duration::from_millis(50));
    assert_eq!(polled.sequence, snapshot.sequence);
    assert_eq!(polled.to_state_json()["phase"], serde_json::json!("failed"));
}

#[test]
fn spawn_refuses_an_empty_data_base_dir_instead_of_targeting_the_real_root() {
    // M8：空基目录会被子进程当成「未设置」，于是 Host 悄悄指向开发者真实的数据根。
    // 隔离必须由配置校验保证，而不是靠每个调用点记得设。
    let mut config = sidecar_config("empty-base");
    config.data_base_dir = PathBuf::new();

    let error = HostHandle::spawn(config).expect_err("空数据基目录必须被拒绝");
    assert!(matches!(error, HostError::InvalidConfig(_)));
    let message = error.to_string();
    assert!(message.contains("data_base_dir"), "错误要点名字段：{message}");
}
