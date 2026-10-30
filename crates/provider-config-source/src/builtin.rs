//! Mode Built-in Release 的只读解析、规范序列化与候选选择。
//!
//! 真源：`packages/provider-node/src/mode-builtin-release.ts`、
//! `packages/provider-node/src/mode-builtin-provider-config-source.ts`。

use provider_config::{
    parse_mode_builtin_model_config_rules, parse_mode_builtin_provider_config_rules, JsonValue,
    ModelConfigRules, ProviderConfigMap, ProviderTemplateMap,
};
use serde_json::Number;

use crate::personal::reject_retired_provider;
use crate::resolve::source_key_for_path;
use crate::SourceError;

const SCHEMA_VERSION: i64 = 1;
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeBuiltinRelease {
    pub revision: i64,
    pub providers: ProviderConfigMap,
    pub provider_templates: ProviderTemplateMap,
    pub models: ModelConfigRules,
}

impl ModeBuiltinRelease {
    /// `encodeModeBuiltinRelease`：固定键顺序。
    pub fn to_json(&self) -> JsonValue {
        JsonValue::object_from([
            (
                "schemaVersion".to_owned(),
                JsonValue::Number(Number::from(SCHEMA_VERSION)),
            ),
            (
                "revision".to_owned(),
                JsonValue::Number(Number::from(self.revision)),
            ),
            (
                "config".to_owned(),
                JsonValue::object_from([
                    (
                        "providerConfigRules".to_owned(),
                        JsonValue::object_from([
                            (
                                "templateRules".to_owned(),
                                self.provider_templates.to_json(),
                            ),
                            ("providerRules".to_owned(), self.providers.to_json()),
                        ]),
                    ),
                    (
                        "modelConfigRules".to_owned(),
                        self.models.to_mode_builtin_json(),
                    ),
                ]),
            ),
        ])
    }

    /// `serializeModeBuiltinRelease`。
    pub fn serialize(&self) -> String {
        self.to_json().to_json_string()
    }
}

/// 严格解码 Release；未知字段、版本/序号越界、规则非法或包含已退出 Provider 都失败。
pub fn decode_mode_builtin_release(value: &JsonValue) -> Result<ModeBuiltinRelease, SourceError> {
    let obj = value
        .as_object()
        .ok_or_else(|| SourceError::invalid("Mode Built-in Release 必须是对象"))?;
    reject_unknown_keys(
        obj.keys(),
        &["schemaVersion", "revision", "config"],
        "Release",
    )?;

    let schema_version = obj
        .get("schemaVersion")
        .ok_or_else(|| SourceError::invalid("Mode Built-in Release 缺少 schemaVersion"))?;
    if js_number_equals(schema_version, SCHEMA_VERSION) != Some(true) {
        return Err(SourceError::invalid(format!(
            "Mode Built-in Release schemaVersion 必须为 {SCHEMA_VERSION}"
        )));
    }

    let revision_value = obj
        .get("revision")
        .ok_or_else(|| SourceError::invalid("Mode Built-in Release 缺少 revision"))?;
    let revision = js_safe_integer(revision_value)
        .filter(|value| *value >= 0)
        .ok_or_else(|| SourceError::invalid("Mode Built-in Release revision 必须是非负安全整数"))?;

    let config_value = obj
        .get("config")
        .ok_or_else(|| SourceError::invalid("Mode Built-in Release 缺少 config"))?;
    let config = config_value
        .as_object()
        .ok_or_else(|| SourceError::invalid("Mode Built-in Release.config 必须是对象"))?;
    reject_unknown_keys(
        config.keys(),
        &["providerConfigRules", "modelConfigRules"],
        "Release.config",
    )?;
    let provider_rules = config
        .get("providerConfigRules")
        .ok_or_else(|| SourceError::invalid("Release.config.providerConfigRules 缺失"))?;
    let model_rules = config
        .get("modelConfigRules")
        .ok_or_else(|| SourceError::invalid("Release.config.modelConfigRules 缺失"))?;

    let (providers, provider_templates) =
        parse_mode_builtin_provider_config_rules(provider_rules, &[]).map_err(|error| {
            SourceError::invalid(format!("providerConfigRules 不合法: {error:?}"))
        })?;
    reject_retired_provider(&providers)?;
    let models = parse_mode_builtin_model_config_rules(model_rules, &[])
        .map_err(|error| SourceError::invalid(format!("modelConfigRules 不合法: {error:?}")))?;

    Ok(ModeBuiltinRelease {
        revision,
        providers,
        provider_templates,
        models,
    })
}

pub fn serialize_mode_builtin_release(release: &ModeBuiltinRelease) -> String {
    release.serialize()
}

