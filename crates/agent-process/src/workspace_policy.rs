//! workspace 策略写路径 facade。
//!
//! 行为真源：`packages/services/src/mode-agent/modeAgentService.ts` 的四组方法；
//! 协议镜像见 `agent_protocol::workspace_policy`。

use agent_protocol::session::WorkspaceRef;
use agent_protocol::workspace_policy::{
    InteractionPreferences, ModelIoPreferences, WorkspaceUpdateDynamicWorkflowPolicyParams,
    WorkspaceUpdateDynamicWorkflowPolicyResult, WorkspaceUpdateInteractionPreferencesParams,
    WorkspaceUpdateInteractionPreferencesResult, WorkspaceUpdateModelIoPreferencesParams,
    WorkspaceUpdateModelIoPreferencesResult, WorkspaceUpdateOffPeakToolPolicyParams,
    WorkspaceUpdateOffPeakToolPolicyResult, WORKSPACE_UPDATE_DYNAMIC_WORKFLOW_POLICY_METHOD,
    WORKSPACE_UPDATE_INTERACTION_PREFERENCES_METHOD, WORKSPACE_UPDATE_MODEL_IO_PREFERENCES_METHOD,
    WORKSPACE_UPDATE_OFF_PEAK_TOOL_POLICY_METHOD,
};
use mode_protocol_client::ClientError;

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};

/// `ModeAgentAppRuntimePreferences`：App 级运行偏好，广播到所有活 workspace。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AppRuntimePreferences {
    pub ask_user_question_auto_resolution_enabled: bool,
    pub model_io_full_retention_enabled: Option<bool>,
}

impl AgentControlClient {
    /// `workspace/updateInteractionPreferences`。
    pub async fn update_workspace_interaction_preferences(
        &self,
        target: &AgentWorkspaceTarget,
        params: WorkspaceUpdateInteractionPreferencesParams,
    ) -> Result<WorkspaceUpdateInteractionPreferencesResult, ControlError> {
        request_workspace_policy(
            self,
            target,
            WORKSPACE_UPDATE_INTERACTION_PREFERENCES_METHOD,
            &params,
        )
        .await
    }

    /// `workspace/updateModelIoPreferences`。
    pub async fn update_workspace_model_io_preferences(
        &self,
        target: &AgentWorkspaceTarget,
        params: WorkspaceUpdateModelIoPreferencesParams,
    ) -> Result<WorkspaceUpdateModelIoPreferencesResult, ControlError> {
        request_workspace_policy(
            self,
            target,
            WORKSPACE_UPDATE_MODEL_IO_PREFERENCES_METHOD,
            &params,
        )
        .await
    }

    /// `workspace/updateOffPeakToolPolicy`。
    pub async fn update_workspace_off_peak_tool_policy(
        &self,
        target: &AgentWorkspaceTarget,
        params: WorkspaceUpdateOffPeakToolPolicyParams,
    ) -> Result<WorkspaceUpdateOffPeakToolPolicyResult, ControlError> {
        request_workspace_policy(
            self,
            target,
            WORKSPACE_UPDATE_OFF_PEAK_TOOL_POLICY_METHOD,
            &params,
        )
        .await
    }

    /// `workspace/updateDynamicWorkflowPolicy`。
    pub async fn update_workspace_dynamic_workflow_policy(
        &self,
        target: &AgentWorkspaceTarget,
        params: WorkspaceUpdateDynamicWorkflowPolicyParams,
    ) -> Result<WorkspaceUpdateDynamicWorkflowPolicyResult, ControlError> {
        request_workspace_policy(
            self,
            target,
            WORKSPACE_UPDATE_DYNAMIC_WORKFLOW_POLICY_METHOD,
            &params,
        )
        .await
    }

    /// `syncAppRuntimePreferences`：把 App 级偏好广播到所有活 workspace。
    ///
    /// 顺序与 TS 一致：先 interaction，再 model IO；model IO 的 `-32601` 只降级忽略
    /// （旧 CLI），其它错误照常上抛。不同 workspace 串行发送（TS 是并行队列，
    /// 结果语义与最终状态一致）。
    pub async fn sync_app_runtime_preferences(
        &self,
        preferences: AppRuntimePreferences,
    ) -> Result<(), ControlError> {
        let full_retention = preferences.model_io_full_retention_enabled == Some(true);
        for target in self.active_workspace_targets() {
            let workspace = WorkspaceRef {
                workspace_path: target.workspace_path.clone(),
                workspace_key: target.workspace_key(),
                workspace_identity: target.workspace_identity.clone(),
                remote_session_id: None,
                extra: Default::default(),
            };
            self.update_workspace_interaction_preferences(
                &target,
                WorkspaceUpdateInteractionPreferencesParams {
                    workspace: workspace.clone(),
                    preferences: InteractionPreferences {
                        ask_user_question_auto_resolution_enabled: preferences
                            .ask_user_question_auto_resolution_enabled,
                    },
                },
            )
            .await?;
            let model_io = self
                .update_workspace_model_io_preferences(
                    &target,
                    WorkspaceUpdateModelIoPreferencesParams {
                        workspace,
                        preferences: ModelIoPreferences {
                            full_retention_enabled: full_retention,
                        },
                    },
                )
                .await;
            if let Err(ControlError::Client(ClientError::Server { code: -32601, .. })) = model_io {
                continue;
            }
            model_io?;
        }
        Ok(())
    }
}

async fn request_workspace_policy<TParams, TResult>(
    client: &AgentControlClient,
    target: &AgentWorkspaceTarget,
    method: &'static str,
    params: &TParams,
) -> Result<TResult, ControlError>
where
    TParams: serde::Serialize,
    TResult: serde::de::DeserializeOwned,
{
    let process = client
        .client_for(target, RuntimePolicy::StartIfNeeded)
        .await?;
    let value = serde_json::to_value(params).map_err(|error| {
        ControlError::InvalidParams(format!("{method} params serialization failed: {error}"))
    })?;
    let response = process.client().request(method, Some(value)).await?;
    serde_json::from_value::<TResult>(response)
        .map_err(|error| ControlError::InvalidResult(format!("invalid {method} result: {error}")))
}
