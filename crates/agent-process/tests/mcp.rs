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
        let marker = format!("frame\t{method}\t");
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(&self.0) {
                if let Some(line) = text.lines().find(|line| line.starts_with(&marker)) {
                    return line.splitn(3, '\t').nth(2)?.parse().ok();
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
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
