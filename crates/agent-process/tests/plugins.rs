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
use serde_json::{json, Value};

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
    assert_eq!(
        plugin.components.as_ref().unwrap()[0].kind,
        PluginComponentKind::Skill
    );
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
    assert_eq!(
        result.metadata.as_ref().unwrap().author.as_deref(),
        Some("demo")
    );

    let params = log
        .frame_params("plugins/describe")
        .expect("必须记录到请求帧");
    assert_eq!(params["marketplace"], "official");
    assert_eq!(params["pluginName"], "demo");
}

#[tokio::test]
async fn reads_plugins_overview() {
    let log = FrameLog::new("overview");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);

    let result = client
        .get_plugins_overview(
            &AgentWorkspaceTarget::new("/ws/demo"),
            Some(PluginScope::User),
        )
        .await
        .unwrap();
    assert_eq!(result.marketplaces.len(), 1);
    assert_eq!(result.marketplaces[0].plugin_count, 1);
    assert_eq!(result.available_plugins[0].id, "demo-plugin");
    assert!(result.available_plugins[0].listing.is_some());
    assert_eq!(result.installed_plugins[0].scope, PluginScope::Workspace);
    assert!(result.capability.supported);
    assert_eq!(result.official_marketplace_enabled, Some(true));

    let params = log
        .frame_params("plugins/overview")
        .expect("必须记录到请求帧");
    assert_eq!(params["workspace"]["workspacePath"], "/ws/demo");
    assert_eq!(params["configScope"], "user");
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

    let log = FrameLog::new("bad-overview");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("bad-plugins-overview", &log, spawns);
    let error = client
        .get_plugins_overview(&AgentWorkspaceTarget::new("/ws/demo"), None)
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => {
            assert!(
                message.contains("invalid plugins/overview result"),
                "{message}"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn plugin_write_methods_send_params_and_parse_results() {
    let log = FrameLog::new("writes");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, Arc::clone(&spawns));

    let enabled = client
        .set_plugin_enabled(
            &AgentWorkspaceTarget::new("/ws/demo"),
            "demo@market",
            true,
            Some("op-1"),
            Some(PluginScope::User),
        )
        .await
        .unwrap();
    assert_eq!(enabled.plugin.id, "demo@market");
    assert!(enabled.enabled);
    assert_eq!(
        log.frame_params("plugins/setEnabled"),
        Some(json!({
            "workspace": { "workspacePath": "/ws/demo", "workspaceKey": "/ws/demo" },
            "pluginId": "demo@market",
            "enabled": true,
            "operationId": "op-1",
            "scope": "user"
        }))
    );

    let configured = client
        .configure_plugin(
            &AgentWorkspaceTarget::new("/ws/demo"),
            "demo@market",
            [("token".to_string(), json!("abc"))].into_iter().collect(),
            Some(vec!["old".to_string()]),
            None,
            Some(true),
        )
        .await
        .unwrap();
    assert_eq!(configured.plugin_id, "demo@market");
    let configure_frame = log.frame_params("plugins/configure").expect("configure 帧");
    assert_eq!(configure_frame["options"], json!({ "token": "abc" }));
    assert_eq!(configure_frame["clearOptionKeys"], json!(["old"]));
    assert_eq!(configure_frame["dryRun"], json!(true));

    let reset = client
        .reset_plugin_config(&AgentWorkspaceTarget::new("/ws/demo"), "demo@market", None)
        .await
        .unwrap();
    assert_eq!(reset.plugin_id, "demo@market");

    let restored = client
        .restore_builtin_plugin(&AgentWorkspaceTarget::new("/ws/demo"), "demo@market")
        .await
        .unwrap();
    assert!(restored.diagnostics.is_empty());

    let validated = client
        .validate_plugin(
            &AgentWorkspaceTarget::new("/ws/demo"),
            Some("demo"),
            Some("market"),
            None,
        )
        .await
        .unwrap();
    assert!(validated.ok);
    assert_eq!(validated.compatibility.runnable, vec!["agent".to_string()]);
}

#[tokio::test]
async fn plugin_write_rejects_unknown_result_fields() {
    let log = FrameLog::new("writes-bad");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("bad-plugin-write", &log, spawns);
    let error = match client
        .validate_plugin(
            &AgentWorkspaceTarget::new("/ws/demo"),
            Some("demo"),
            None,
            None,
        )
        .await
    {
        Ok(_) => panic!("未知字段必须拒绝"),
        Err(error) => error,
    };
    assert!(matches!(error, ControlError::InvalidResult(_)), "{error:?}");
}

#[tokio::test]
async fn plugin_reference_catalog_prefers_category_and_falls_back_on_method_not_found() {
    let log = FrameLog::new("catalog");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, Arc::clone(&spawns));
    let catalog = client
        .plugin_reference_catalog(&AgentWorkspaceTarget::new("/ws/demo"), Some("sess-1"))
        .await
        .unwrap();
    assert_eq!(
        catalog.authority,
        agent_protocol::plugin::PluginReferenceAuthority::Workspace
    );
    assert_eq!(catalog.plugins[0].category.as_deref(), Some("productivity"));
    assert_eq!(catalog.plugins[0].subagent_names, Vec::<String>::new());
    assert_eq!(
        log.frame_params("plugins/referenceCatalogWithCategory"),
        Some(json!({
            "workspace": { "workspacePath": "/ws/demo", "workspaceKey": "/ws/demo" },
            "sessionId": "sess-1"
        }))
    );

    let fallback_log = FrameLog::new("catalog-fallback");
    let fallback_client = control("old-plugin-agent", &fallback_log, spawns);
    let fallback = fallback_client
        .plugin_reference_catalog(&AgentWorkspaceTarget::new("/ws/demo"), None)
        .await
        .unwrap();
    assert!(fallback.plugins[0].category.is_none());
    assert!(fallback_log
        .frame_params("plugins/referenceCatalog")
        .is_some());
}

#[tokio::test]
async fn suggested_reference_requires_trusted_identity_for_actionable_status() {
    let log = FrameLog::new("suggested");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, Arc::clone(&spawns));
    let ready = client
        .resolve_suggested_plugin_reference(
            &AgentWorkspaceTarget::new("/ws/demo"),
            "suggested-1",
            "op-1",
            agent_protocol::plugin::PluginDeliveryKind::DesktopContinuous,
            agent_protocol::plugin::PluginDeliveryKind::WebRemoteReplayable,
        )
        .await
        .unwrap();
    assert_eq!(
        ready.status,
        agent_protocol::plugin::PluginSuggestedReferenceStatus::Ready
    );
    assert_eq!(
        log.frame_params("plugins/resolveSuggestedReference"),
        Some(json!({
            "workspace": { "workspacePath": "/ws/demo", "workspaceKey": "/ws/demo" },
            "stableId": "suggested-1",
            "operationId": "op-1",
            "clientMode": "desktop-continuous",
            "deliveryKind": "web-remote-replayable"
        }))
    );

    let untrusted = control(
        "suggested-reference-untrusted",
        &FrameLog::new("suggested-bad"),
        spawns,
    );
    let error = match untrusted
        .resolve_suggested_plugin_reference(
            &AgentWorkspaceTarget::new("/ws/demo"),
            "suggested-1",
            "op-1",
            agent_protocol::plugin::PluginDeliveryKind::DesktopContinuous,
            agent_protocol::plugin::PluginDeliveryKind::DesktopContinuous,
        )
        .await
    {
        Ok(_) => panic!("缺可信身份必须拒绝"),
        Err(error) => error,
    };
    assert!(matches!(error, ControlError::InvalidResult(_)), "{error:?}");
}
