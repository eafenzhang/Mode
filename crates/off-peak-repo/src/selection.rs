//! `ModelSelection` 的严格解析与落盘编码。
//!
//! 行为真源：`packages/shared/src/model-selection.ts` 的 `modelSelectionSchema`
//! 与 `offPeakTaskRepo.ts` 的 `serializeOffPeakModelSelection`。
//!
//! zod 的 `.strict()` 拒绝未知键、`.trim().min(1)` 会 trim 后校验非空；
//! 编码按 `providerId → modelId → options` 的对象字面量插入序手工拼接。

use serde::Serialize;
use serde_json::Value;

use crate::error::{OffPeakRepoError, Result};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelection {
    pub provider_id: String,
    pub model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<SelectionOptions>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_level: Option<String>,
}

fn parse_non_empty_trimmed(value: Option<&Value>) -> Option<String> {
    let trimmed = value?.as_str()?.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// 严格解析已落盘的 JSON；任何未知键或空字符串都判为不可用。
pub fn parse_model_selection(raw: &str) -> Option<ModelSelection> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let object = value.as_object()?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "providerId" | "modelId" | "options"))
    {
        return None;
    }
    let provider_id = parse_non_empty_trimmed(object.get("providerId"))?;
    let model_id = parse_non_empty_trimmed(object.get("modelId"))?;
    let options = match object.get("options") {
        None => None,
        Some(Value::Object(options)) => {
            if options.keys().any(|key| key.as_str() != "reasoningLevel") {
                return None;
            }
            let reasoning_level = match options.get("reasoningLevel") {
                None => None,
                Some(value) => Some(parse_non_empty_trimmed(Some(value))?),
            };
            Some(SelectionOptions { reasoning_level })
        }
        Some(_) => return None,
    };
    Some(ModelSelection {
        provider_id,
        model_id,
        options,
    })
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("JSON string serialization cannot fail")
}

/// 按 TS 对象插入序编码；`options` 为空对象时整个字段省略。
pub fn serialize_model_selection(selection: &ModelSelection) -> Result<String> {
    let provider_id = selection.provider_id.trim();
    let model_id = selection.model_id.trim();
    if provider_id.is_empty() {
        return Err(OffPeakRepoError::Message(
            "modelSelection.providerId 不能为空".to_owned(),
        ));
    }
    if model_id.is_empty() {
        return Err(OffPeakRepoError::Message(
            "modelSelection.modelId 不能为空".to_owned(),
        ));
    }
    let reasoning_level = match selection
        .options
        .as_ref()
        .and_then(|options| options.reasoning_level.as_deref())
    {
        Some(level) => {
            let level = level.trim();
            if level.is_empty() {
                return Err(OffPeakRepoError::Message(
                    "modelSelection.options.reasoningLevel 不能为空".to_owned(),
                ));
            }
            Some(level)
        }
        None => None,
    };
    let mut encoded = String::from("{\"providerId\":");
    encoded.push_str(&json_string(provider_id));
    encoded.push_str(",\"modelId\":");
    encoded.push_str(&json_string(model_id));
    if let Some(level) = reasoning_level {
        encoded.push_str(",\"options\":{\"reasoningLevel\":");
        encoded.push_str(&json_string(level));
        encoded.push('}');
    }
    encoded.push('}');
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_parse_rejects_unknown_keys_and_empty_values() {
        assert!(parse_model_selection(r#"{"providerId":"p","modelId":"m"}"#).is_some());
        assert!(parse_model_selection(r#"{"providerId":" p ","modelId":"m"}"#)
            .unwrap()
            .provider_id
            .eq("p"));
        assert!(parse_model_selection(r#"{"providerId":"p","modelId":"m","x":1}"#).is_none());
        assert!(parse_model_selection(r#"{"providerId":"  ","modelId":"m"}"#).is_none());
        assert!(parse_model_selection(r#"{"providerId":"p","modelId":"m","options":{"x":1}}"#)
            .is_none());
        assert!(parse_model_selection(
            r#"{"providerId":"p","modelId":"m","options":{"reasoningLevel":""}}"#
        )
        .is_none());
    }

    #[test]
    fn serialize_omits_empty_options_and_keeps_key_order() {
        let plain = ModelSelection {
            provider_id: "p".to_owned(),
            model_id: "m".to_owned(),
            options: None,
        };
        assert_eq!(
            serialize_model_selection(&plain).unwrap(),
            r#"{"providerId":"p","modelId":"m"}"#
        );
        let empty_options = ModelSelection {
            provider_id: "p".to_owned(),
            model_id: "m".to_owned(),
            options: Some(SelectionOptions {
                reasoning_level: None,
            }),
        };
        assert_eq!(
            serialize_model_selection(&empty_options).unwrap(),
            r#"{"providerId":"p","modelId":"m"}"#
        );
        let with_level = ModelSelection {
            provider_id: "p".to_owned(),
            model_id: "m".to_owned(),
            options: Some(SelectionOptions {
                reasoning_level: Some("high".to_owned()),
            }),
        };
        assert_eq!(
            serialize_model_selection(&with_level).unwrap(),
            r#"{"providerId":"p","modelId":"m","options":{"reasoningLevel":"high"}}"#
        );
    }
}
