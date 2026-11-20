//! Skills 会话面（C4 T2）：`skills/referenceCatalog` 与 `Skill` 工具。
//!
//! 观察基线：`docs/specs/agent-c4-contract.md` §2 与 `fixtures/c4/skills-*.json`。
//!
//! - 目录：`.mode/skills/**`（workspace）+ 插件 skill；`.claude/skills` 不发现（黑盒对照）。
//! - 会话 authority：未知/非活动 session fail closed（`-32004 Session is not active: <id>`）。
//! - `Skill` 工具：`{skill:"<name>"}` → `<skill_content name="…">` 正文，结果带
//!   `skillMetadata:{source:"mode"}`；未找到的失败形状 T0 未观察，按工具执行失败返回。
//!
//! 注入：T0 的首请求观察是「目录可见 ≠ 首请求注入」（没有合成 user skills 提醒）；
//! 本波只提供目录与工具，不伪造未观察的注入时序（登记在契约 §2.5）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_core::{PluginToolDeclaration, PluginToolError, PluginToolSource};
use agent_protocol::skills::{
    SkillReferenceAuthority, SkillReferenceCatalogEntry, SkillReferenceScope,
    SkillsReferenceCatalogResult,
};
use serde_json::{json, Value};
use skills_service::{ListParams, SkillsService, SkillsServiceOptions};

/// 目录投影：读 workspace 当前的技能发现结果（workspace/session authority 共用）。
pub struct SkillCatalog {
    service: SkillsService,
}

impl SkillCatalog {
    pub fn new(service: SkillsService) -> Self {
        Self { service }
    }

    pub fn from_process() -> Self {
        Self::new(SkillsService::new(SkillsServiceOptions::from_process()))
    }

    /// 列目录（只保留 enabled；协议要求 `enabled:true`）。
    pub fn catalog(
        &self,
        workspace_path: &str,
        authority: SkillReferenceAuthority,
    ) -> Result<SkillsReferenceCatalogResult, String> {
        let listed = self.service.list(&ListParams {
            workspace_path: PathBuf::from(workspace_path),
            workspace_identity: None,
        });
        let mut skills = Vec::new();
        for skill in listed
            .get("skills")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            if skill.get("enabled").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            let Some(name) = skill.get("name").and_then(Value::as_str) else {
                continue;
            };
            let scope = match skill.get("scope").and_then(Value::as_str) {
                Some("user") => SkillReferenceScope::User,
                Some("plugin") => SkillReferenceScope::Plugin,
                _ => SkillReferenceScope::Workspace,
            };
            skills.push(SkillReferenceCatalogEntry {
                id: skill
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or(name)
                    .to_string(),
                name: name.to_string(),
                description: skill
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                path: skill
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                scope,
                enabled: true,
                plugin_name: skill
                    .get("pluginName")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
        }
        let result = SkillsReferenceCatalogResult { authority, skills };
        result.validate()?;
        Ok(result)
    }
}

/// `Skill` 工具的动态声明与执行（C4 T2）。
pub struct SkillToolSource {
    workspace_path: PathBuf,
    service: SkillsService,
}

impl SkillToolSource {
    pub fn new(workspace_path: impl Into<PathBuf>) -> Self {
        Self::with_options(workspace_path, SkillsServiceOptions::from_process())
    }

    /// 测试/嵌入：显式 home 与 runtime 档位（避免读真实用户目录）。
    pub fn with_options(workspace_path: impl Into<PathBuf>, options: SkillsServiceOptions) -> Self {
        Self {
            workspace_path: workspace_path.into(),
            service: SkillsService::new(options),
        }
    }

