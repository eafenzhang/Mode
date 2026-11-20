//! LT-8 P1 端到端：真实 `agent-server` 子进程 + TUI stdio 客户端 + sessions-index
//! live 增量。验证 spawn/订阅/请求/通知归约整条链路，而不只是纯函数。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mode_tui::{apply_agent_notification, AgentClient, TuiModel};
use serde_json::json;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-tui-agent-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp root");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[test]
fn tui_client_receives_live_sessions_index_over_stdio() {
    let root = TempRoot::new("live");
    let workspace = root.path().join("ws");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let workspace = workspace.display().to_string();
    let binary = env!("CARGO_BIN_EXE_agent-server");
    let agent = AgentClient::spawn_with_env(
        binary,
        &["app-server".to_string(), "--stdio".to_string()],
        &[(
            "MODE_DATA_BASE_DIR".to_string(),
            root.path().display().to_string(),
        )],
    )
    .expect("spawn agent-server");

    agent
        .request(
            "v4/conversation/subscribe",
            json!({
                "topic": format!("sessions-index/{workspace}"),
                "connectionId": "tui-test",
                "clientMode": "desktop-continuous",
            }),
        )
        .expect("subscribe");
    // 等 ACK 写出、live 转发任务挂上广播，再新建会话。
    std::thread::sleep(Duration::from_millis(500));
    agent
        .request(
            "session/create",
            json!({
                "sessionId": "sess_tui_live",
                "workspace": {
                    "workspacePath": workspace,
                    "workspaceKey": workspace,
                },
            }),
        )
        .expect("create session");

    let mut model = TuiModel::new(&workspace);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        while let Some(message) = agent.try_recv() {
            apply_agent_notification(&mut model, &message, now_ms());
        }
        if model
            .sessions
            .iter()
            .any(|session| session.id == "sess_tui_live")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        model
            .sessions
            .iter()
            .any(|session| session.id == "sess_tui_live"),
        "expected live session, got {:?}",
        model.sessions
    );
}
