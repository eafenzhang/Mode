//! `RemoteTransportRouter` 契约（LT-16.4d 收口）。
//!
//! 分派器要解决的是「五种 kind、各自阻塞点不同」：LAN 已完成，其余三种没落地。
//! 这里验三件事：正确路由到对应端口、未注册 kind 报准确错误、entry 记录不泄漏。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use host_channels::remote::RemoteTransportPort;
use host_channels::RemoteTransportRouter;
use remote_session_registry::{ConnectPlan, RemoteTargetSpec, RemoteWorkspaceContext};
use rpc_host::BoxFuture;

/// 记录调用的假端口。
#[derive(Default)]
struct SpyPort {
    connects: AtomicUsize,
    aborts: AtomicUsize,
    disposes: AtomicUsize,
    releases: AtomicUsize,
    last_entry: AtomicUsize,
    fail: Mutex<Option<String>>,
}

impl SpyPort {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl RemoteTransportPort for SpyPort {
    fn connect(
        &self,
        plan: ConnectPlan,
        _target: RemoteTargetSpec,
    ) -> BoxFuture<Result<(), String>> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        self.last_entry
            .store(plan.entry_id as usize, Ordering::SeqCst);
        let failure = self.fail.lock().expect("fail").clone();
        Box::pin(async move {
            match failure {
                Some(message) => Err(message),
                None => Ok(()),
            }
        })
    }

    fn abort(&self, _entry_id: u64) {
        self.aborts.fetch_add(1, Ordering::SeqCst);
    }

    fn dispose(&self, _entry_id: u64) {
        self.disposes.fetch_add(1, Ordering::SeqCst);
    }

