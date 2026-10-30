//! 插件只读面：list / describe 的参数、严格结果与非法结果。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget, ControlError,
    SpecFactory,
};
use agent_protocol::plugin::{PluginComponentKind, PluginScope};
use serde_json::Value;

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mode-plugins-{label}-{}-{}.log",
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
async fn lists_plugins_with_scope_and_parses_components() {
    let log = FrameLog::new("list");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);

    let result = client
        .list_plugins(
            &AgentWorkspaceTarget::new("/ws/demo"),
            Some(PluginScope::Workspace),
        )
        .await
        .unwrap();
    assert_eq!(result.plugins.len(), 1);
    let plugin = &result.plugins[0];
    assert_eq!(plugin.id, "demo-plugin");
    assert_eq!(plugin.components.as_ref().unwrap()[0].kind, PluginComponentKind::Skill);
    assert!(plugin.hook_details.as_ref().unwrap()[0].runnable);
    assert_eq!(result.diagnostics.len(), 1);

    let params = log.frame_params("plugins/list").expect("必须记录到请求帧");
    assert_eq!(params["workspace"]["workspacePath"], "/ws/demo");
    assert_eq!(params["configScope"], "workspace");
}

#[tokio::test]
async fn describes_plugin_with_component_groups() {
    let log = FrameLog::new("describe");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);

    let result = client
        .describe_plugin(&AgentWorkspaceTarget::new("/ws/demo"), "official", "demo")
        .await
        .unwrap();
    assert_eq!(result.components.len(), 1);
    assert_eq!(result.components[0].items[0].name, "run");
    assert_eq!(result.metadata.as_ref().unwrap().author.as_deref(), Some("demo"));

    let params = log.frame_params("plugins/describe").expect("必须记录到请求帧");
    assert_eq!(params["marketplace"], "official");
    assert_eq!(params["pluginName"], "demo");
}

#[tokio::test]
async fn malformed_plugin_results_are_rejected() {
    let log = FrameLog::new("bad-list");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("bad-plugins-list", &log, spawns);
    let error = client
        .list_plugins(&AgentWorkspaceTarget::new("/ws/demo"), None)
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => {
            assert!(message.contains("invalid plugins/list result"), "{message}");
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let log = FrameLog::new("bad-describe");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("bad-plugins-describe", &log, spawns);
    let error = client
        .describe_plugin(&AgentWorkspaceTarget::new("/ws/demo"), "official", "demo")
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => {
            assert!(
                message.contains("invalid plugins/describe result"),
                "{message}"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
