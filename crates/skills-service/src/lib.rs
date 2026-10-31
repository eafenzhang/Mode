//! 技能服务（Rust 实现）。
//!
//! 真源：`packages/services/src/skills/skillsService.ts`（clean-room：只读本仓库 TS）。
//! 行为契约与已知差异见 `docs/specs/skills-service.md`。
//!
//! 覆盖六个方法：`list` / `set_enabled` / `build_prompt_context` / `copy_to_common` /
//! `remove_from_common` / `delete_skill`。`skill-sync` 不在本 crate 内。

mod config;
mod discovery;
mod frontmatter;
mod paths;
mod plugins;

/// Node `fs`/`path`/排序语义：与插件发现共用同一实现，避免两处漂移。
pub(crate) use plugin_discovery::js as fsx;

pub use frontmatter::{read_frontmatter, Frontmatter};

/// 有界技能遍历的公开入口（`settings-sync` / `skill-sync` 等域复用同一扫描策略）。
pub fn walk_skill_markdown_paths(root_path: &Path) -> Vec<PathBuf> {
    let mut diagnostics = Vec::new();
    discovery::collect_skill_markdown_paths(root_path, &mut diagnostics)
}

pub use discovery::{should_walk_skill_directory_entry, MAX_SKILL_SCAN_DEPTH};

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Map, Value};

use crate::paths::{
    resolve_ancestor_workspace_roots, user_agents_skill_root, user_mode_skill_root,
    workspace_mode_skill_root,
};

const PROVIDER: &str = "glm";

#[derive(Debug, Clone)]
pub struct SkillsServiceOptions {
    /// 用户主目录（`resolveUserHomeDir()` 的结果）。
    pub home_dir: PathBuf,
    /// `isDesktopRuntime`：决定是否扫描用户级技能。
    pub is_desktop_runtime: bool,
}

impl SkillsServiceOptions {
    /// 壳内默认：`HOME.trim() || USERPROFILE.trim() || homedir()`，桌面运行时由 `MODE_PROCESS_LABEL` 判定。
    pub fn from_process() -> Self {
        let home = std::env::var("HOME")
            .ok()
            .map(|value| fsx::js_trim(&value).to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                std::env::var("USERPROFILE")
                    .ok()
                    .map(|value| fsx::js_trim(&value).to_string())
                    .filter(|value| !value.is_empty())
            })
            .map(PathBuf::from)
            .unwrap_or_else(fsx::node_homedir);
        let is_desktop_runtime = std::env::var("MODE_PROCESS_LABEL")
            .map(|value| !value.is_empty())
            .unwrap_or(false);
        Self {
            home_dir: home,
            is_desktop_runtime,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillsServiceError {
    SkillNotFound(String),
    CommonTargetExists(String),
    NotInCommon,
    PluginSkillNotDeletable,
    NotDeletable(String),
    Io(String),
}

impl fmt::Display for SkillsServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkillsServiceError::SkillNotFound(skill_id) => {
                write!(formatter, "Skill not found: {skill_id}")
            }
            SkillsServiceError::CommonTargetExists(name) => {
                write!(formatter, "通用目录已存在同名技能: {name}")
            }
            SkillsServiceError::NotInCommon => write!(formatter, "该技能不在通用目录中"),
            SkillsServiceError::PluginSkillNotDeletable => {
                write!(formatter, "插件提供的技能不可单独删除，请卸载对应插件")
            }
            SkillsServiceError::NotDeletable(path) => write!(formatter, "该技能不可删除: {path}"),
            SkillsServiceError::Io(message) => write!(formatter, "{message}"),
        }
    }
}

impl std::error::Error for SkillsServiceError {}