    fn release_workspace(
        &self,
        _entry_id: u64,
        _workspace_key: String,
        _generation: u32,
        _context: RemoteWorkspaceContext,
    ) -> BoxFuture<Result<(), String>> {
        self.releases.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

fn lan(host: &str, port: u16) -> RemoteTargetSpec {
    RemoteTargetSpec::Lan {
        host: host.to_string(),
        port,
        server_id: None,
        token: None,
    }
}

fn ssh() -> RemoteTargetSpec {
    RemoteTargetSpec::Ssh {
        host: "h".to_string(),
        port: None,
        username: "u".to_string(),
        private_key_path: None,
        has_password: false,
    }
}

fn plan(entry_id: u64, session_id: &str) -> ConnectPlan {
    ConnectPlan {
        remote_session_id: session_id.to_string(),
        entry_id,
        reused_online_entry: false,
        connection_key: "key".to_string(),
    }
}

#[tokio::test]
async fn routes_each_kind_to_its_own_port() {
    let lan_port = SpyPort::new();
    let ssh_port = SpyPort::new();
    let router = RemoteTransportRouter::new()
        .with_port("lan", lan_port.clone() as Arc<dyn RemoteTransportPort>)
        .with_port("ssh", ssh_port.clone() as Arc<dyn RemoteTransportPort>);

    assert_eq!(router.registered_kinds(), vec!["lan", "ssh"]);

    router
        .connect(plan(1, "s1"), lan("h", 1))
        .await
        .expect("lan");
    router.connect(plan(2, "s2"), ssh()).await.expect("ssh");

    assert_eq!(lan_port.connects.load(Ordering::SeqCst), 1);
    assert_eq!(ssh_port.connects.load(Ordering::SeqCst), 1);
    assert_eq!(router.tracked_entries(), 2);
}

#[tokio::test]
async fn unsupported_kind_reports_which_transport_is_missing() {
    let router = RemoteTransportRouter::new()
        .with_port("lan", SpyPort::new() as Arc<dyn RemoteTransportPort>);

    for (target, expected) in [
        (ssh(), "SSH"),
        (
            RemoteTargetSpec::Wsl {
                distro: None,
                user: None,
            },
            "WSL",
        ),
        (
            RemoteTargetSpec::Docker {
                container: "c".to_string(),
            },
            "Docker",
        ),
        (
            RemoteTargetSpec::V3 {
                device_id: "d".to_string(),
            },
            "v3",
        ),
    ] {
        let error = router
            .connect(plan(9, "s"), target)
            .await
            .expect_err("未注册 kind 必须失败");
        // 错误要指向「这种传输没落地」，而不是笼统的「服务未就绪」——
        // 后者会让用户以为是配对或网络问题。
        assert!(error.contains(expected), "期望提到 {expected}：{error}");
        assert!(error.contains("尚未支持"), "{error}");
    }
    // 失败的连接不得留下 entry 记录。
    assert_eq!(router.tracked_entries(), 0);
}

#[tokio::test]
async fn failed_connect_does_not_track_the_entry() {
    let port = SpyPort::new();
    *port.fail.lock().expect("fail") = Some("连接失败".to_string());
    let router =
        RemoteTransportRouter::new().with_port("lan", port as Arc<dyn RemoteTransportPort>);

    let error = router
        .connect(plan(3, "s"), lan("h", 1))
        .await
        .expect_err("传输失败");
    assert_eq!(error, "连接失败");
    assert_eq!(
        router.tracked_entries(),
        0,
        "失败的连接不得留下 entry 记录（否则 dispose 会去断一个不存在的连接）"
    );
}

#[tokio::test]
async fn abort_and_dispose_route_by_the_recorded_kind() {
    let lan_port = SpyPort::new();
    let ssh_port = SpyPort::new();
    let router = RemoteTransportRouter::new()
        .with_port("lan", lan_port.clone() as Arc<dyn RemoteTransportPort>)
        .with_port("ssh", ssh_port.clone() as Arc<dyn RemoteTransportPort>);

    router
        .connect(plan(10, "s1"), lan("h", 1))
        .await
        .expect("lan");
    router.connect(plan(11, "s2"), ssh()).await.expect("ssh");

    // abort(10) 应只打到 lan 端口。
    router.abort(10);
    assert_eq!(lan_port.aborts.load(Ordering::SeqCst), 1);
    assert_eq!(ssh_port.aborts.load(Ordering::SeqCst), 0);

    // dispose(11) 应只打到 ssh 端口。
    router.dispose(11);
    assert_eq!(lan_port.disposes.load(Ordering::SeqCst), 0);
    assert_eq!(ssh_port.disposes.load(Ordering::SeqCst), 1);

    // 两条记录都被摘掉。
    assert_eq!(router.tracked_entries(), 0);
}

#[tokio::test]
async fn unknown_entry_operations_are_no_ops() {
    let lan_port = SpyPort::new();
    let router = RemoteTransportRouter::new()
        .with_port("lan", lan_port.clone() as Arc<dyn RemoteTransportPort>);

    // 没连接过的 entry：abort/dispose 都不该打到任何端口（否则会误断别人的连接）。
    router.abort(999);
    router.dispose(999);
    assert_eq!(lan_port.aborts.load(Ordering::SeqCst), 0);
    assert_eq!(lan_port.disposes.load(Ordering::SeqCst), 0);

    // release 对未知 entry 也无事可做。
    assert_eq!(
        router
            .release_workspace(
                999,
                "/w".to_string(),
                1,
                RemoteWorkspaceContext {
                    workspace_path: "/w".to_string(),
                    workspace_identity: None,
                },
            )
            .await,
        Ok(())
    );
    assert_eq!(lan_port.releases.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn release_workspace_routes_to_the_owning_port() {
    let lan_port = SpyPort::new();
    let ssh_port = SpyPort::new();
    let router = RemoteTransportRouter::new()
        .with_port("lan", lan_port.clone() as Arc<dyn RemoteTransportPort>)
        .with_port("ssh", ssh_port.clone() as Arc<dyn RemoteTransportPort>);

    router
        .connect(plan(20, "s"), ssh())
        .await
        .expect("ssh connect");
    router
        .release_workspace(
            20,
            "/w".to_string(),
            2,
            RemoteWorkspaceContext {
                workspace_path: "/w".to_string(),
                workspace_identity: None,
            },
        )
        .await
        .expect("release");

    assert_eq!(ssh_port.releases.load(Ordering::SeqCst), 1);
    assert_eq!(lan_port.releases.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn repeated_dispose_does_not_leak_tracking() {
    let lan_port = SpyPort::new();
    let router = RemoteTransportRouter::new()
        .with_port("lan", lan_port.clone() as Arc<dyn RemoteTransportPort>);

    for entry in 30..33 {
        router
            .connect(plan(entry, "s"), lan("h", 1))
            .await
            .expect("connect");
    }
    assert_eq!(router.tracked_entries(), 3);

    for entry in 30..33 {
        router.dispose(entry);
        router.dispose(entry); // 幂等
    }
    assert_eq!(router.tracked_entries(), 0);
    // 每个 entry 只真的断一次：第二次 dispose 时记录已摘，不会再打端口。
    assert_eq!(lan_port.disposes.load(Ordering::SeqCst), 3);
}
