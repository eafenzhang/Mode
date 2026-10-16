//! Host sidecar 的生命周期用例（M3 计划 DR-T1-3 的两条判据 + 就绪/失败路径）。
//!
//! 这些用例**真的拉起 Node 子进程**（本机 / CI 的 Windows 腿都有 node），因为要验的正是进程事实：
//! 就绪信号来自 Host 写出的第一帧、失败信号来自壳能观测到的进程事实（起不来 / 退出 / 超期）。
//! 端口那一侧（真 `MessagePort` + `Initialize` + RPC）由
//! `apps/desktop-tauri/test/portRoundTrip.test.mts` 覆盖，这里不重复。
//!
//! 运行：`node scripts/rust-cargo.ts test -p sidecar-host`

use std::path::PathBuf;
use std::time::Duration;

use sidecar_host::process::{process_exists, silent_config, wait_for_phase};
use sidecar_host::{
    HostConfig, HostError, HostHandle, StartupErrorCode, StartupPhase,
};

/// 仓库根（`crates/sidecar-host` 上溯两级）。
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// 真的 Host 入口（Task 1 的交付物）。
fn sidecar_entry() -> PathBuf {
    repo_root().join("packages/desktop/src/host-standalone/index.ts")
}

fn sidecar_config(name: &str) -> HostConfig {
    let mut config = HostConfig::for_sidecar_entry(sidecar_entry(), repo_root());
    config.startup_id = format!("test-{name}");
    config.attempt_id = format!("test-{name}-a1");
    // 隔离：用例绝不碰用户真实数据根（Task 1 的 Host 不写盘，这条是双保险）。
    config.data_root = repo_root().join("target/host-sidecar-test-data").join(name);
    config.ready_timeout = Duration::from_secs(30);
    config
}

fn failed_code(handle: &HostHandle) -> Option<StartupErrorCode> {
    handle.startup_snapshot().error_code
}

#[test]
fn real_sidecar_becomes_ready_on_its_first_frame_and_shuts_down_cleanly() {
    let handle = HostHandle::spawn(sidecar_config("ready")).expect("拉起真 Host");
    let pid = handle.pid();

    assert!(
        wait_for_phase(&handle, StartupPhase::Ready, Duration::from_secs(30)),
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

    // 主动收摊：不留孤儿，且幂等（RunEvent::Exit 与 Drop 可能都调一次）。
    handle.shutdown().expect("shutdown 成功");
    assert!(!handle.is_running(), "shutdown 后 Host 必须已经退出");
    assert!(
        !process_exists(pid),
        "进程表里不得残留 sidecar（pid {pid}）"
    );
    handle.shutdown().expect("重复 shutdown 必须无害");
}

#[test]
fn shutdown_kills_a_silent_host_and_leaves_no_process() {
    // 沉默子进程：只活不说话（监督路径的用例需要一个可控的 Host）。
    let mut config = silent_config("setInterval(function () {}, 1000);");
    config.startup_id = "test-silent".to_string();
    config.attempt_id = "test-silent-a1".to_string();
    config.ready_timeout = Duration::from_secs(60); // 别让看门狗抢在用例的 shutdown 之前

    let handle = HostHandle::spawn(config).expect("拉起沉默 Host");
    let pid = handle.pid();
    assert!(handle.is_running(), "沉默 Host 应该活着");
    assert_eq!(handle.startup_snapshot().phase, StartupPhase::Starting);

    handle.shutdown().expect("shutdown 成功");
    // taskkill /T 是同步的，但进程表刷新有一点点延迟。
    for _ in 0..20 {
        if !process_exists(pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!process_exists(pid), "shutdown 后不得残留进程（pid {pid}）");
}

#[test]
fn a_host_that_exits_before_ready_reports_failed_with_an_error_code() {
    // 入口不存在 → node 立刻以非 0 退出：这正是「sidecar 崩了」在壳侧的样子。
    let mut config = HostConfig::for_sidecar_entry(repo_root().join("no-such-host-entry.ts"), repo_root());
    config.startup_id = "test-crash".to_string();
    config.attempt_id = "test-crash-a1".to_string();
    config.ready_timeout = Duration::from_secs(30);

    let handle = HostHandle::spawn(config).expect("node 本身能起来（失败发生在脚本解析）");
    assert!(
        !wait_for_phase(&handle, StartupPhase::Ready, Duration::from_secs(20)),
        "退出的 Host 不可能就绪"
    );
    let snapshot = handle.startup_snapshot();
    assert_eq!(snapshot.phase, StartupPhase::Failed);
    assert_eq!(
        snapshot.error_code,
        Some(StartupErrorCode::TransportClosed),
        "就绪前退出必须报 transport_closed（渲染层据此画失败屏，而不是等 40s 超时）"
    );
    assert_eq!(snapshot.failed_phase, Some(StartupPhase::Starting));
    let _ = handle.shutdown();
}

#[test]
fn a_host_that_never_writes_hits_the_ready_deadline_and_gets_killed() {
    let mut config = silent_config("setInterval(function () {}, 1000);");
    config.startup_id = "test-deadline".to_string();
    config.attempt_id = "test-deadline-a1".to_string();
    config.ready_timeout = Duration::from_millis(300);

    let handle = HostHandle::spawn(config).expect("拉起沉默 Host");
    let pid = handle.pid();
    assert!(
        !wait_for_phase(&handle, StartupPhase::Ready, Duration::from_secs(10)),
        "沉默 Host 不可能就绪"
    );
    let snapshot = handle.startup_snapshot();
    assert_eq!(snapshot.phase, StartupPhase::Failed);
    assert_eq!(snapshot.error_code, Some(StartupErrorCode::StartupStatusTimeout));
    for _ in 0..20 {
        if !process_exists(pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!process_exists(pid), "超期的 Host 必须被壳收掉（pid {pid}）");
    let _ = handle.shutdown();
}

#[test]
fn a_host_that_cannot_be_spawned_still_yields_a_queryable_failed_handle() {
    let mut config = HostConfig::for_sidecar_entry(sidecar_entry(), repo_root());
    config.node = PathBuf::from("definitely-not-a-node-binary-m3");
    config.startup_id = "test-missing-node".to_string();
    config.attempt_id = "test-missing-node-a1".to_string();

    let error = HostHandle::spawn(config.clone()).expect_err("node 不存在时应失败");
    assert!(matches!(error, HostError::Spawn(_)));
    assert_eq!(error.startup_error_code(), StartupErrorCode::OpenFailed);

    // 关键：失败也要有句柄，否则渲染层拿不到失败状态（DR-T1-3）。
    let handle = HostHandle::failed(&config, &error);
    let snapshot = handle.startup_snapshot();
    assert_eq!(snapshot.phase, StartupPhase::Failed);
    assert_eq!(failed_code(&handle), Some(StartupErrorCode::OpenFailed));
    assert!(!handle.is_running());
    // 状态流必须能立刻取到失败状态（渲染层的长轮询就是这条路径）。
    let polled = handle.wait_for_startup_change(None, Duration::from_millis(50));
    assert_eq!(polled.sequence, snapshot.sequence);
    assert_eq!(polled.to_state_json()["phase"], serde_json::json!("failed"));
}
