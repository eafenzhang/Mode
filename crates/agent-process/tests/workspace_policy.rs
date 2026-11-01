//! workspace 策略写路径：参数形状、结果解析与严格性。

use std::path::PathBuf;
use std::sync::Arc;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget,
    AppRuntimePreferences, ControlError, SpecFactory,
};
use agent_protocol::session::WorkspaceRef;
use agent_protocol::workspace_policy::{
    InteractionPreferences, ModelIoPreferences, WorkspaceUpdateDynamicWorkflowPolicyParams,
    WorkspaceUpdateInteractionPreferencesParams, WorkspaceUpdateModelIoPreferencesParams,
    WorkspaceUpdateOffPeakToolPolicyParams,
};
use serde_json::{json, Value};

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mode-workspace-policy-{label}-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    fn frame_params_all(&self, method: &str) -> Vec<Value> {
        let marker = format!("frame	{method}	");
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(&self.0) {
                let params: Vec<Value> = text
                    .lines()
                    .filter(|line| line.starts_with(&marker))
                    .filter_map(|line| line.splitn(3, '\t').nth(2)?.parse().ok())
                    .collect();
                if !params.is_empty() {
                    return params;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Vec::new()
    }

    fn frame_params(&self, method: &str) -> Option<Value> {
        let marker = format!("frame\t{method}\t");
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(&self.0) {
                if let Some(line) = text.lines().find(|line| line.starts_with(&marker)) {
                    return line.splitn(3, '\t').nth(2)?.parse().ok();
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        None
    }
}

impl Drop for FrameLog {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn control(mode: &str, log: &FrameLog) -> AgentControlClient {
    let mode = mode.to_string();
    let log = log.0.to_string_lossy().into_owned();
    let factory: SpecFactory = Arc::new(move |_target| {
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

fn target() -> AgentWorkspaceTarget {
    AgentWorkspaceTarget::new("/ws/demo")
}

fn workspace() -> WorkspaceRef {
    WorkspaceRef {
        workspace_path: "/ws/demo".to_string(),
        workspace_key: "/ws/demo".to_string(),
        workspace_identity: None,
        remote_session_id: None,
        extra: Default::default(),
    }
}

#[tokio::test]
async fn all_four_policy_methods_send_workspace_and_parse_results() {
    let log = FrameLog::new("ok");
    let client = control("", &log);

    let interaction = client
        .update_workspace_interaction_preferences(
            &target(),
            WorkspaceUpdateInteractionPreferencesParams {
                workspace: workspace(),
                preferences: InteractionPreferences {
                    ask_user_question_auto_resolution_enabled: true,
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(interaction.snoozed_interaction_count, 2);
    assert!(interaction.ask_user_question_auto_resolution_enabled);

    let model_io = client
        .update_workspace_model_io_preferences(
            &target(),
            WorkspaceUpdateModelIoPreferencesParams {
                workspace: workspace(),
                preferences: ModelIoPreferences {
                    full_retention_enabled: false,
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(model_io.updated_session_count, 3);
    assert!(!model_io.full_retention_enabled);

    let off_peak = client
        .update_workspace_off_peak_tool_policy(
            &target(),
            WorkspaceUpdateOffPeakToolPolicyParams {
                workspace: workspace(),
                enabled: true,
            },
        )
        .await
        .unwrap();
    assert!(off_peak.enabled);

    let workflow = client
        .update_workspace_dynamic_workflow_policy(
            &target(),
            WorkspaceUpdateDynamicWorkflowPolicyParams {
                workspace: workspace(),
                enabled: true,
            },
        )
        .await
        .unwrap();
    assert!(workflow.enabled);
    assert_eq!(workflow.workspace.workspace_key, "/ws/demo");

    assert_eq!(
        log.frame_params("workspace/updateInteractionPreferences"),
        Some(json!({
            "workspace": { "workspacePath": "/ws/demo", "workspaceKey": "/ws/demo" },
            "preferences": { "askUserQuestionAutoResolutionEnabled": true }
        }))
    );
    assert_eq!(
        log.frame_params("workspace/updateOffPeakToolPolicy"),
        Some(json!({
            "workspace": { "workspacePath": "/ws/demo", "workspaceKey": "/ws/demo" },
            "enabled": true
        }))
    );
}

#[tokio::test]
async fn malformed_policy_result_is_rejected() {
    let log = FrameLog::new("bad");
    let client = control("bad-workspace-policy", &log);
    let error = match client
        .update_workspace_dynamic_workflow_policy(
            &target(),
            WorkspaceUpdateDynamicWorkflowPolicyParams {
                workspace: workspace(),
                enabled: true,
            },
        )
        .await
    {
        Ok(_) => panic!("未知字段必须拒绝"),
        Err(error) => error,
    };
    assert!(matches!(error, ControlError::InvalidResult(_)), "{error:?}");
}

#[tokio::test]
async fn sync_app_runtime_preferences_fans_out_to_active_workspaces() {
    let log = FrameLog::new("sync");
    let client = control("", &log);
    // 没有活进程时不发请求。
    client
        .sync_app_runtime_preferences(AppRuntimePreferences {
            ask_user_question_auto_resolution_enabled: true,
            model_io_full_retention_enabled: Some(true),
        })
        .await
        .unwrap();
    assert!(log
        .frame_params_all("workspace/updateInteractionPreferences")
        .is_empty());

    client
        .read_workspace_presentation(&AgentWorkspaceTarget::new("/ws/a"))
        .await
        .unwrap();
    client
        .read_workspace_presentation(&AgentWorkspaceTarget::new("/ws/b"))
        .await
        .unwrap();

    client
        .sync_app_runtime_preferences(AppRuntimePreferences {
            ask_user_question_auto_resolution_enabled: true,
            model_io_full_retention_enabled: Some(true),
        })
        .await
        .unwrap();

    let interaction = log.frame_params_all("workspace/updateInteractionPreferences");
    let model_io = log.frame_params_all("workspace/updateModelIoPreferences");
    assert_eq!(interaction.len(), 2);
    assert_eq!(model_io.len(), 2);
    assert!(interaction.iter().all(|frame| {
        frame["preferences"]["askUserQuestionAutoResolutionEnabled"] == json!(true)
    }));
    assert!(model_io
        .iter()
        .all(|frame| frame["preferences"]["fullRetentionEnabled"] == json!(true)));
}

#[tokio::test]
async fn sync_app_runtime_preferences_ignores_method_not_found_for_model_io() {
    let log = FrameLog::new("sync-old");
    let client = control("old-model-io", &log);
    client
        .read_workspace_presentation(&AgentWorkspaceTarget::new("/ws/a"))
        .await
        .unwrap();
    client
        .sync_app_runtime_preferences(AppRuntimePreferences {
            ask_user_question_auto_resolution_enabled: false,
            model_io_full_retention_enabled: Some(false),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn sync_app_runtime_preferences_propagates_real_model_io_errors() {
    let log = FrameLog::new("sync-bad");
    let client = control("bad-model-io", &log);
    client
        .read_workspace_presentation(&AgentWorkspaceTarget::new("/ws/a"))
        .await
        .unwrap();
    let error = client
        .sync_app_runtime_preferences(AppRuntimePreferences {
            ask_user_question_auto_resolution_enabled: true,
            model_io_full_retention_enabled: None,
        })
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            ControlError::Client(mode_protocol_client::ClientError::Server { code: -32600, .. })
        ),
        "{error:?}"
    );
}
