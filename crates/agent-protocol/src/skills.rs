//! 技能对话引用 catalog：`skills/referenceCatalog` 的严格镜像。
//!
//! 真源：`packages/shared/src/mode-protocol/index.ts` 的
//! `modeSkillReferenceCatalogEntrySchema` / `modeSkillsReferenceCatalogParamsSchema` /
//! `modeSkillsReferenceCatalogResultSchema`。
//!
//! 该协议只承载 Composer 的只读引用投影：带 `sessionId` 时读 AgentRuntime 首次 context
//! 冻结的发现结果（未知 Session fail closed），不带时读 workspace 当前目录；
//! 不替代 Settings 的技能管理接口，也不持久化 runtime 快照。

use serde::{Deserialize, Serialize};

use crate::session_read::SessionWorkspaceRef;

pub const SKILLS_REFERENCE_CATALOG_METHOD: &str = "skills/referenceCatalog";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillReferenceScope {
    #[serde(rename = "workspace")]
    Workspace,
    #[serde(rename = "user")]
    User,
    #[serde(rename = "plugin")]
    Plugin,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillReferenceCatalogEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub path: String,
    pub scope: SkillReferenceScope,
    /// 协议是 `z.literal(true)`：目录只投影已启用的技能。
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsReferenceCatalogParams {
    pub workspace: SessionWorkspaceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillReferenceAuthority {
    #[serde(rename = "session")]
    Session,
    #[serde(rename = "workspace")]
    Workspace,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsReferenceCatalogResult {
    pub authority: SkillReferenceAuthority,
    pub skills: Vec<SkillReferenceCatalogEntry>,
}

impl SkillsReferenceCatalogResult {
    /// 与 `z.literal(true)` 对齐：disabled 条目不应出现在目录里。
    pub fn validate(&self) -> Result<(), String> {
        if let Some(entry) = self.skills.iter().find(|entry| !entry.enabled) {
            return Err(format!(
                "skills/referenceCatalog returned disabled skill: {}",
                entry.id
            ));
        }
        Ok(())
    }
}
