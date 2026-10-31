//! 技能发现：有界遍历、frontmatter 校验、去重与排序。
//!
//! 真源：`skillsService.ts` 的 `discoverSkills` / `collectSkillMarkdownPaths` /
//! `collectSkillNameKeysInRoot` / `isUserAgentsSkillCoveredByMode` / `readSkillMetadata`，
//! `skillDiscoveryWalk.ts` 的 `walkSkillMarkdownPaths`，
//! `packages/shared/src/skill-scan-policy.ts` 的排除目录与深度上限。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::frontmatter::read_frontmatter;
use crate::fsx;
use crate::paths::{
    dedupe_roots, dedupe_scan_roots_by_realpath, resolve_ancestor_workspace_roots,
    user_agents_skill_root, user_mode_skill_root, SkillRootDescriptor, SkillScope,
};
use crate::plugins::resolve_plugin_skill_root_descriptors;

pub(crate) const SKILL_FILE_NAME: &str = "SKILL.md";
const SKILL_META_FILE_NAME: &str = "_meta.json";
pub const MAX_SKILL_SCAN_DEPTH: usize = 8;
pub(crate) const MAX_DESCRIPTION_LENGTH: usize = 1024;
const SKILL_SCAN_EXCLUDED_DIRECTORY_NAMES: [&str; 12] = [
    "node_modules",
    "dist",
    "build",
    "out",
    "target",
    "vendor",
    "coverage",
    ".cache",
    ".next",
    ".turbo",
    ".venv",
    "__pycache__",
];
const SKILL_DISCOVERY_DOT_DIR_ALLOWLIST: [&str; 1] = [".system"];

pub fn should_walk_skill_directory_entry(name: &str) -> bool {
    if SKILL_SCAN_EXCLUDED_DIRECTORY_NAMES.contains(&name) {
        return false;
    }
    if !name.starts_with('.') {
        return true;
    }
    SKILL_DISCOVERY_DOT_DIR_ALLOWLIST.contains(&name)
}

#[derive(Debug, Clone)]
pub(crate) struct WalkError {
    pub path: PathBuf,
    pub message: String,
}

