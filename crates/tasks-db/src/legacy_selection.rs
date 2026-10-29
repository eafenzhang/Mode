//! 0002 冻结迁移：把旧 `automations.model/provider/thought_level` 转成
//! `model_selection`。
//!
//! 逐分支复刻 `packages/services/src/session/tasksDatabase/provider-selection-v2.ts`。
//! 这是发布前已裁决的编码，不能改成分散的 parser 或实时 provider 目录。

use rusqlite::{params, Connection};

use crate::error::TasksDbError;
use crate::js_compat::{decode_component, js_trim};

struct LegacyRow {
    automation_id: String,
    model: Option<String>,
    provider: Option<String>,
    thought_level: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct DecodedSelection {
    provider_id: String,
    model_id: String,
    reasoning_level: Option<String>,
}

/// TS 冻结映射：旧 `builtin:*` 编码 → 当前 provider 身份。
fn provider_name(provider: &str) -> Option<&'static str> {
    match provider {
        "builtin:bigmodel" => Some("bigmodel-api"),
        "builtin:zai" => Some("zai-api"),
        "builtin:bigmodel-start-plan" => Some("account:bigmodel-start-plan"),
        "builtin:zai-start-plan" => Some("account:zai-start-plan"),
        "builtin:bigmodel-coding-plan" => Some("account:bigmodel-individual-coding-plan"),
        "builtin:zai-coding-plan" => Some("account:zai-individual-coding-plan"),
        _ => None,
    }
}

fn decode_legacy_selection(row: &LegacyRow) -> Option<DecodedSelection> {
    let value = js_trim(row.model.as_deref()?);
    if value.is_empty() {
        return None;
    }
    let mut provider = row
        .provider
        .as_deref()
        .map(js_trim)
        .unwrap_or_default()
        .to_owned();
    let mut model = value.to_owned();
    let mut reasoning_level = row.thought_level.as_deref().map(js_trim).map(str::to_owned);

    if let Some(body) = value.strip_prefix("custom:") {
        let separator = body.find(':')?;
        let parts: Vec<&str> = body.split(':').collect();
        if parts.len() >= 3 && parts[0] == "builtin" {
            provider = format!("builtin:{}", parts[1]);
            model = decode_component(&parts[2..].join(":"));
        } else {
            provider = decode_component(&body[..separator]);
            model = decode_component(&body[separator + 1..]);
        }
    } else if let Some(separator) = value.find('/') {
        provider = value[..separator].to_owned();
        model = value[separator + 1..].to_owned();
        if let Some(level_separator) = model.find('$') {
            if level_separator > 0 && level_separator < model.len() - 1 {
                let level = js_trim(&model[level_separator + 1..]);
                if level.is_empty() {
                    return None;
                }
                reasoning_level = Some(level.to_owned());
                model = model[..level_separator].to_owned();
            }
        }
    } else if provider == "glm" || provider == "mode" {
        // 旧 provider=glm/mode 是执行后端，不是供应商身份。
        return None;
    }

    let provider = js_trim(&provider);
    let model = js_trim(&model);
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    let provider_id = if provider.starts_with("builtin:") {
        provider_name(provider)?
    } else {
        provider
    };
    Some(DecodedSelection {
        provider_id: provider_id.to_owned(),
        model_id: model.to_owned(),
        // TS 用 `reasoningLevel ?` 判真值：空串不进 options。
        reasoning_level: reasoning_level.filter(|level| !level.is_empty()),
    })
}

/// 按 TS 对象字面量插入序手工拼 JSON：`providerId`、`modelId`、`options`。
///
/// 不使用 `serde_json::Map`（默认 BTreeMap 会改键序）。字符串值仍交给
/// `serde_json::to_string` 生成与 `JSON.stringify` 一致的转义。
fn encode_selection(selection: &DecodedSelection) -> String {
    let mut encoded = String::from("{\"providerId\":");
    encoded.push_str(&json_string(&selection.provider_id));
    encoded.push_str(",\"modelId\":");
    encoded.push_str(&json_string(&selection.model_id));
    if let Some(level) = selection
        .reasoning_level
        .as_deref()
        .filter(|level| !level.is_empty())
    {
        encoded.push_str(",\"options\":{\"reasoningLevel\":");
        encoded.push_str(&json_string(level));
        encoded.push('}');
    }
    encoded.push('}');
    encoded
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("JSON string serialization cannot fail")
}

