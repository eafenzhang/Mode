//! C4 T2：Skills 目录与 `Skill` 工具的集成测试。
//!
//! 观察基线：`docs/specs/agent-c4-contract.md` §2 与 `fixtures/c4/skills-*.json`。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use agent_core::{PluginToolDeclaration, PluginToolError, PluginToolSource};
use agent_protocol::skills::{SkillReferenceAuthority, SkillReferenceScope};
use agent_server::{CompositeToolSource, SkillCatalog, SkillToolSource};
use serde_json::{json, Value};
use skills_service::{SkillsService, SkillsServiceOptions};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-agent-server-skills-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp root");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn seed_skill(workspace: &Path, root: &str, name: &str, body: &str) -> PathBuf {
    let dir = workspace.join(root).join(name);
    fs::create_dir_all(&dir).expect("create skill dir");
    let path = dir.join("SKILL.md");
    fs::write(
        &path,
        format!("---\nname: {name}\ndescription: C4 probe skill {name}.\n---\n\n{body}\n"),
    )
    .expect("write skill");
    path
}

fn test_service(home: &Path) -> SkillsService {
    SkillsService::new(SkillsServiceOptions {
        home_dir: home.to_path_buf(),
        is_desktop_runtime: false,
    })
}

#[test]
fn catalog_discovers_workspace_skills_and_skips_claude_root() {
    let root = TempRoot::new("catalog");
    let workspace = root.path().join("ws");
    fs::create_dir_all(&workspace).expect("workspace");
    seed_skill(&workspace, ".mode/skills", "probe-workspace-skill", "Body.");
    seed_skill(&workspace, ".claude/skills", "probe-claude-skill", "Body.");

    let catalog = SkillCatalog::new(test_service(root.path()));
    let result = catalog
        .catalog(
            &workspace.display().to_string(),
            SkillReferenceAuthority::Workspace,
        )
        .expect("catalog");
    assert_eq!(result.authority, SkillReferenceAuthority::Workspace);
    let names: Vec<_> = result
        .skills
        .iter()
        .map(|skill| skill.name.as_str())
        .collect();
    assert_eq!(names, vec!["probe-workspace-skill"]);
    let entry = &result.skills[0];
    assert_eq!(entry.scope, SkillReferenceScope::Workspace);
    assert!(entry.enabled);
    assert!(entry.path.ends_with("SKILL.md"));
    assert_eq!(entry.description, "C4 probe skill probe-workspace-skill.");
    // 协议投影自校验：目录里不得有 disabled 条目。
    assert!(result.validate().is_ok());
}

#[test]
fn surface_authority_is_reflected_in_result() {
    let root = TempRoot::new("authority");
    let workspace = root.path().join("ws");
    fs::create_dir_all(&workspace).expect("workspace");
    seed_skill(&workspace, ".mode/skills", "probe-workspace-skill", "Body.");

    let catalog = SkillCatalog::new(test_service(root.path()));
    let result = catalog
        .catalog(
            &workspace.display().to_string(),
            SkillReferenceAuthority::Session,
        )
        .expect("catalog");
    assert_eq!(result.authority, SkillReferenceAuthority::Session);
    assert_eq!(result.skills.len(), 1);
}

#[test]
fn skill_tool_returns_skill_content_and_metadata() {
    let root = TempRoot::new("tool");
    let workspace = root.path().join("ws");
    fs::create_dir_all(&workspace).expect("workspace");
    seed_skill(
        &workspace,
        ".mode/skills",
        "probe-workspace-skill",
        "Workspace skill body for the C4 probe.",
    );

    let source = SkillToolSource::with_options(
        &workspace,
        SkillsServiceOptions {
            home_dir: root.path().to_path_buf(),
            is_desktop_runtime: false,
        },
    );
    let declarations = source.list_tools().expect("list tools");
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0].name, "Skill");
    assert_eq!(declarations[0].read_only, Some(true));
    assert_eq!(
        declarations[0].side_effect_scope.as_deref(),
        Some("session")
    );

    let result = source
        .call_tool("Skill", &json!({ "skill": "probe-workspace-skill" }))
        .expect("skill call");
    let content = result["content"].as_str().expect("content string");
    assert!(content.starts_with(
        "<skill_content name=\"probe-workspace-skill\">\n# Skill: probe-workspace-skill\nWorkspace skill body for the C4 probe.\nBase directory for this skill: "
    ));
    assert!(
        content.contains("probe-workspace-skill"),
        "base directory 必须指向技能目录：{content}"
    );
    assert!(content.ends_with(
        "\nRelative paths in this skill are relative to this base directory.\n</skill_content>"
    ));
    assert_eq!(result["skillMetadata"], json!({ "source": "mode" }));

    let error = source
        .call_tool("Skill", &json!({ "skill": "missing-skill" }))
        .expect_err("missing skill");
    assert_eq!(error.error_type, "tool_execution_failed");
    assert_eq!(error.message, "Skill not found: missing-skill");
}

struct FakeSource;

impl PluginToolSource for FakeSource {
    fn list_tools(&self) -> Result<Vec<PluginToolDeclaration>, String> {
        Ok(vec![PluginToolDeclaration {
            name: "FakeTool".to_string(),
            description: "fake".to_string(),
            input_schema: json!({ "type": "object" }),
            risk_level: None,
            side_effect_scope: None,
            budget_strategy: None,
            read_only: None,
            display_json: None,
        }])
    }

    fn call_tool(&self, tool_name: &str, _input: &Value) -> Result<Value, PluginToolError> {
        if tool_name != "FakeTool" {
            return Err(PluginToolError::tool_execution_failed("unexpected tool"));
        }
        Ok(json!({ "content": "fake-result" }))
    }
}

#[test]
fn composite_source_routes_skill_and_other_sources() {
    let root = TempRoot::new("composite");
    let workspace = root.path().join("ws");
    fs::create_dir_all(&workspace).expect("workspace");
    seed_skill(&workspace, ".mode/skills", "probe-workspace-skill", "Body.");

    let skill = Arc::new(SkillToolSource::with_options(
        &workspace,
        SkillsServiceOptions {
            home_dir: root.path().to_path_buf(),
            is_desktop_runtime: false,
        },
    )) as Arc<dyn PluginToolSource>;
    let fake = Arc::new(FakeSource) as Arc<dyn PluginToolSource>;
    let (declarations, composite) =
        CompositeToolSource::combine(vec![skill, fake]).expect("combine");
    let names: Vec<_> = declarations
        .iter()
        .map(|declaration| declaration.name.as_str())
        .collect();
    assert_eq!(names, vec!["Skill", "FakeTool"]);

    let skill_result = composite
        .call_tool("Skill", &json!({ "skill": "probe-workspace-skill" }))
        .expect("skill route");
    assert_eq!(skill_result["skillMetadata"]["source"], json!("mode"));
    let fake_result = composite
        .call_tool("FakeTool", &json!({}))
        .expect("fake route");
    assert_eq!(fake_result["content"], json!("fake-result"));
    let unknown = composite
        .call_tool("Missing", &json!({}))
        .expect_err("unknown route");
    assert_eq!(unknown.error_type, "tool_execution_failed");
}