/// 深度优先遍历，产出每个 `SKILL.md` 的绝对路径（顺序与 TS 生成器一致）。
pub(crate) fn walk_skill_markdown_paths(root_path: &Path) -> (Vec<PathBuf>, Vec<WalkError>) {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut errors: Vec<WalkError> = Vec::new();
    let mut visited_symlink_targets: HashSet<String> = HashSet::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root_path.to_path_buf(), 0)];

    while let Some((dir, depth)) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                errors.push(WalkError {
                    path: dir,
                    message: error.to_string(),
                });
                continue;
            }
        };

        let mut has_skill_file = false;
        let mut child_directories: Vec<PathBuf> = Vec::new();
        let mut child_symlinks: Vec<PathBuf> = Vec::new();

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let file_type = entry.file_type().ok();
            let is_directory = file_type.map(|kind| kind.is_dir()).unwrap_or(false);
            let is_symlink = file_type.map(|kind| kind.is_symlink()).unwrap_or(false);

            if name == SKILL_FILE_NAME && !is_directory {
                has_skill_file = true;
                continue;
            }
            if !should_walk_skill_directory_entry(&name) {
                continue;
            }
            let entry_path = dir.join(&name);
            if is_directory {
                child_directories.push(entry_path);
            } else if is_symlink {
                child_symlinks.push(entry_path);
            }
        }

        if has_skill_file {
            found.push(dir.join(SKILL_FILE_NAME));
        }

        if depth >= MAX_SKILL_SCAN_DEPTH {
            continue;
        }

        for child in child_directories {
            stack.push((child, depth + 1));
        }
        for child in child_symlinks {
            let target = match std::fs::metadata(&child) {
                Ok(metadata) => metadata,
                Err(error) => {
                    errors.push(WalkError {
                        path: child,
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            if !target.is_dir() {
                continue;
            }
            let canonical = fsx::realpath(&child)
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|| child.to_string_lossy().into_owned());
            if !visited_symlink_targets.insert(canonical) {
                continue;
            }
            stack.push((child, depth + 1));
        }
    }

    (found, errors)
}

/// 收集并按 `localeCompare` 升序返回；扫描失败进诊断。
pub(crate) fn collect_skill_markdown_paths(
    root_path: &Path,
    diagnostics: &mut Vec<Value>,
) -> Vec<PathBuf> {
    let (found, errors) = walk_skill_markdown_paths(root_path);
    for error in errors {
        diagnostics.push(json!({
            "code": "skill_scan_failed",
            "severity": "warning",
            "message": error.message,
            "path": error.path.to_string_lossy(),
        }));
    }
    let mut unique: Vec<PathBuf> = Vec::new();
    for path in found {
        if !unique.iter().any(|existing| existing == &path) {
            unique.push(path);
        }
    }
    unique.sort_by(|left, right| {
        fsx::locale_compare(&left.to_string_lossy(), &right.to_string_lossy())
    });
    unique
}

fn normalize_skill_name_key(name: &str) -> String {
    fsx::js_trim(name).to_lowercase()
}

fn read_skill_name_key(skill_path: &Path) -> String {
    let fallback_name = skill_folder_name(skill_path);
    let Some(content) = fsx::read_text(skill_path) else {
        return normalize_skill_name_key(&fallback_name);
    };
    let parsed = read_frontmatter(&content);
    let raw_name = if parsed.has_frontmatter {
        fsx::js_trim(&parsed.name).to_string()
    } else {
        fallback_name.clone()
    };
    let resolved = if raw_name.is_empty() {
        fallback_name
    } else {
        raw_name
    };
    normalize_skill_name_key(&resolved)
}

fn skill_folder_name(skill_path: &Path) -> String {
    skill_path
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn collect_skill_name_keys_in_root(root_path: &Path) -> HashSet<String> {
    let mut keys = HashSet::new();
    if !fsx::exists(root_path) {
        return keys;
    }
    let mut diagnostics = Vec::new();
    for skill_path in collect_skill_markdown_paths(root_path, &mut diagnostics) {
        keys.insert(read_skill_name_key(&skill_path));
    }
    keys
}

fn is_user_agents_skill_covered_by_mode(
    home_dir: &Path,
    skill_path: &Path,
    root_path: &Path,
    user_mode_skill_name_keys: &HashSet<String>,
) -> bool {
    if root_path != user_agents_skill_root(home_dir).as_path() {
        return false;
    }
    let mirrored = user_mode_skill_root(home_dir)
        .join(skill_folder_name(skill_path))
        .join(SKILL_FILE_NAME);
    if fsx::exists(&mirrored) {
        return true;
    }
    user_mode_skill_name_keys.contains(&read_skill_name_key(skill_path))
}

pub(crate) fn read_skill_metadata(skill_path: &Path) -> Option<Value> {
    let meta_path = skill_path.parent()?.join(SKILL_META_FILE_NAME);
    let raw = fsx::read_text(&meta_path)?;
    if raw.is_empty() {
        return None;
    }
    let parsed: Value = serde_json::from_str(&raw).ok()?;
    let object = parsed.as_object()?;

    let mut metadata = Map::new();
    for key in ["slug", "version", "ownerId"] {
        if let Some(value) = object.get(key).and_then(Value::as_str) {
            let trimmed = fsx::js_trim(value);
            if !trimmed.is_empty() {
                metadata.insert(key.to_string(), Value::String(trimmed.to_string()));
            }
        }
    }
    // publishedAt 原样保留 JSON Number：走 as_f64 再序列化会把整数渲染成 1700000000.0，
    // 与 TS JSON.stringify 的整数输出不一致。
    if let Some(value) = object.get("publishedAt") {
        if value.is_number() {
            metadata.insert("publishedAt".to_string(), value.clone());
        }
    }
    if metadata.is_empty() {
        None
    } else {
        Some(Value::Object(metadata))
    }
}

fn hash_stable_id_part(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn build_skill_id(provider: &str, scope: SkillScope, name: &str, path: &str) -> String {
    format!(
        "{provider}:{}:{name}:{}",
        scope.as_str(),
        hash_stable_id_part(path)
    )
}

pub(crate) struct DiscoverResult {
    pub skills: Vec<Value>,
    pub diagnostics: Vec<Value>,
}

pub(crate) fn discover_skills(
    home_dir: &Path,
    workspace_path: &Path,
    include_user_skills: bool,
    provider: &str,
) -> DiscoverResult {
    let workspace_roots = dedupe_roots(&resolve_ancestor_workspace_roots(workspace_path));
    let mut roots: Vec<SkillRootDescriptor> = workspace_roots
        .into_iter()
        .map(|root_path| SkillRootDescriptor {
            scope: SkillScope::Workspace,
            root_path,
            plugin_name: None,
            plugin_id: None,
        })
        .collect();
    if include_user_skills {
        // 用户级技能是全局资源，`.mode/skills` 有技能也不能截断 `.agents/skills`。
        roots.push(SkillRootDescriptor {
            scope: SkillScope::User,
            root_path: user_mode_skill_root(home_dir),
            plugin_name: None,
            plugin_id: None,
        });
        roots.push(SkillRootDescriptor {
            scope: SkillScope::User,
            root_path: user_agents_skill_root(home_dir),
            plugin_name: None,
            plugin_id: None,
        });
    }
    roots.extend(resolve_plugin_skill_root_descriptors(home_dir));

    let mut diagnostics: Vec<Value> = Vec::new();
    let mut skills: Vec<Value> = Vec::new();
    let mut seen_skill_paths: Vec<String> = Vec::new();
    let user_mode_skill_name_keys = if include_user_skills {
        collect_skill_name_keys_in_root(&user_mode_skill_root(home_dir))
    } else {
        HashSet::new()
    };
    let scan_root_paths = dedupe_scan_roots_by_realpath(
        &roots
            .iter()
            .map(|root| root.root_path.clone())
            .collect::<Vec<PathBuf>>(),
    );

    for root_path in scan_root_paths {
        let Some(root) = roots
            .iter()
            .rev()
            .find(|candidate| candidate.root_path == root_path)
        else {
            continue;
        };
        if !fsx::exists(&root.root_path) {
            continue;
        }

        let skill_paths = collect_skill_markdown_paths(&root.root_path, &mut diagnostics);
        for skill_path in skill_paths {
            if is_user_agents_skill_covered_by_mode(
                home_dir,
                &skill_path,
                &root.root_path,
                &user_mode_skill_name_keys,
            ) {
                continue;
            }
            let canonical_skill_path =
                fsx::realpath(&skill_path).unwrap_or_else(|| skill_path.clone());
            let canonical_key = canonical_skill_path.to_string_lossy().into_owned();
            if seen_skill_paths
                .iter()
                .any(|existing| existing == &canonical_key)
            {
                continue;
            }
            seen_skill_paths.push(canonical_key);
            if !fsx::exists(&skill_path) {
                continue;
            }
            let skill_display_path = skill_path.to_string_lossy().into_owned();
            let Some(markdown) = fsx::read_text(&skill_path) else {
                diagnostics.push(json!({
                    "code": "skill_read_failed",
                    "severity": "warning",
                    "message": format!("Failed to read skill: {skill_display_path}"),
                    "path": skill_display_path,
                }));
                continue;
            };
            let parsed = read_frontmatter(&markdown);
            let folder_name = skill_folder_name(&skill_path);

            // name 校验只要求非空；缺 frontmatter 的手写 skill 用目录名兜底。
            let raw_name = if parsed.has_frontmatter {
                fsx::js_trim(&parsed.name).to_string()
            } else {
                folder_name.clone()
            };
            let resolved_name = if raw_name.is_empty() {
                folder_name
            } else {
                raw_name
            };
            if resolved_name.is_empty() {
                diagnostics.push(json!({
                    "code": "skill_missing_name",
                    "severity": "error",
                    "message": format!("Skill frontmatter must include a name: {skill_display_path}"),
                    "path": skill_display_path,
                }));
                continue;
            }

            let description = if parsed.has_frontmatter {
                fsx::js_trim(&parsed.description).to_string()
            } else {
                String::new()
            };
            // JS 的 string.length 是 UTF-16 码元数。
            if description.encode_utf16().count() > MAX_DESCRIPTION_LENGTH {
                diagnostics.push(json!({
                    "code": "skill_description_too_long",
                    "severity": "error",
                    "message": format!(
                        "Skill description is too long (>{MAX_DESCRIPTION_LENGTH}): {resolved_name}"
                    ),
                    "path": skill_display_path,
                    "skillName": resolved_name,
                }));
                continue;
            }

            let body = fsx::js_trim(&parsed.body).to_string();
            let metadata = read_skill_metadata(&skill_path);
            let canonical_display = canonical_skill_path.to_string_lossy().into_owned();

            let mut summary = Map::new();
            summary.insert(
                "id".to_string(),
                Value::String(build_skill_id(
                    provider,
                    root.scope,
                    &resolved_name,
                    &canonical_display,
                )),
            );
            summary.insert("name".to_string(), Value::String(resolved_name));
            summary.insert("description".to_string(), Value::String(description));
            summary.insert("body".to_string(), Value::String(body));
            summary.insert("path".to_string(), Value::String(canonical_display));
            summary.insert("sourcePath".to_string(), Value::String(skill_display_path));
            summary.insert(
                "scope".to_string(),
                Value::String(root.scope.as_str().to_string()),
            );
            summary.insert("enabled".to_string(), Value::Bool(true));
            if let Some(plugin_name) = &root.plugin_name {
                summary.insert("pluginName".to_string(), Value::String(plugin_name.clone()));
            }
            if let Some(plugin_id) = &root.plugin_id {
                summary.insert("pluginId".to_string(), Value::String(plugin_id.clone()));
            }
            if let Some(metadata) = metadata {
                summary.insert("metadata".to_string(), metadata);
            }
            skills.push(Value::Object(summary));
        }
    }

    skills.sort_by(|left, right| {
        let name_order = fsx::locale_compare(
            left.get("name").and_then(Value::as_str).unwrap_or(""),
            right.get("name").and_then(Value::as_str).unwrap_or(""),
        );
        if name_order != std::cmp::Ordering::Equal {
            return name_order;
        }
        let scope_order = fsx::locale_compare(
            left.get("scope").and_then(Value::as_str).unwrap_or(""),
            right.get("scope").and_then(Value::as_str).unwrap_or(""),
        );
        if scope_order != std::cmp::Ordering::Equal {
            return scope_order;
        }
        fsx::locale_compare(
            left.get("path").and_then(Value::as_str).unwrap_or(""),
            right.get("path").and_then(Value::as_str).unwrap_or(""),
        )
    });

    DiscoverResult {
        skills,
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excluded_directories_and_dot_allowlist() {
        assert!(!should_walk_skill_directory_entry("node_modules"));
        assert!(!should_walk_skill_directory_entry(".cache"));
        assert!(should_walk_skill_directory_entry(".system"));
        assert!(should_walk_skill_directory_entry("group"));
    }

    #[test]
    fn skill_id_shape_and_hash() {
        assert_eq!(
            build_skill_id(
                "glm",
                SkillScope::User,
                "alpha",
                "/ws/.mode/skills/alpha/SKILL.md"
            ),
            format!(
                "glm:user:alpha:{}",
                hash_stable_id_part("/ws/.mode/skills/alpha/SKILL.md")
            )
        );
        assert_eq!(hash_stable_id_part("abc").len(), 12);
    }
}
