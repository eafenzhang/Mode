//! `LanHostSupervisor` 行为契约（LT-16.4a）：监听生命周期 + 状态聚合 + 事件。
//!
//! 真实 lan-http/LanPairing（临时凭据库）+ axum serve；默认端口传 0 避免占用测试机端口。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mode_server::lan::lan_pairing_from_store;
use mode_server::lan_host::LanHostSupervisor;
use mode_server::server_info::ServerInfoConfig;
use mode_server::ws_host::{HostCapabilityStore, WsHostDeps};
use rpc_wire::Value;

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "lan-host-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config() -> ServerInfoConfig {
    ServerInfoConfig {
        server_id: "srv-test".to_string(),
        name: Some("测试机".to_string()),
        version: "0.0.1".to_string(),
        auth_required: true,
        workspaces: Vec::new(),
        public_relay: None,
        computer_use: None,
        remote_desktop: None,
        remote_desktop_native: None,
        lan_peer_controller: None,
    }
}

fn supervisor(
    dir: &TempDir,
    log: Arc<Mutex<Vec<bool>>>,
) -> Arc<LanHostSupervisor> {
    let pairing = lan_pairing_from_store(&dir.0, None).expect("pairing");
    let deps = WsHostDeps {
        host_capabilities: Arc::new(HostCapabilityStore::new()),
        agent_control: None,
        data_root: dir.0.clone(),
        lan_pairing: Some(Arc::clone(&pairing)),
    };
    Arc::new(LanHostSupervisor::new(
        tokio::runtime::Handle::current(),
        pairing,
        config(),
        deps,
        0,
        Arc::new(move |enabled| log.lock().expect("log").push(enabled)),
    ))
}

#[tokio::test]
async fn start_loopback_then_stop() {
    let dir = TempDir::new("loopback");
    let supervisor = supervisor(&dir, Arc::new(Mutex::new(Vec::new())));

    let state = supervisor.start(false).await.expect("start loopback");
    assert_eq!(state["enabled"], false);
    assert!(state["port"].as_u64().expect("port") > 0);
    assert_eq!(state["addresses"], serde_json::json!([]));
    assert!(supervisor.is_running().await);

    supervisor.stop().await;
    let state = supervisor.state().await;
    assert_eq!(state["enabled"], false);
    assert!(state["port"].is_null());
    assert!(!supervisor.is_running().await);
}

#[tokio::test]
async fn set_enabled_persists_after_success_and_manages_pair_code() {
    let dir = TempDir::new("enable");
    let log = Arc::new(Mutex::new(Vec::new()));
    let supervisor = supervisor(&dir, Arc::clone(&log));

    let state = supervisor.set_enabled(true).await.expect("enable");
    assert_eq!(state["enabled"], true);
    assert!(state["port"].as_u64().expect("port") > 0);

    let state = supervisor.create_pair_code().await.expect("pair code");
    let code = state["pairCode"]["code"].as_str().expect("code");
    assert_eq!(code.len(), 6, "{code}");

    let state = supervisor.set_enabled(false).await.expect("disable");
    assert_eq!(state["enabled"], false);
    assert!(state["port"].is_null());
    assert!(state["pairCode"].is_null());
    assert_eq!(*log.lock().expect("log"), vec![true, false]);
}

#[tokio::test]
async fn on_did_change_emits_snapshot_and_stops_after_dispose() {
    let dir = TempDir::new("events");
    let supervisor = supervisor(&dir, Arc::new(Mutex::new(Vec::new())));
    let frames = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let sink = Arc::clone(&frames);
    let subscription = supervisor.subscribe_changes(Arc::new(move |value| {
        if let Value::Json(text) = value {
            sink.lock()
                .expect("frames")
                .push(serde_json::from_str(&text).expect("state JSON"));
        }
    }));
    // 首个订阅者拿到当前快照（事件循环下一拍投递，不阻塞 listen 调用）。
    tokio::task::yield_now().await;
    assert_eq!(frames.lock().expect("frames").len(), 1);

    supervisor.set_enabled(true).await.expect("enable");
    {
        let frames = frames.lock().expect("frames");
        assert_eq!(frames.len(), 2, "{frames:?}");
        assert_eq!(frames[1]["enabled"], true);
    }
    let before = frames.lock().expect("frames").len();
    subscription();
    supervisor.set_enabled(false).await.expect("disable");
    assert_eq!(frames.lock().expect("frames").len(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn port_trait_bridges_from_blocking_thread() {
    use host_channels::lan_access::LanAccessHostPort;

    let dir = TempDir::new("port");
    let supervisor = supervisor(&dir, Arc::new(Mutex::new(Vec::new())));
    let port: Arc<dyn LanAccessHostPort> = Arc::clone(&supervisor) as Arc<dyn LanAccessHostPort>;

    // 频道侧端口调用发生在 spawn_blocking 线程：block_on 不能 panic。
    let port_clone = Arc::clone(&port);
    let enabled = tokio::task::spawn_blocking(move || port_clone.set_enabled(true))
        .await
        .expect("blocking task");
    assert!(enabled.is_ok(), "{enabled:?}");

    let state = tokio::task::spawn_blocking(move || port.state())
        .await
        .expect("blocking task");
    let state = state.expect("state");
    assert_eq!(state["enabled"], true);
    assert!(state["port"].as_u64().expect("port") > 0);
}
