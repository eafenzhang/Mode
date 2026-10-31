//! CLI config 读写：技能开关映射与路径归一。
//!
//! 真源：`skillsService.ts` 的 `readCliConfigFile` / `readSkillEnabledMapFromConfig` /
//! `writeSkillEnabledMap` / `normalizeSkillConfigPath`。

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::fsx;
use crate::paths::MODE_DATA_ROOT_DIR_NAME;

pub(crate) fn cli_settings_dir(home_dir: &Path) -> PathBuf {
    home_dir.join(MODE_DATA_ROOT_DIR_NAME).join("cli")
}

pub(crate) fn cli_config_file(home_dir: &Path) -> PathBuf {
    cli_settings_dir(home_dir).join("config.json")
}

pub(crate) fn audit_log_path(home_dir: &Path) -> PathBuf {
    home_dir
        .join(MODE_DATA_ROOT_DIR_NAME)
        .join("v2")
        .join("skills-audit.log")
}

/// 读 `<home>/.mode/cli/config.json`；缺失、非 JSON、非对象都按 `{}`。
pub(crate) fn read_cli_config(home_dir: &Path) -> Map<String, Value> {
    let Some(raw) = fsx::read_text(&cli_config_file(home_dir)) else {
        return Map::new();
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Object(object)) => object,
        _ => Map::new(),
    }
}

pub(crate) fn normalize_skill_config_path(path: &str) -> String {
    path.replace('\\', "/")
}

/// 只收 `skills[path] = { enable: boolean }`，键归一为 `/` 分隔。
pub(crate) fn read_skill_enabled_map(home_dir: &Path) -> Vec<(String, bool)> {
    let config = read_cli_config(home_dir);
    let skills = config
        .get("skills")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut result = Vec::new();
    for (path, value) in skills {
        let Some(enable) = value.get("enable").and_then(Value::as_bool) else {
            continue;
        };
        result.push((normalize_skill_config_path(&path), enable));
    }
    result
}

/// 开关落盘：`true` 删除 override，`false` 写 `{ enable: false }`；空对象则删除 `skills` 字段。
pub(crate) fn write_skill_enabled_map(
    home_dir: &Path,
    next: &[(String, bool)],
) -> std::io::Result<()> {
    let mut config = read_cli_config(home_dir);
    let mut skills = config
        .get("skills")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut ordered: Vec<&(String, bool)> = next.iter().collect();
    ordered.sort_by(|left, right| fsx::locale_compare(&left.0, &right.0));

    for (path, enable) in ordered {
        let normalized = normalize_skill_config_path(path);
        if *enable {
            // 开启是默认态，落盘成 `{ enable: true }` 会让插件/默认配置的后续变化失效。
            skills.remove(&normalized);
        } else {
            let mut entry = Map::new();
            entry.insert("enable".to_string(), Value::Bool(false));
            skills.insert(normalized, Value::Object(entry));
        }
    }

    if skills.is_empty() {
        config.remove("skills");
    } else {
        config.insert("skills".to_string(), Value::Object(skills));
    }

    std::fs::create_dir_all(cli_settings_dir(home_dir))?;
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&Value::Object(config))
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?
    );
    std::fs::write(cli_config_file(home_dir), text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "zc-skills-config-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn enabled_true_removes_override_and_preserves_other_keys() {
        let home = temp_home("write");
        std::fs::create_dir_all(cli_settings_dir(&home)).unwrap();
        std::fs::write(
            cli_config_file(&home),
            r#"{"theme":"dark","skills":{"C:\\ws\\a\\SKILL.md":{"enable":true},"C:\\ws\\b\\SKILL.md":{"enable":false}}}"#,
        )
        .unwrap();

        write_skill_enabled_map(
            &home,
            &[
                ("C:\\ws\\a\\SKILL.md".to_string(), false),
                ("C:\\ws\\b\\SKILL.md".to_string(), true),
            ],
        )
        .unwrap();

        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(cli_config_file(&home)).unwrap())
                .unwrap();
        assert_eq!(written["theme"], "dark");
        assert_eq!(written["skills"]["C:/ws/a/SKILL.md"]["enable"], false);
        assert!(written["skills"].get("C:/ws/b/SKILL.md").is_none());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn empty_map_drops_skills_key() {
        let home = temp_home("drop");
        std::fs::create_dir_all(cli_settings_dir(&home)).unwrap();
        std::fs::write(
            cli_config_file(&home),
            r#"{"skills":{"a":{"enable":false}}}"#,
        )
        .unwrap();
        write_skill_enabled_map(&home, &[("a".to_string(), true)]).unwrap();
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(cli_config_file(&home)).unwrap())
                .unwrap();
        assert!(written.get("skills").is_none());
        let _ = std::fs::remove_dir_all(&home);
    }
}