/// Builtin 层快照：对应 `snapshotFromRelease`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltinLayer {
    pub revision: String,
    pub providers: ProviderConfigMap,
    pub provider_templates: ProviderTemplateMap,
    pub models: ModelConfigRules,
}

impl BuiltinLayer {
    pub fn to_json(&self) -> JsonValue {
        JsonValue::object_from([
            (
                "revision".to_owned(),
                JsonValue::String(self.revision.clone()),
            ),
            ("providers".to_owned(), self.providers.to_json()),
            (
                "providerTemplates".to_owned(),
                self.provider_templates.to_json(),
            ),
            ("models".to_owned(), self.models.to_json()),
        ])
    }

    pub fn to_json_string(&self) -> String {
        self.to_json().to_json_string()
    }
}

/// 只读读取 Bundled/Active 候选并按 TS 规则选择。
///
/// `active_path` 为空时使用 `bundled_path`；`sourceKey` 始终按 Active 路径计算。
pub fn read_mode_builtin_layer(
    bundled_path: &str,
    active_path: Option<&str>,
) -> Result<BuiltinLayer, SourceError> {
    let bundled = bundled_path.trim();
    if bundled.is_empty() {
        return Err(SourceError::invalid(
            "Mode Built-in bundledFilePath 不能为空",
        ));
    }
    let active = active_path
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(bundled);
    let source_key = source_key_for_path(active);

    let bundled_candidate = read_release_candidate(bundled);
    let active_candidate = if active == bundled {
        None
    } else {
        Some(read_release_candidate(active))
    };
    let release = select_release_candidate(&bundled_candidate, active_candidate.as_ref())?;

    Ok(BuiltinLayer {
        revision: format!("mode-builtin:{}:{}", release.revision, source_key),
        providers: release.providers,
        provider_templates: release.provider_templates,
        models: release.models,
    })
}

enum ReleaseCandidate {
    Missing,
    Invalid(String),
    Valid(ModeBuiltinRelease),
}

impl ReleaseCandidate {
    fn valid(&self) -> Option<&ModeBuiltinRelease> {
        match self {
            ReleaseCandidate::Valid(release) => Some(release),
            _ => None,
        }
    }

    fn error_message(&self) -> Option<String> {
        match self {
            ReleaseCandidate::Invalid(message) => Some(message.clone()),
            _ => None,
        }
    }
}

fn read_release_candidate(path: &str) -> ReleaseCandidate {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ReleaseCandidate::Missing
        }
        Err(error) => return ReleaseCandidate::Invalid(error.to_string()),
    };
    // Node `readFile(..., "utf8")` 对非法序列使用 U+FFFD 替换。
    let text = String::from_utf8_lossy(&bytes);
    let value = match JsonValue::from_json_str(&text) {
        Ok(value) => value,
        Err(error) => return ReleaseCandidate::Invalid(error.to_string()),
    };
    match decode_mode_builtin_release(&value) {
        Ok(release) => ReleaseCandidate::Valid(release),
        Err(error) => ReleaseCandidate::Invalid(error.to_string()),
    }
}

fn select_release_candidate(
    bundled: &ReleaseCandidate,
    active: Option<&ReleaseCandidate>,
) -> Result<ModeBuiltinRelease, SourceError> {
    let bundled_valid = bundled.valid();
    let active_valid = active.and_then(ReleaseCandidate::valid);
    if let (Some(bundled), Some(active)) = (bundled_valid, active_valid) {
        // 同 revision 冲突属于 Active 缓存失效，不能反向使可信 Bundled 无法启动。
        if bundled.revision == active.revision && bundled.serialize() != active.serialize() {
            return Ok(bundled.clone());
        }
    }

    let mut newest: Option<&ModeBuiltinRelease> = None;
    for candidate in [bundled_valid, active_valid].into_iter().flatten() {
        match newest {
            None => newest = Some(candidate),
            Some(current) if candidate.revision > current.revision => newest = Some(candidate),
            _ => {}
        }
    }
    newest.cloned().ok_or_else(|| SourceError::Unavailable {
        bundled: bundled.error_message(),
        active: active.and_then(ReleaseCandidate::error_message),
    })
}

fn reject_unknown_keys<'a>(
    keys: impl Iterator<Item = &'a String>,
    allowed: &[&str],
    what: &str,
) -> Result<(), SourceError> {
    if let Some(key) = keys
        .into_iter()
        .find(|key| !allowed.contains(&key.as_str()))
    {
        return Err(SourceError::invalid(format!("{what} 含未知字段: {key}")));
    }
    Ok(())
}

/// `z.literal(number)` 的 `===` 比较。
fn js_number_equals(value: &JsonValue, expected: i64) -> Option<bool> {
    let JsonValue::Number(number) = value else {
        return Some(false);
    };
    let actual = number
        .as_i64()
        .map(|value| value as f64)
        .or_else(|| number.as_f64())?;
    Some(actual == expected as f64)
}