/// 冻结 0002 的一次转换：运行 Reader 只看新列，旧三列原样保留。
pub fn import_legacy_automation_selections(conn: &Connection) -> Result<(), TasksDbError> {
    let mut statement = conn.prepare(
        "SELECT automation_id, model, provider, thought_level FROM automations WHERE model IS NOT NULL",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok(LegacyRow {
                automation_id: row.get(0)?,
                model: row.get(1)?,
                provider: row.get(2)?,
                thought_level: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);

    for row in rows {
        match decode_legacy_selection(&row) {
            Some(selection) => {
                // 外层事务保证旧来源与写入同一快照；`IS ?` 做 NULL 安全比较。
                conn.execute(
                    "UPDATE automations SET model_selection = ?
                     WHERE automation_id = ?
                       AND model IS ? AND provider IS ? AND thought_level IS ?",
                    params![
                        encode_selection(&selection),
                        row.automation_id,
                        row.model,
                        row.provider,
                        row.thought_level,
                    ],
                )?;
            }
            None => {
                // 有旧显式意图但无法确定身份：不保留未发布中间态的“默认”。
                if row
                    .model
                    .as_deref()
                    .map(js_trim)
                    .is_some_and(|value| !value.is_empty())
                {
                    conn.execute(
                        "UPDATE automations SET model_selection=NULL WHERE automation_id=?",
                        params![row.automation_id],
                    )?;
                }
            }
        }
    }

    conn.execute_batch(
        "UPDATE automations SET model_selection='null'
    WHERE model_selection IS NULL AND (model IS NULL OR trim(model)='')",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{decode_legacy_selection, encode_selection, LegacyRow};

    fn row(model: Option<&str>, provider: Option<&str>, level: Option<&str>) -> LegacyRow {
        LegacyRow {
            automation_id: "a".to_owned(),
            model: model.map(str::to_owned),
            provider: provider.map(str::to_owned),
            thought_level: level.map(str::to_owned),
        }
    }

    #[test]
    fn decodes_builtin_custom_value() {
        let decoded = decode_legacy_selection(&row(
            Some("custom:builtin:zai-start-plan:glm-5.3"),
            None,
            None,
        ))
        .expect("decodable");
        assert_eq!(decoded.provider_id, "account:zai-start-plan");
        assert_eq!(decoded.model_id, "glm-5.3");
        assert_eq!(
            encode_selection(&decoded),
            r#"{"providerId":"account:zai-start-plan","modelId":"glm-5.3"}"#
        );
    }

    #[test]
    fn preserves_key_order_with_options() {
        let decoded = decode_legacy_selection(&row(Some("openai/gpt-5$high"), None, None))
            .expect("decodable");
        assert_eq!(
            encode_selection(&decoded),
            r#"{"providerId":"openai","modelId":"gpt-5","options":{"reasoningLevel":"high"}}"#
        );
    }

    #[test]
    fn rejects_glm_backend_and_empty_values() {
        assert!(decode_legacy_selection(&row(Some("glm-5"), Some("glm"), None)).is_none());
        assert!(decode_legacy_selection(&row(Some("   "), Some("openai"), None)).is_none());
        assert!(decode_legacy_selection(&row(Some("custom:no-colon"), None, None)).is_none());
        assert!(
            decode_legacy_selection(&row(Some("custom:builtin:unknown:glm-5"), None, None))
                .is_none()
        );
    }

    #[test]
    fn falls_back_to_raw_text_on_bad_percent_encoding() {
        let decoded = decode_legacy_selection(&row(Some("custom:bad%ZZ:model%2Fone"), None, None))
            .expect("decodable");
        assert_eq!(decoded.provider_id, "bad%ZZ");
        assert_eq!(decoded.model_id, "model/one");
    }
}
