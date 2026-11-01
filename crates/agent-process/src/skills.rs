//! 技能引用 catalog facade：`skills/referenceCatalog`。
//!
//! 行为真源：`packages/services/src/mode-agent/modeAgentService.ts` 的同名方法；
//! 协议镜像见 `agent_protocol::skills`。

use agent_protocol::skills::{
    SkillsReferenceCatalogParams, SkillsReferenceCatalogResult, SKILLS_REFERENCE_CATALOG_METHOD,
};

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};
use crate::session_read::session_workspace_ref;

impl AgentControlClient {
    /// `skills/referenceCatalog`：带 sessionId 时读该 Session 冻结的目录，
    /// 不带时读 workspace 当前目录。
    pub async fn skill_reference_catalog(
        &self,
        target: &AgentWorkspaceTarget,
        session_id: Option<&str>,
    ) -> Result<SkillsReferenceCatalogResult, ControlError> {
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
        let params = SkillsReferenceCatalogParams {
            workspace: session_workspace_ref(target),
            session_id: session_id.map(str::to_string),
        };
        let value =
            serde_json::to_value(&params).expect("skills/referenceCatalog params 序列化不会失败");
        let response = process
            .client()
            .request(SKILLS_REFERENCE_CATALOG_METHOD, Some(value))
            .await?;
        let result: SkillsReferenceCatalogResult =
            serde_json::from_value(response).map_err(|error| {
                ControlError::InvalidResult(format!(
                    "invalid skills/referenceCatalog result: {error}"
                ))
            })?;
        result.validate().map_err(ControlError::InvalidResult)?;
        Ok(result)
    }
}
