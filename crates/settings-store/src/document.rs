//! `AppSettings` 的默认值、合并与打补丁（W1-1b 的模型层）。
//!
//! 默认值与「合并 + 补丁」的期望结果由 TS 的 zod schema 生成
//! （`scripts/generate-settings-fixtures.ts` → `fixtures/settings.json`）。
//! Rust 侧只负责与这些结果一致。
//!
//! ## 与 TS 的两处差异（都已写进 spec）
//!
//! 1. **字段类型不匹配时只回落该字段**：TS 里一个字段类型不对会让整个 `safeParse` 失败、
//!    整个文件回落默认值。这里逐字段回落（更保守，不会因为一个字段丢掉整份配置）。
//! 2. **补丁不做类型校验**：TS 的 `appSettingsPatchSchema` 会让非法补丁整条 `update` 抛错；
//!    这里非法值在合并时按类型回落默认值。调用方需要严格语义时应自行校验。

use std::collections::HashSet;
use std::sync::LazyLock;

use serde_json::{Map, Value};

/// 与 `settingService` 的 `MAX_RECENT_PROJECTS` 一致。
const MAX_RECENT_PROJECTS: usize = 10;

const FIXTURE_JSON: &str = include_str!("../fixtures/settings.json");

struct FixtureData {
    allowed_keys: HashSet<String>,
    defaults: Value,
}

static FIXTURE: LazyLock<FixtureData> = LazyLock::new(|| {
    #[derive(serde::Deserialize)]
    struct RawFixture {
        #[serde(rename = "allowedKeys")]
        allowed_keys: Vec<String>,
        defaults: Value,
    }
    let raw: RawFixture =
        serde_json::from_str(FIXTURE_JSON).expect("fixtures/settings.json 必须是合法 JSON");
    FixtureData {
        allowed_keys: raw.allowed_keys.into_iter().collect(),
        defaults: raw.defaults,
    }
});

/// schema 给出的完整默认值（`appSettingsSchema.parse({})`）。
pub fn defaults() -> &'static Value {
    &FIXTURE.defaults
}

/// schema 允许的顶层键（含没有默认值的可选字段）。
pub fn allowed_keys() -> &'static HashSet<String> {
    &FIXTURE.allowed_keys
}

/// 把持久化值合并到默认值上：只保留 schema 允许的顶层键，缺省字段取默认值。
pub fn merge_persisted(persisted: &Value) -> Value {
    let Some(persisted_object) = persisted.as_object() else {
        return defaults().clone();
    };
    let default_object = defaults().as_object().expect("默认值必须是对象");
    let mut merged = Map::new();
    for (key, default_value) in default_object {
        let value = persisted_object
            .get(key)
            .map(|persisted_value| merge_value(default_value, persisted_value))
            .unwrap_or_else(|| default_value.clone());
        merged.insert(key.clone(), value);
    }
    // 允许但默认值里缺席的可选字段（如 `dataBaseDir`）：原样保留。
    for (key, value) in persisted_object {
        if !merged.contains_key(key) && allowed_keys().contains(key) {
            merged.insert(key.clone(), value.clone());
        }
    }
    Value::Object(merged)
}

/// 打补丁：浅合并（与 TS `{...current, ...patch}` 同义），再走一次归一化。
pub fn apply_patch(current: &Value, patch: &Value) -> Value {
    let mut merged = current.as_object().cloned().unwrap_or_default();
    if let Some(patch_object) = patch.as_object() {
        for (key, value) in patch_object {
            // TS 的 patch schema 会静默丢掉未知键；这里同样忽略。
            if allowed_keys().contains(key) {
                merged.insert(key.clone(), value.clone());
            }
        }
    }
    normalize(Value::Object(merged))
}

/// 归一化：补齐默认值、丢掉 schema 不认识的顶层键、整理 `recentProjects`。
pub fn normalize(value: Value) -> Value {
    let mut normalized = merge_persisted(&value);
    if let Some(items) = normalized
        .get_mut("recentProjects")
        .and_then(Value::as_array_mut)
    {
        let mut seen = HashSet::new();
        items.retain(|item| match item.as_str() {
            Some(text) => seen.insert(text.to_string()),
            None => true,
        });
        items.truncate(MAX_RECENT_PROJECTS);
    }
    normalized
}

fn merge_value(default_value: &Value, persisted: &Value) -> Value {
    match (default_value, persisted) {
        (Value::Object(default_object), Value::Object(persisted_object)) => {
            let mut merged = default_object.clone();
            for (key, value) in persisted_object {
                let next = match merged.get(key) {
                    Some(existing) => merge_value(existing, value),
                    None => value.clone(),
                };
                merged.insert(key.clone(), next);
            }
            Value::Object(merged)
        }
        (Value::Array(_), Value::Array(_)) => persisted.clone(),
        (Value::Bool(_), Value::Bool(_))
        | (Value::String(_), Value::String(_))
        | (Value::Number(_), Value::Number(_)) => persisted.clone(),
        (Value::Null, _) => persisted.clone(),
        _ => default_value.clone(),
    }
}
