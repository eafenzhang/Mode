//! `mcp/list`：参数形状、状态解析与非法结果。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget, ControlError,
    ListMcpServersOptions, SpecFactory,
};
use agent_protocol::mcp::{McpListMode, McpServerStatusKind};
use serde_json::Value;

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mode-mcp-{label}-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    fn frame_params(&self, method: &str) -> Option<Value> {
        self.frames(method).into_iter().next()
    }

    fn frames(&self, method: &str) -> Vec<Value> {
        let marker = format!("frame\t{method}\t");
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(&self.0) {
                let frames: Vec<Value> = text
                    .lines()
                    .filter(|line| line.starts_with(&marker))
                    .filter_map(|line| line.splitn(3, '\t').nth(2)?.parse().ok())
                    .collect();
                if !frames.is_empty() {
                    return frames;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Vec::new()
    }
}

impl Drop for FrameLog {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn control(mode: &str, log: &FrameLog, spawns: Arc<AtomicUsize>) -> AgentControlClient {
    let mode = mode.to_string();
    let log = log.0.to_string_lossy().into_owned();
    let factory: SpecFactory = Arc::new(move |_target| {
        spawns.fetch_add(1, Ordering::SeqCst);
        let mut spec = AgentProcessSpec::new(FIXTURE);
        if !mode.is_empty() {
            spec.args.push(mode.clone());
        }
        spec.env
            .push(("AGENT_FIXTURE_FRAME_LOG".to_string(), log.clone()));
        Ok(spec)
    });
    AgentControlClient::new(Arc::new(AgentRuntimeRegistry::new(factory)))
}

#[tokio::test]
async fn lists_statuses_with_requested_mode() {
    let log = FrameLog::new("status");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);

    let result = client
        .list_mcp_server_statuses(
            &AgentWorkspaceTarget::new("/ws/demo"),
            ListMcpServersOptions {
                mode: Some(McpListMode::Status),
                ..ListMcpServersOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(result.statuses.len(), 1);
    assert_eq!(
        result.statuses["mcp-1"].status,
        McpServerStatusKind::Connected
    );
    assert_eq!(result.statuses["mcp-1"].tool_count, 3);

    let params = log.frame_params("mcp/list").expect("必须记录到请求帧");
    assert_eq!(params["workspace"]["workspacePath"], "/ws/demo");
    assert_eq!(params["mode"], "status");
    assert!(params.get("mcpServers").is_none());
}

#[tokio::test]
async fn omitted_mode_is_not_serialized() {
    let log = FrameLog::new("omitted");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);

    client
        .list_mcp_server_statuses(
            &AgentWorkspaceTarget::new("/ws/demo"),
            ListMcpServersOptions::default(),
        )
        .await
        .unwrap();

    let params = log.frame_params("mcp/list").expect("必须记录到请求帧");
    assert!(params.get("mode").is_none(), "mode 缺省时不能出现键");
    assert!(params.get("mcpServers").is_none());
}

#[tokio::test]
async fn unknown_result_fields_are_rejected() {
    let log = FrameLog::new("bad-result");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("bad-mcp-list", &log, spawns);

    let error = client
        .list_mcp_server_statuses(
            &AgentWorkspaceTarget::new("/ws/demo"),
            ListMcpServersOptions::default(),
        )
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => {
            assert!(message.contains("invalid mcp/list result"), "{message}");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn retries_without_mode_when_old_agent_rejects_it() {
    let log = FrameLog::new("reject-mode");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("mcp-reject-mode-once", &log, spawns);

    let result = client
        .list_mcp_server_statuses_compat(
            &AgentWorkspaceTarget::new("/ws/demo"),
            ListMcpServersOptions {
                mode: Some(McpListMode::Connect),
                ..ListMcpServersOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(result.statuses.len(), 1);

    let frames = log.frames("mcp/list");
    assert_eq!(frames.len(), 2, "应重试一次");
    assert_eq!(frames[0]["mode"], "connect");
    assert!(frames[1].get("mode").is_none());
}

#[tokio::test]
async fn status_mode_rejection_returns_stable_error_without_retry() {
    let log = FrameLog::new("status-unsupported");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("mcp-reject-mode", &log, spawns);

    let error = client
        .list_mcp_server_statuses_compat(
            &AgentWorkspaceTarget::new("/ws/demo"),
            ListMcpServersOptions {
                mode: Some(McpListMode::Status),
                ..ListMcpServersOptions::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ControlError::McpStatusModeUnsupported));
    assert_eq!(log.frames("mcp/list").len(), 1, "status 不支持时不得降级重试");
}

#[tokio::test]
async fn retries_without_mcp_servers_when_rejected() {
    use agent_protocol::session_create::{McpServer, McpServerStdio};

    let log = FrameLog::new("reject-servers");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("mcp-reject-servers-once", &log, spawns);

    client
        .list_mcp_server_statuses_compat(
            &AgentWorkspaceTarget::new("/ws/demo"),
            ListMcpServersOptions {
                mcp_servers: Some(vec![McpServer::Stdio(McpServerStdio {
                    name: "m".to_string(),
                    command: "cmd".to_string(),
                    args: Vec::new(),
                    env: Vec::new(),
                    isolation: None,
                    protocol_version: None,
                    timeout_ms: None,
                })]),
                mode: None,
            },
        )
        .await
        .unwrap();

    let frames = log.frames("mcp/list");
    assert_eq!(frames.len(), 2);
    assert!(frames[0].get("mcpServers").is_some());
    assert!(frames[1].get("mcpServers").is_none());
}
