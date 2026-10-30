//! workspace 只读面 facade：首屏展示与子进程清单。
//!
//! 行为真源：`packages/services/src/mode-agent/modeAgentService.ts` 的
//! `readWorkspacePresentation`（含「transport 已关闭时换 client 重试一次」）与
//! `collectLocalRuntimeChildProcesses`。

use agent_protocol::workspace::{
    ChildProcess, ChildProcessesResult, WorkspacePresentation, WorkspacePresentationParams,
    PROCESS_CHILD_PROCESSES_METHOD, WORKSPACE_READ_PRESENTATION_METHOD,
};
use mode_protocol_client::ClientError;

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};
use crate::session_read::session_workspace_ref;

impl AgentControlClient {
    /// `workspace/readPresentation`：workspace 模式 + slash commands。
    ///
    /// 配置更新可能正好重启该 workspace 的 Agent：旧 client 已取得但尚未发请求时 transport
    /// 被关闭，这里重新取一次 client 再试（与 TS 的 closed-transport 重试一致）。
    pub async fn read_workspace_presentation(
        &self,
        target: &AgentWorkspaceTarget,
    ) -> Result<WorkspacePresentation, ControlError> {
        let params = WorkspacePresentationParams {
            workspace: session_workspace_ref(target),
        };
        let params =
            serde_json::to_value(&params).expect("workspace/readPresentation params 序列化不会失败");
        for attempt in 0..2 {
            let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
            match process
                .client()
                .request(WORKSPACE_READ_PRESENTATION_METHOD, Some(params.clone()))
                .await
            {
                Ok(response) => {
                    return serde_json::from_value(response).map_err(|error| {
                        ControlError::InvalidResult(format!(
                            "invalid workspace/readPresentation result: {error}"
                        ))
                    });
                }
                Err(error)
                    if attempt == 0
                        && matches!(error, ClientError::Closed | ClientError::Transport(_)) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(ControlError::InvalidResult(
            "workspace/readPresentation retry exhausted".to_string(),
        ))
    }

    /// `process/childProcesses`：当前 runtime 的 MCP 子进程清单。
    ///
    /// TS 的 `collectLocalRuntimeChildProcesses` 会遍历本 Host 内全部 runtime（含 plugin /
    /// mcp-status 泳道）；本 facade 只覆盖单个 workspace runtime。
    pub async fn list_child_processes(
        &self,
        target: &AgentWorkspaceTarget,
    ) -> Result<Vec<ChildProcess>, ControlError> {
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let response = process
            .client()
            .request(PROCESS_CHILD_PROCESSES_METHOD, None)
            .await?;
        let result: ChildProcessesResult = serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid process/childProcesses result: {error}"))
        })?;
        Ok(result.processes)
    }
}