impl From<std::io::Error> for SkillsServiceError {
    fn from(error: std::io::Error) -> Self {
        SkillsServiceError::Io(error.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct ListParams {
    pub workspace_path: PathBuf,
    pub workspace_identity: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SetEnabledParams {
    pub workspace_path: PathBuf,
    pub workspace_identity: Option<String>,
    pub skill_id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct PromptContextParams {
    pub workspace_path: PathBuf,
    pub workspace_identity: Option<String>,
    pub prompt: String,
}

#[derive(Debug, Clone)]
pub struct SkillIdParams {
    pub workspace_path: PathBuf,
    pub workspace_identity: Option<String>,
    pub skill_id: String,
}

pub struct SkillsService {
    options: SkillsServiceOptions,
    /// `setEnabled` 在 TS 里用 promise 链串行化，这里用互斥锁复刻同一语义。
    write_lock: Mutex<()>,
}

impl SkillsService {
    pub fn new(options: SkillsServiceOptions) -> Self {
        Self {
            options,
            write_lock: Mutex::new(()),
        }
    }

    pub fn home_dir(&self) -> &Path {
        &self.options.home_dir
    }

    fn user_scope_available(&self) -> bool {
        self.options.is_desktop_runtime
    }

    fn capability(&self) -> Value {
        if self.user_scope_available() {
            json!({ "userScopeAvailable": true })
        } else {
            json!({ "userScopeAvailable": false, "userScopeReason": "desktop_only" })
        }
    }

    fn discover(&self, workspace_path: &Path) -> discovery::DiscoverResult {
        discovery::discover_skills(
            &self.options.home_dir,
            workspace_path,
            self.user_scope_available(),
            PROVIDER,
        )
    }

    pub fn list(&self, params: &ListParams) -> Value {
        let discovered = self.discover(&params.workspace_path);
        let enabled_map = config::read_skill_enabled_map(&self.options.home_dir);
        let skills: Vec<Value> = discovered
            .skills
            .into_iter()
            .map(|skill| {
                let key = config::normalize_skill_config_path(
                    skill.get("path").and_then(Value::as_str).unwrap_or(""),
                );
                let enabled = enabled_map
                    .iter()
                    .rev()
                    .find(|(path, _)| path == &key)
                    .map(|(_, enable)| *enable)
                    .unwrap_or(true);
                let mut skill = skill;
                if let Value::Object(map) = &mut skill {
                    map.insert("enabled".to_string(), Value::Bool(enabled));
                }
                skill
            })
            .collect();

        json!({
            "skills": skills,
            "capability": self.capability(),
            "diagnostics": discovered.diagnostics,
        })
    }

    pub fn set_enabled(&self, params: &SetEnabledParams) -> Result<(), SkillsServiceError> {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let discovered = self.discover(&params.workspace_path);
        let Some(skill) = find_skill(&discovered.skills, &params.skill_id) else {
            return Err(SkillsServiceError::SkillNotFound(params.skill_id.clone()));
        };
        let path = skill.get("path").and_then(Value::as_str).unwrap_or("");
        let normalized = config::normalize_skill_config_path(path);

        let mut enabled_map = config::read_skill_enabled_map(&self.options.home_dir);
        match enabled_map
            .iter_mut()
            .find(|(existing, _)| existing == &normalized)
        {
            Some(entry) => entry.1 = params.enabled,
            None => enabled_map.push((normalized, params.enabled)),
        }
        config::write_skill_enabled_map(&self.options.home_dir, &enabled_map)?;
        Ok(())
    }

    pub fn build_prompt_context(
        &self,
        params: &PromptContextParams,
    ) -> Result<Value, SkillsServiceError> {
        let mentioned = collect_mentioned_skill_names(&params.prompt);
        if mentioned.is_empty() {
            return Ok(json!({
                "prompt": params.prompt,
                "activatedSkillNames": [],
            }));
        }

        let listed = self.list(&ListParams {
            workspace_path: params.workspace_path.clone(),
            workspace_identity: params.workspace_identity.clone(),
        });
        let activated: Vec<Value> = listed
            .get("skills")
            .and_then(Value::as_array)
            .map(|skills| {
                skills
                    .iter()
                    .filter(|skill| skill.get("enabled").and_then(Value::as_bool) == Some(true))
                    .filter(|skill| {
                        skill
                            .get("name")
                            .and_then(Value::as_str)
                            .map(|name| mentioned.iter().any(|mentioned| mentioned == name))
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if activated.is_empty() {
            return Ok(json!({
                "prompt": params.prompt,
                "activatedSkillNames": [],
            }));
        }

        let activated_skill_names: Vec<String> = activated
            .iter()
            .filter_map(|skill| skill.get("name").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        append_skills_audit_log(
            &self.options.home_dir,
            &params.workspace_path,
            params.workspace_identity.as_deref(),
            &activated_skill_names,
        )?;
        let block = build_activated_skills_prompt_block(&activated);
        Ok(json!({
            "prompt": format!("{}\n\n{}", params.prompt, block),
            "activatedSkillNames": activated_skill_names,
        }))
    }

    pub fn copy_to_common(&self, params: &SkillIdParams) -> Result<Value, SkillsServiceError> {
        let discovered = self.discover(&params.workspace_path);
        let Some(skill) = find_skill(&discovered.skills, &params.skill_id) else {
            return Err(SkillsServiceError::SkillNotFound(params.skill_id.clone()));
        };
        let skill_path = PathBuf::from(skill.get("path").and_then(Value::as_str).unwrap_or(""));
        let source_dir = skill_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let scope = skill.get("scope").and_then(Value::as_str).unwrap_or("");
        let common_root = if scope == "workspace" {
            workspace_mode_skill_root(&params.workspace_path)
        } else {
            user_mode_skill_root(&self.options.home_dir)
        };
        let directory_name = source_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let target_dir = common_root.join(&directory_name);
        if fsx::exists(&target_dir) {
            return Err(SkillsServiceError::CommonTargetExists(directory_name));
        }
        std::fs::create_dir_all(&common_root)?;
        copy_dir_recursive(&source_dir, &target_dir)?;
        Ok(json!({
            "newPath": target_dir.join(discovery::SKILL_FILE_NAME).to_string_lossy(),
        }))
    }

    pub fn remove_from_common(&self, params: &SkillIdParams) -> Result<(), SkillsServiceError> {
        let discovered = self.discover(&params.workspace_path);
        let Some(skill) = find_skill(&discovered.skills, &params.skill_id) else {
            return Err(SkillsServiceError::SkillNotFound(params.skill_id.clone()));
        };
        let skill_path = skill.get("path").and_then(Value::as_str).unwrap_or("");
        let normalized_path = skill_path.replace('\\', "/").to_lowercase();
        let user_common_root = user_mode_skill_root(&self.options.home_dir)
            .to_string_lossy()
            .replace('\\', "/")
            .to_lowercase();
        let workspace_common_root = workspace_mode_skill_root(&params.workspace_path)
            .to_string_lossy()
            .replace('\\', "/")
            .to_lowercase();
        let in_user_common = normalized_path.contains(&format!("{user_common_root}/"));
        let in_workspace_common = normalized_path.contains(&format!("{workspace_common_root}/"));
        if !in_user_common && !in_workspace_common {
            return Err(SkillsServiceError::NotInCommon);
        }
        let directory = PathBuf::from(skill_path)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        remove_dir_force(&directory);
        Ok(())
    }

    pub fn delete_skill(&self, params: &SkillIdParams) -> Result<(), SkillsServiceError> {
        let discovered = self.discover(&params.workspace_path);
        let Some(skill) = find_skill(&discovered.skills, &params.skill_id) else {
            return Err(SkillsServiceError::SkillNotFound(params.skill_id.clone()));
        };
        if skill.get("scope").and_then(Value::as_str) == Some("plugin") {
            return Err(SkillsServiceError::PluginSkillNotDeletable);
        }
        let skill_path = skill.get("path").and_then(Value::as_str).unwrap_or("");
        // 用发现阶段的原始路径定位目录项：软链技能只删链接本体，不动目标。
        let source_path = skill
            .get("sourcePath")
            .and_then(Value::as_str)
            .unwrap_or(skill_path);
        let skill_dir = PathBuf::from(source_path)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let leaf_name = skill_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        // 只解析父目录：叶子若是软链必须保持未解析，删除时才能只删链接。
        let canonical_parent = match skill_dir.parent().and_then(fsx::realpath) {
            Some(path) => path,
            None => return Err(SkillsServiceError::NotDeletable(skill_path.to_string())),
        };

        let mut allowed_roots = resolve_ancestor_workspace_roots(&params.workspace_path);
        allowed_roots.push(user_mode_skill_root(&self.options.home_dir));
        allowed_roots.push(user_agents_skill_root(&self.options.home_dir));
        let contained = allowed_roots.iter().any(|root| {
            let canonical_root = fsx::realpath(root).unwrap_or_else(|| root.clone());
            fsx::contains_path(&canonical_root, &canonical_parent)
        });
        if !contained {
            return Err(SkillsServiceError::NotDeletable(skill_path.to_string()));
        }
        remove_dir_force(&canonical_parent.join(leaf_name));
        Ok(())
    }
}

fn find_skill<'a>(skills: &'a [Value], skill_id: &str) -> Option<&'a Value> {
    skills
        .iter()
        .find(|skill| skill.get("id").and_then(Value::as_str) == Some(skill_id))
}

fn collect_mentioned_skill_names(prompt: &str) -> Vec<String> {
    let bytes = prompt.as_bytes();
    let mut names: Vec<String> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            index += 1;
            continue;
        }
        let start = index + 1;
        let mut cursor = start;
        while cursor < bytes.len() && is_skill_name_byte(bytes[cursor]) {
            cursor += 1;
        }
        if cursor == start {
            index += 1;
            continue;
        }
        loop {
            if cursor < bytes.len() && bytes[cursor] == b'-' {
                let tail_start = cursor + 1;
                let mut tail = tail_start;
                while tail < bytes.len() && is_skill_name_byte(bytes[tail]) {
                    tail += 1;
                }
                if tail > tail_start {
                    cursor = tail;
                    continue;
                }
            }
            break;
        }
        let name = prompt[start..cursor].to_string();
        if !names.iter().any(|existing| existing == &name) {
            names.push(name);
        }
        index = cursor;
    }
    names
}

fn is_skill_name_byte(value: u8) -> bool {
    value.is_ascii_lowercase() || value.is_ascii_digit()
}

fn escape_xml_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn build_activated_skills_prompt_block(skills: &[Value]) -> String {
    let mut parts: Vec<String> = vec!["<available_skills>".to_string()];
    for skill in skills {
        let name = escape_xml_attribute(skill.get("name").and_then(Value::as_str).unwrap_or(""));
        let path = escape_xml_attribute(skill.get("path").and_then(Value::as_str).unwrap_or(""));
        let body = skill.get("body").and_then(Value::as_str).unwrap_or("");
        parts.push(format!(
            "<activated_skill name=\"{name}\" path=\"{path}\">\n{body}\n</activated_skill>"
        ));
    }
    parts.push("</available_skills>".to_string());
    parts.join("\n")
}

fn append_skills_audit_log(
    home_dir: &Path,
    workspace_path: &Path,
    workspace_identity: Option<&str>,
    activated_skill_names: &[String],
) -> Result<(), SkillsServiceError> {
    let log_path = config::audit_log_path(home_dir);
    let Some(parent) = log_path.parent() else {
        return Err(SkillsServiceError::Io(format!(
            "cannot resolve audit log parent: {}",
            log_path.display()
        )));
    };
    std::fs::create_dir_all(parent)?;
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let mut record = Map::new();
    record.insert("createdAt".to_string(), json!(created_at));
    record.insert(
        "workspacePath".to_string(),
        Value::String(workspace_path.to_string_lossy().into_owned()),
    );
    record.insert(
        "workspaceIdentity".to_string(),
        match workspace_identity {
            Some(identity) => Value::String(identity.to_string()),
            None => Value::Null,
        },
    );
    record.insert(
        "activatedSkillNames".to_string(),
        Value::Array(
            activated_skill_names
                .iter()
                .map(|name| Value::String(name.clone()))
                .collect(),
        ),
    );
    let line = format!("{}\n", Value::Object(record));
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

fn copy_dir_recursive(source: &Path, target: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return copy_symlink(source, target);
    }
    if file_type.is_dir() {
        std::fs::create_dir_all(target)?;
        let mut entries: Vec<(String, PathBuf)> = std::fs::read_dir(source)?
            .flatten()
            .map(|entry| {
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    entry.path(),
                )
            })
            .collect();
        entries.sort_by(|left, right| fsx::locale_compare(&left.0, &right.0));
        for (name, path) in entries {
            copy_dir_recursive(&path, &target.join(name))?;
        }
        return Ok(());
    }
    std::fs::copy(source, target).map(|_| ())
}

#[cfg(unix)]
fn copy_symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(std::fs::read_link(source)?, target)
}

#[cfg(windows)]
fn copy_symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    let resolved = std::fs::metadata(source)?;
    if resolved.is_dir() {
        std::os::windows::fs::symlink_dir(std::fs::read_link(source)?, target)
    } else {
        std::os::windows::fs::symlink_file(std::fs::read_link(source)?, target)
    }
}

fn remove_dir_force(path: &Path) {
    let _ = std::fs::remove_dir_all(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentioned_names_follow_regex() {
        // `$x_` 只匹配前面的 `$x`：`_` 不在 `[a-z0-9-]` 里。
        assert_eq!(
            collect_mentioned_skill_names("use $alpha and $beta-2, not $Gamma or $x_"),
            vec!["alpha".to_string(), "beta-2".to_string(), "x".to_string()]
        );
        assert!(collect_mentioned_skill_names("no mentions").is_empty());
    }

    #[test]
    fn prompt_block_escapes_attributes() {
        let skill = json!({
            "name": "a&b",
            "path": "C:\\ws\\<skill>\\SKILL.md",
            "body": "text",
        });
        assert_eq!(
            build_activated_skills_prompt_block(&[skill]),
            "<available_skills>\n<activated_skill name=\"a&amp;b\" path=\"C:\\ws\\&lt;skill&gt;\\SKILL.md\">\ntext\n</activated_skill>\n</available_skills>"
        );
    }

    #[test]
    fn description_limit_is_documented() {
        // 与 `discovery.rs` 的 MAX_DESCRIPTION_LENGTH 对齐，避免两处漂移。
        assert_eq!(discovery::MAX_DESCRIPTION_LENGTH, 1024);
    }
}
