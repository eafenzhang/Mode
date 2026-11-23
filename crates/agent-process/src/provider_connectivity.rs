//! `provider/testModelConnectivity`：设置页「测试连通性」的 agent 协议面。
//!
//! 真源：`packages/services/src/mode-agent/modeAgentService.ts` 的
//! `testModelConnectivity`；只向前端返回 `{success:true}`，失败以请求错误上抛。

use model_selection::ModelSelection;
use serde::Serialize;
use serde_json::Value;

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};
use crate::session_read::session_workspace_ref;

const PROVIDER_TEST_MODEL_CONNECTIVITY_METHOD: &str = "provider/testModelConnectivity";

#[derive(Serialize)]
struct TestModelConnectivityParams<'a> {
    workspace: agent_protocol::session_read::SessionWorkspaceRef,
    selection: &'a ModelSelection,
}

impl AgentControlClient {
    pub async fn test_model_connectivity(
        &self,
        target: &AgentWorkspaceTarget,
        selection: &ModelSelection,
    ) -> Result<(), ControlError> {
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
        let params = TestModelConnectivityParams {
            workspace: session_workspace_ref(target),
            selection,
        };
        let params = serde_json::to_value(&params)
            .expect("provider/testModelConnectivity params 序列化不会失败");
        let response = process
            .client()
            .request(PROVIDER_TEST_MODEL_CONNECTIVITY_METHOD, Some(params))
            .await?;
        if !matches!(response.get("success"), Some(Value::Bool(true))) {
            return Err(ControlError::InvalidResult(
                "invalid provider/testModelConnectivity result: success must be true".to_string(),
            ));
        }
        Ok(())
    }
}