    fn find_skill(&self, name: &str) -> Option<Value> {
        let listed = self.service.list(&ListParams {
            workspace_path: self.workspace_path.clone(),
            workspace_identity: None,
        });
        listed
            .get("skills")
            .and_then(Value::as_array)?
            .iter()
            .find(|skill| {
                skill.get("enabled").and_then(Value::as_bool) != Some(false)
                    && skill.get("name").and_then(Value::as_str) == Some(name)
            })
            .cloned()
    }
}

/// T0 未观察到 Skill 工具的 description/schema 原文；这里由本仓库撰写（clean-room）。
const SKILL_TOOL_DESCRIPTION: &str = "Load the full instructions of a skill available in this session. Use it when a task matches a skill listed in the skills catalog; the tool returns the SKILL.md body and its base directory.";

fn skill_tool_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "skill": {
                "type": "string",
                "description": "The skill name as listed in the skills catalog."
            }
        },
        "required": ["skill"],
        "additionalProperties": false
    })
}

impl PluginToolSource for SkillToolSource {
    fn list_tools(&self) -> Result<Vec<PluginToolDeclaration>, String> {
        Ok(vec![PluginToolDeclaration {
            name: "Skill".to_string(),
            description: SKILL_TOOL_DESCRIPTION.to_string(),
            input_schema: skill_tool_schema(),
            risk_level: Some("medium".to_string()),
            side_effect_scope: Some("session".to_string()),
            budget_strategy: Some("truncate".to_string()),
            read_only: Some(true),
            display_json: None,
        }])
    }

    fn call_tool(&self, tool_name: &str, input: &Value) -> Result<Value, PluginToolError> {
        if tool_name != "Skill" {
            return Err(PluginToolError::tool_execution_failed(format!(
                "Unknown skill tool: {tool_name}"
            )));
        }
        let name = input
            .get("skill")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if name.is_empty() {
            return Err(PluginToolError::tool_execution_failed(
                "Skill name is required",
            ));
        }
        let skill = self.find_skill(name).ok_or_else(|| {
            PluginToolError::tool_execution_failed(format!("Skill not found: {name}"))
        })?;
        let path = skill.get("path").and_then(Value::as_str).ok_or_else(|| {
            PluginToolError::tool_execution_failed(format!("Skill has no path: {name}"))
        })?;
        let raw = std::fs::read_to_string(path).map_err(|error| {
            PluginToolError::tool_execution_failed(format!("Failed to read skill {name}: {error}"))
        })?;
        let body = skills_service::read_frontmatter(&raw).body;
        let base = Path::new(path)
            .parent()
            .map(|parent| parent.display().to_string())
            .unwrap_or_default();
        let content = format!(
            "<skill_content name=\"{name}\">\n# Skill: {name}\n{body}\nBase directory for this skill: {base}\nRelative paths in this skill are relative to this base directory.\n</skill_content>"
        );
        Ok(json!({
            "content": content,
            "skillMetadata": { "source": "mode" },
        }))
    }
}

/// 多工具源的路由（Skill + MCP 等）：声明按源顺序拼接，调用按工具名分派。
pub struct CompositeToolSource {
    routes: HashMap<String, Arc<dyn PluginToolSource>>,
}

impl CompositeToolSource {
    /// 拉取每个源的声明并建立路由；任一源枚举失败则整体失败（fail closed）。
    pub fn combine(
        sources: Vec<Arc<dyn PluginToolSource>>,
    ) -> Result<(Vec<PluginToolDeclaration>, Arc<Self>), String> {
        let mut routes = HashMap::new();
        let mut declarations = Vec::new();
        for source in sources {
            for declaration in source.list_tools()? {
                routes.insert(declaration.name.clone(), Arc::clone(&source));
                declarations.push(declaration);
            }
        }
        Ok((declarations, Arc::new(Self { routes })))
    }
}

impl PluginToolSource for CompositeToolSource {
    fn list_tools(&self) -> Result<Vec<PluginToolDeclaration>, String> {
        // 声明已在 `combine` 时物化；这里保持 trait 契约（返回空表不会重复注入）。
        Ok(Vec::new())
    }

    fn call_tool(&self, tool_name: &str, input: &Value) -> Result<Value, PluginToolError> {
        let source = self.routes.get(tool_name).ok_or_else(|| {
            PluginToolError::tool_execution_failed(format!("Unknown tool: {tool_name}"))
        })?;
        source.call_tool(tool_name, input)
    }
}
