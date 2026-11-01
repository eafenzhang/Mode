//! workspace 策略写路径 facade。
//!
//! 行为真源：`packages/services/src/mode-agent/modeAgentService.ts` 的四组方法；
//! 协议镜像见 `agent_protocol::workspace_policy`。

use agent_protocol::workspace_policy::{
    WorkspaceUpdateDynamicWorkflowPolicyParams, WorkspaceUpdateDynamicWorkflowPolicyResult,
    WorkspaceUpdateInteractionPreferencesParams, WorkspaceUpdateInteractionPreferencesResult,
    WorkspaceUpdateModelIoPreferencesParams, WorkspaceUpdateModelIoPreferencesResult,
    WorkspaceUpdateOffPeakToolPolicyParams, WorkspaceUpdateOffPeakToolPolicyResult,
    WORKSPACE_UPDATE_DYNAMIC_WORKFLOW_POLICY_METHOD,
    WORKSPACE_UPDATE_INTERACTION_PREFERENCES_METHOD, WORKSPACE_UPDATE_MODEL_IO_PREFERENCES_METHOD,
    WORKSPACE_UPDATE_OFF_PEAK_TOOL_POLICY_METHOD,
};

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};

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
