//! 已保存工作流只读面 facade。
//!
//! 行为真源：`packages/services/src/mode-agent/modeAgentService.ts` 的
//! `listSavedWorkflows` / `getSavedWorkflow` / `listSavedWorkflowRuns`。
//!
//! 差异：全局档（`scope: global`）的「载体运行时」由 TS 自行挑选（可能回落到 plugin 管理面
//! 进程）；本 facade 直接使用调用方给的 workspace target 作为载体，不另起管理进程。

use agent_protocol::workflow::{
    SavedWorkflowScope, WorkflowsGetParams, WorkflowsGetResult, WorkflowsListParams,
    WorkflowsListResult, WorkflowsRunsParams, WorkflowsRunsResult, WORKFLOWS_GET_METHOD,
    WORKFLOWS_LIST_METHOD, WORKFLOWS_RUNS_MAX_LIMIT, WORKFLOWS_RUNS_METHOD,
};

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};
use crate::session_read::session_workspace_ref;

impl AgentControlClient {
    /// `workflows/list`：扫描 workspace（或全局档）下的已保存工作流。
    pub async fn list_saved_workflows(
        &self,
        target: &AgentWorkspaceTarget,
        scope: Option<SavedWorkflowScope>,
    ) -> Result<WorkflowsListResult, ControlError> {
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let params = WorkflowsListParams {
            workspace: session_workspace_ref(target),
            scope,
        };
        let params =
            serde_json::to_value(&params).expect("workflows/list params 序列化不会失败");
        let response = process
            .client()
            .request(WORKFLOWS_LIST_METHOD, Some(params))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid workflows/list result: {error}"))
        })
    }

    /// `workflows/get`：返回 `Ok`（脚本本体）或 `Failure`（四态原因）。
    pub async fn get_saved_workflow(
        &self,
        target: &AgentWorkspaceTarget,
        name: &str,
        scope: Option<SavedWorkflowScope>,
    ) -> Result<WorkflowsGetResult, ControlError> {
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let params = WorkflowsGetParams {
            workspace: session_workspace_ref(target),
            name: name.to_string(),
            scope,
        };
        let params = serde_json::to_value(&params).expect("workflows/get params 序列化不会失败");
        let response = process
            .client()
            .request(WORKFLOWS_GET_METHOD, Some(params))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid workflows/get result: {error}"))
        })
    }

    /// `workflows/runs`：运行历史；`limit` 必须落在 1..=50（协议限制）。
    pub async fn list_saved_workflow_runs(
        &self,
        target: &AgentWorkspaceTarget,
        name: Option<&str>,
        limit: u64,
        scope: Option<SavedWorkflowScope>,
    ) -> Result<WorkflowsRunsResult, ControlError> {
        if limit == 0 || limit > WORKFLOWS_RUNS_MAX_LIMIT {
            return Err(ControlError::InvalidParams(format!(
                "workflows/runs limit must be in 1..={WORKFLOWS_RUNS_MAX_LIMIT}"
            )));
        }
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let params = WorkflowsRunsParams {
            workspace: session_workspace_ref(target),
            name: name.map(str::to_string),
            limit,
            scope,
        };
        let params =
            serde_json::to_value(&params).expect("workflows/runs params 序列化不会失败");
        let response = process
            .client()
            .request(WORKFLOWS_RUNS_METHOD, Some(params))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid workflows/runs result: {error}"))
        })
    }
}