/// `z.number().int().nonnegative().max(MAX_SAFE_INTEGER)` 的整数部分。
fn js_safe_integer(value: &JsonValue) -> Option<i64> {
    let JsonValue::Number(number) = value else {
        return None;
    };
    let integer = number
        .as_i64()
        .or_else(|| {
            number
                .as_u64()
                .map(|value| i64::try_from(value).unwrap_or(i64::MAX))
        })
        .or_else(|| {
            let value = number.as_f64()?;
            if value.is_finite() && value.fract() == 0.0 && value.abs() <= MAX_SAFE_INTEGER as f64 {
                Some(value as i64)
            } else {
                None
            }
        })?;
    (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER)
        .contains(&integer)
        .then_some(integer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_json(revision: i64) -> String {
        format!(
            r#"{{"schemaVersion":1,"revision":{revision},"config":{{"providerConfigRules":{{"templateRules":[],"providerRules":[{{"providerId":"p1","config":{{"group":"zai-family"}}}}]}},"modelConfigRules":{{"modelRules":[],"modelApiRules":[],"providerSiteRules":[],"templateModelRules":[],"builtinProviderModelRules":[]}}}}}}"#
        )
    }

    fn write(dir: &std::path::Path, name: &str, content: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pcs-unit-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn release_round_trips_canonically() {
        let value = JsonValue::from_json_str(&release_json(5)).unwrap();
        let release = decode_mode_builtin_release(&value).unwrap();
        assert_eq!(release.revision, 5);
        assert_eq!(release.serialize(), value.to_json_string());
    }

    #[test]
    fn retired_provider_release_is_rejected() {
        let text = r#"{"schemaVersion":1,"revision":1,"config":{"providerConfigRules":{"templateRules":[],"providerRules":[{"providerId":"builtin:zapi","config":{"group":"zai-family"}}]},"modelConfigRules":{"modelRules":[],"modelApiRules":[],"providerSiteRules":[],"templateModelRules":[],"builtinProviderModelRules":[]}}}"#;
        let value = JsonValue::from_json_str(text).unwrap();
        assert!(decode_mode_builtin_release(&value).is_err());
    }

    #[test]
    fn missing_active_falls_back_to_bundled() {
        let dir = temp_dir("missing-active");
        let bundled = write(&dir, "bundled.json", &release_json(5));
        let active = dir.join("active").join("active.json");
        let active = active.to_string_lossy().into_owned();
        let layer = read_mode_builtin_layer(&bundled, Some(&active)).unwrap();
        assert!(layer.revision.starts_with("mode-builtin:5:"));
        assert_eq!(layer.providers.keys(), vec!["p1".to_owned()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn newer_active_wins_and_equal_revision_conflict_prefers_bundled() {
        let dir = temp_dir("active-select");
        let bundled = write(&dir, "bundled.json", &release_json(5));
        let active_newer = write(&dir, "active-newer.json", &release_json(7));
        assert!(read_mode_builtin_layer(&bundled, Some(&active_newer))
            .unwrap()
            .revision
            .starts_with("mode-builtin:7:"));

        let mut conflict = JsonValue::from_json_str(&release_json(5)).unwrap();
        if let JsonValue::Object(map) = &mut conflict {
            // 改一个与 revision 无关的字段制造同 revision 不同内容。
            let config = map.get_mut("config").unwrap();
            if let JsonValue::Object(config) = config {
                config.insert(
                    "modelConfigRules".to_owned(),
                    JsonValue::from_json_str(
                        r#"{"modelRules":[{"modelMatch":"x-.*","config":{"enabled":true}}],"modelApiRules":[],"providerSiteRules":[],"templateModelRules":[],"builtinProviderModelRules":[]}"#,
                    )
                    .unwrap(),
                );
            }
        }
        let active_conflict = write(&dir, "active-conflict.json", &conflict.to_json_string());
        let layer = read_mode_builtin_layer(&bundled, Some(&active_conflict)).unwrap();
        assert!(layer.revision.starts_with("mode-builtin:5:"));
        // Bundled 的 models 为空；冲突 Active 才有 modelMatch 规则。
        assert!(layer.models.rules().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn both_invalid_is_unavailable() {
        let dir = temp_dir("both-invalid");
        let bundled = write(&dir, "bundled.json", "{}");
        let active = write(&dir, "active.json", "{}");
        assert!(matches!(
            read_mode_builtin_layer(&bundled, Some(&active)),
            Err(SourceError::Unavailable { .. })
        ));
        std::fs::remove_dir_all(&dir).ok();
    }
}
